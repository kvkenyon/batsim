//! End-to-end ingest pipeline tests: fixture files → parse → partition →
//! manifest → re-read. Uses the committed tiny xlsx fixture (generated via
//! openpyxl; see README) and inline CSV fixtures.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)]

use std::collections::BTreeMap;

use batsim_ercot::cpt;
use batsim_ercot::ingest::{self, ManifestMeta, ParseStats, ReportFormat, ReportKind};
use batsim_ercot::schema::{ManifestEntry, SCHEMA_VERSION_KEY, SIGNAL_RTM_SPP};
use batsim_ercot::types::{LoadZone, Location, PriceSample, Provenance};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use time::macros::datetime;
use time::{Duration, OffsetDateTime};

const XLSX_FIXTURE: &[u8] = include_bytes!("fixtures/rtm_sample_2023-08-17.xlsx");

fn group_by_day_location(
    samples: Vec<PriceSample>,
) -> BTreeMap<(time::Date, String), (Location, Vec<PriceSample>)> {
    let mut groups: BTreeMap<(time::Date, String), (Location, Vec<PriceSample>)> = BTreeMap::new();
    for sample in samples {
        let day = cpt::operating_day(sample.ts);
        let key = (day, sample.location.settlement_point());
        groups
            .entry(key)
            .or_insert_with(|| (sample.location.clone(), Vec::new()))
            .1
            .push(sample);
    }
    groups
}

fn ingest_prices(root: &std::path::Path, samples: Vec<PriceSample>) -> Vec<ManifestEntry> {
    let mut entries = Vec::new();
    for ((day, loc_name), (location, rows)) in group_by_day_location(samples) {
        let n = rows.len() as u64;
        let rel = ingest::write_price_partition(root, SIGNAL_RTM_SPP, day, &location, &rows, None)
            .unwrap();
        entries.push(ManifestEntry {
            signal: SIGNAL_RTM_SPP.to_string(),
            date: day
                .format(&time::macros::format_description!("[year]-[month]-[day]"))
                .unwrap(),
            location: loc_name,
            path: rel,
            rows: n,
            provenance: Provenance::SettlementFinal,
        });
    }
    entries
}

/// Re-read a partition and assert schema version + strictly increasing ts.
fn check_partition(path: &std::path::Path, expected_rows: u64) -> Vec<i64> {
    let file = std::fs::File::open(path).unwrap();
    let builder = ParquetRecordBatchReaderBuilder::try_new(file).unwrap();
    let kv = builder
        .metadata()
        .file_metadata()
        .key_value_metadata()
        .expect("schema version metadata");
    assert!(kv
        .iter()
        .any(|k| k.key == SCHEMA_VERSION_KEY && k.value.as_deref() == Some("1")));
    let mut ts: Vec<i64> = Vec::new();
    for batch in builder.build().unwrap() {
        let batch = batch.unwrap();
        let col = batch
            .column(0)
            .as_any()
            .downcast_ref::<arrow::array::Int64Array>()
            .unwrap();
        ts.extend_from_slice(col.values());
    }
    assert_eq!(ts.len() as u64, expected_rows);
    for w in ts.windows(2) {
        assert!(w[0] < w[1], "timestamps not strictly increasing");
    }
    ts
}

#[test]
fn ingest_xlsx_fixture_parses_to_expected_samples() {
    let parsed =
        ingest::parse_spp_report(ReportKind::RtmSpp, XLSX_FIXTURE, ReportFormat::Xlsx).unwrap();
    assert_eq!(parsed.rows.len(), 192);
    assert_eq!(
        parsed.stats,
        ParseStats {
            rows_read: 192,
            duplicates_skipped: 0,
            empty_skipped: 0,
        }
    );
    for row in &parsed.rows {
        assert_eq!(row.interval_secs, 900);
        assert_eq!(row.provenance, Provenance::SettlementFinal);
    }
    // First LZ_NORTH interval: 00:00 CPT = 05:00 UTC, price 10 + 1*0.5.
    let lz = Location::LoadZone(LoadZone::North);
    let first = parsed.rows.iter().find(|r| r.location == lz).unwrap();
    assert_eq!(first.ts, datetime!(2023-08-17 05:00 UTC));
    assert_eq!(first.lmp_usd_per_mwh, 10.5);
    // Last LZ_NORTH interval (n=96): 23:45 CPT = next day 04:45 UTC.
    let last = parsed
        .rows
        .iter()
        .filter(|r| r.location == lz)
        .last()
        .unwrap();
    assert_eq!(last.ts, datetime!(2023-08-18 04:45 UTC));
    assert_eq!(last.lmp_usd_per_mwh, 58.0);
    let hb = Location::from_settlement_point("HB_NORTH");
    let hb_first = parsed.rows.iter().find(|r| r.location == hb).unwrap();
    assert_eq!(hb_first.lmp_usd_per_mwh, 9.5);
}

#[test]
fn ingest_xlsx_fixture_full_pipeline_round_trip() {
    let parsed =
        ingest::parse_spp_report(ReportKind::RtmSpp, XLSX_FIXTURE, ReportFormat::Xlsx).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let entries = ingest_prices(tmp.path(), parsed.rows);
    assert_eq!(entries.len(), 2);
    let meta = ManifestMeta {
        rules_version: "v2025".to_string(),
        source_report: "13061".to_string(),
        source_doc_ids: vec![],
        ingested_at: "2026-07-27T00:00:00Z".to_string(),
    };
    let manifest = ingest::upsert_manifest(tmp.path(), &meta, &entries).unwrap();
    assert_eq!(manifest.entries.len(), 2);
    assert_eq!(manifest.entries[0].location, "HB_NORTH");
    assert_eq!(manifest.entries[1].location, "LZ_NORTH");
    for entry in &manifest.entries {
        assert_eq!(entry.rows, 96);
        let ts = check_partition(&tmp.path().join(&entry.path), 96);
        assert_eq!(ts.len(), 96);
    }
    // Manifest re-reads identically (deterministic serialization).
    let reread = ingest::read_manifest(tmp.path()).unwrap().unwrap();
    let a = serde_json::to_string_pretty(&reread).unwrap();
    let b = std::fs::read_to_string(tmp.path().join("manifest.json")).unwrap();
    assert_eq!(format!("{a}\n"), b);
}

#[test]
fn ingest_dst_fall_back_day_full_pipeline() {
    // Build the whole 2023-11-05 fall-back day as CSV (hour 2 twice), parse,
    // write, and verify 100 strictly increasing UTC intervals on disk.
    let mut csv = String::from(
        "Delivery Date,Delivery Hour,Delivery Interval,Repeated Hour Flag,Settlement Point Name,Settlement Point Type,Settlement Point Price\n",
    );
    for h in 1..=24u8 {
        let passes: &[&str] = if h == 2 { &["N", "Y"] } else { &["N"] };
        for flag in passes {
            for i in 1..=4 {
                csv.push_str(&format!("11/05/2023,{h},{i},{flag},LZ_HOUSTON,LZ,20.5\n"));
            }
        }
    }
    let parsed =
        ingest::parse_spp_report(ReportKind::RtmSpp, csv.as_bytes(), ReportFormat::Csv).unwrap();
    assert_eq!(parsed.rows.len(), 100);
    let tmp = tempfile::tempdir().unwrap();
    let entries = ingest_prices(tmp.path(), parsed.rows);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].date, "2023-11-05");
    let ts = check_partition(&tmp.path().join(&entries[0].path), 100);
    // 100 contiguous 15-min steps spanning 25 hours.
    for w in ts.windows(2) {
        assert_eq!(w[1] - w[0], 900);
    }
    let start = OffsetDateTime::from_unix_timestamp(ts[0]).unwrap();
    let end = OffsetDateTime::from_unix_timestamp(*ts.last().unwrap()).unwrap();
    assert_eq!(start, datetime!(2023-11-05 05:00 UTC));
    assert_eq!(end, datetime!(2023-11-06 05:45 UTC));
    assert_eq!(end - start, Duration::hours(24) + Duration::minutes(45));
}
