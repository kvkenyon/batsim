//! Canonical Parquet partition writers + `manifest.json` handling.
//!
//! Pure functions: no network, no wall clock. The ingest timestamp and rules
//! version are supplied by the caller ([`ManifestMeta`]), so output is
//! deterministic given identical inputs.
//!
//! Layout (see [`crate::schema`]):
//! `<root>/<signal>/date=YYYY-MM-DD/location=<LOC>.parquet` plus
//! `<root>/manifest.json`. Location-less signals use location dir
//! [`ALL_LOCATION`]. Every file carries `batsim.schema_version = "1"` in its
//! Parquet key-value metadata.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;

use arrow::array::{Float64Array, Int64Array, StringArray, UInt32Array};
use arrow::datatypes::{DataType, Field, Schema};
use arrow::record_batch::RecordBatch;
use parquet::arrow::ArrowWriter;
use parquet::basic::Compression;
use parquet::file::properties::WriterProperties;
use parquet::format::KeyValue;
use time::macros::format_description;
use time::{Date, OffsetDateTime};

use crate::error::{ErcotError, Result};
use crate::schema::{
    as_cols, load_cols, price_cols, Manifest, ManifestEntry, SCHEMA_VERSION, SCHEMA_VERSION_KEY,
    SIGNAL_DAM_SPP, SIGNAL_RTM_SPP,
};
use crate::types::{AsProduct, Location, PriceSample, Provenance};

/// Location partition directory name for location-less signals
/// (`as_mcpc`, `system_load`).
pub const ALL_LOCATION: &str = "ALL";

/// Metadata supplied by the caller for one ingest run.
///
/// Kept separate from the entry list so [`upsert_manifest`] stays pure: the
/// binary fills `ingested_at` from the wall clock, the library never does.
#[derive(Debug, Clone)]
pub struct ManifestMeta {
    /// ERCOT rules version (`ErcotRules::meta.protocol_version`).
    pub rules_version: String,
    /// Source report identifier (ERCOT report type ID or `"synthetic"`).
    pub source_report: String,
    /// ERCOT MIS DocID(s) the data was parsed from, when fetched.
    pub source_doc_ids: Vec<u64>,
    /// Ingest timestamp, RFC 3339 UTC.
    pub ingested_at: String,
}

/// Relative partition path: `<signal>/date=YYYY-MM-DD/location=<LOC>.parquet`.
///
/// Always forward-slash separated; this exact string is stored in the
/// manifest and joined onto the archive root on read.
#[must_use]
pub fn partition_rel_path(signal: &str, date_cpt: Date, location_dir: &str) -> String {
    let date_fmt = format_description!("[year]-[month]-[day]");
    // `format!` on a fixed picture cannot fail meaningfully; fall back to the
    // Debug form rather than panic.
    let date = date_cpt
        .format(&date_fmt)
        .unwrap_or_else(|_| format!("{date_cpt:?}"));
    format!("{signal}/date={date}/location={location_dir}.parquet")
}

/// Write one price partition (`rtm_spp` or `dam_spp`).
///
/// Rows are sorted by `ts` before writing; duplicate timestamps within the
/// partition are rejected (a partition holds exactly one location, so a
/// duplicate `ts` is a source-data conflict). `lmp_raw`, when given, carries
/// pre-correction values for auditability and must match `rows` 1:1; when
/// `None`, the raw column mirrors `lmp`.
///
/// Returns the partition path relative to `root` (for the manifest).
///
/// # Errors
/// - `InvalidParam`: unknown signal, empty rows, `lmp_raw` length mismatch,
///   row location mismatch, or duplicate timestamps.
/// - `Io` / `Parquet`: filesystem or writer failure.
pub fn write_price_partition(
    root: &Path,
    signal: &str,
    date_cpt: Date,
    location: &Location,
    rows: &[PriceSample],
    lmp_raw: Option<&[f64]>,
) -> Result<String> {
    if signal != SIGNAL_RTM_SPP && signal != SIGNAL_DAM_SPP {
        return Err(ErcotError::InvalidParam(format!(
            "write_price_partition: signal must be {SIGNAL_RTM_SPP} or {SIGNAL_DAM_SPP}, got {signal:?}"
        )));
    }
    if rows.is_empty() {
        return Err(ErcotError::InvalidParam(
            "write_price_partition: no rows".to_string(),
        ));
    }
    if let Some(raw) = lmp_raw {
        if raw.len() != rows.len() {
            return Err(ErcotError::InvalidParam(format!(
                "write_price_partition: lmp_raw len {} != rows len {}",
                raw.len(),
                rows.len()
            )));
        }
    }
    let location_dir = location.settlement_point();
    check_location_dir(&location_dir)?;
    // Pair each row with its raw value BEFORE sorting so `lmp_raw` stays
    // positional to its row.
    let mut sorted: Vec<(&PriceSample, f64)> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| (r, lmp_raw.map_or(r.lmp_usd_per_mwh, |raw| raw[i])))
        .collect();
    sorted.sort_by_key(|(r, _)| r.ts);
    check_unique_ts(sorted.iter().map(|(r, _)| r.ts), "write_price_partition")?;

    let n = sorted.len();
    let mut ts: Vec<i64> = Vec::with_capacity(n);
    let mut secs: Vec<u32> = Vec::with_capacity(n);
    let mut locs: Vec<&str> = Vec::with_capacity(n);
    let mut lmp: Vec<f64> = Vec::with_capacity(n);
    let mut ordc: Vec<f64> = Vec::with_capacity(n);
    let mut rdpa: Vec<f64> = Vec::with_capacity(n);
    let mut prov_labels: Vec<&'static str> = Vec::with_capacity(n);
    let mut raw: Vec<f64> = Vec::with_capacity(n);
    for (row, raw_value) in &sorted {
        if row.location != *location {
            return Err(ErcotError::InvalidParam(format!(
                "write_price_partition: row location {} != partition location {location}",
                row.location
            )));
        }
        ts.push(row.ts.unix_timestamp());
        secs.push(row.interval_secs);
        locs.push(&location_dir);
        lmp.push(row.lmp_usd_per_mwh);
        ordc.push(row.ordc_adder_usd_per_mwh);
        rdpa.push(row.rdpa_adder_usd_per_mwh);
        prov_labels.push(provenance_label(row.provenance));
        raw.push(*raw_value);
    }

    let schema = price_schema();
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(Int64Array::from(ts)),
            Arc::new(UInt32Array::from(secs)),
            Arc::new(StringArray::from(locs)),
            Arc::new(Float64Array::from(lmp)),
            Arc::new(Float64Array::from(ordc)),
            Arc::new(Float64Array::from(rdpa)),
            Arc::new(StringArray::from(prov_labels)),
            Arc::new(Float64Array::from(raw)),
        ],
    )?;
    let rel = partition_rel_path(signal, date_cpt, &location_dir);
    write_batch(&root.join(&rel), schema, &batch)?;
    Ok(rel)
}

/// Write one DAM AS clearing-price partition (`as_mcpc`, location `ALL`).
///
/// Each row is `(ts, product, mcpc_usd_per_mw, provenance)`; rows are sorted
/// by `(ts, product)` before writing. Duplicate `(ts, product)` pairs are
/// rejected.
///
/// Returns the partition path relative to `root`.
///
/// # Errors
/// - `InvalidParam`: empty rows or duplicate `(ts, product)` pairs.
/// - `Io` / `Parquet`: filesystem or writer failure.
pub fn write_as_partition(
    root: &Path,
    date_cpt: Date,
    rows: &[(OffsetDateTime, AsProduct, f64, Provenance)],
) -> Result<String> {
    if rows.is_empty() {
        return Err(ErcotError::InvalidParam(
            "write_as_partition: no rows".to_string(),
        ));
    }
    let mut sorted = rows.to_vec();
    sorted.sort_by_key(|r| (r.0, r.1));
    let mut prev: Option<(OffsetDateTime, AsProduct)> = None;
    for (row_ts, product, _, _) in &sorted {
        if prev == Some((*row_ts, *product)) {
            return Err(ErcotError::InvalidParam(format!(
                "write_as_partition: duplicate ({row_ts}, {product})"
            )));
        }
        prev = Some((*row_ts, *product));
    }

    let n = sorted.len();
    let mut ts: Vec<i64> = Vec::with_capacity(n);
    let mut products: Vec<&str> = Vec::with_capacity(n);
    let mut mcpc: Vec<f64> = Vec::with_capacity(n);
    let mut prov_labels: Vec<&str> = Vec::with_capacity(n);
    for (row_ts, product, price, provenance) in &sorted {
        ts.push(row_ts.unix_timestamp());
        products.push(product.dam_column());
        mcpc.push(*price);
        prov_labels.push(provenance_label(*provenance));
    }

    let schema = as_schema();
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(Int64Array::from(ts)),
            Arc::new(StringArray::from(products)),
            Arc::new(Float64Array::from(mcpc)),
            Arc::new(StringArray::from(prov_labels)),
        ],
    )?;
    let rel = partition_rel_path(crate::schema::SIGNAL_AS_MCPC, date_cpt, ALL_LOCATION);
    write_batch(&root.join(&rel), schema, &batch)?;
    Ok(rel)
}

/// Write one system-load partition (`system_load`, location `ALL`).
///
/// Each row is `(ts, system_load_mw, reserves_mw)`; `reserves_mw` is
/// nullable. Rows are sorted by `ts`; duplicate timestamps are rejected.
///
/// Returns the partition path relative to `root`.
///
/// # Errors
/// - `InvalidParam`: empty rows or duplicate timestamps.
/// - `Io` / `Parquet`: filesystem or writer failure.
pub fn write_load_partition(
    root: &Path,
    date_cpt: Date,
    rows: &[(OffsetDateTime, f64, Option<f64>)],
) -> Result<String> {
    if rows.is_empty() {
        return Err(ErcotError::InvalidParam(
            "write_load_partition: no rows".to_string(),
        ));
    }
    let mut sorted = rows.to_vec();
    sorted.sort_by_key(|r| r.0);
    check_unique_ts(sorted.iter().map(|r| r.0), "write_load_partition")?;

    let ts: Vec<i64> = sorted.iter().map(|r| r.0.unix_timestamp()).collect();
    let load: Vec<f64> = sorted.iter().map(|r| r.1).collect();
    let reserves: Vec<Option<f64>> = sorted.iter().map(|r| r.2).collect();

    let schema = load_schema();
    let batch = RecordBatch::try_new(
        schema.clone(),
        vec![
            Arc::new(Int64Array::from(ts)),
            Arc::new(Float64Array::from(load)),
            Arc::new(Float64Array::from(reserves)),
        ],
    )?;
    let rel = partition_rel_path(crate::schema::SIGNAL_SYSTEM_LOAD, date_cpt, ALL_LOCATION);
    write_batch(&root.join(&rel), schema, &batch)?;
    Ok(rel)
}

/// Read `manifest.json` from an archive root.
///
/// Returns `Ok(None)` when the file does not exist (fresh archive).
///
/// # Errors
/// - `Parse`: the file exists but is not valid manifest JSON.
/// - `Io`: read failure.
pub fn read_manifest(root: &Path) -> Result<Option<Manifest>> {
    let path = root.join("manifest.json");
    let text = match fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let manifest: Manifest = serde_json::from_str(&text).map_err(|e| ErcotError::Parse {
        context: path.display().to_string(),
        detail: e.to_string(),
    })?;
    Ok(Some(manifest))
}

/// Upsert dataset entries into `manifest.json` and write it back.
///
/// Entries are keyed by `(signal, date, location)`; an incoming entry
/// replaces an existing one with the same key. The written manifest has
/// entries sorted by `(signal, date, location)` and pretty-printed JSON, so
/// serialization is deterministic. Run-level metadata ([`ManifestMeta`])
/// overwrites the stored values; `source_doc_ids` is unioned with any
/// previously recorded IDs (sorted, deduplicated).
///
/// Returns the merged manifest.
///
/// # Errors
/// - `Parse`: an existing manifest is malformed.
/// - `Io`: read/write failure.
pub fn upsert_manifest(root: &Path, meta: &ManifestMeta, entries: &[ManifestEntry]) -> Result<Manifest> {
    let existing = read_manifest(root)?;
    let mut by_key: BTreeMap<(String, String, String), ManifestEntry> = BTreeMap::new();
    let mut doc_ids: Vec<u64> = meta.source_doc_ids.clone();
    if let Some(prev) = existing {
        doc_ids.extend(prev.source_doc_ids);
        for entry in prev.entries {
            by_key.insert(entry_key(&entry), entry);
        }
    }
    for entry in entries {
        by_key.insert(entry_key(entry), entry.clone());
    }
    doc_ids.sort_unstable();
    doc_ids.dedup();
    let manifest = Manifest {
        schema_version: SCHEMA_VERSION,
        rules_version: meta.rules_version.clone(),
        source_report: meta.source_report.clone(),
        source_doc_ids: doc_ids,
        ingested_at: meta.ingested_at.clone(),
        entries: by_key.into_values().collect(),
    };
    let mut text = serde_json::to_string_pretty(&manifest).map_err(|e| ErcotError::Parse {
        context: "manifest.json serialization".to_string(),
        detail: e.to_string(),
    })?;
    text.push('\n');
    fs::create_dir_all(root)?;
    fs::write(root.join("manifest.json"), text)?;
    Ok(manifest)
}

/// `Provenance` as its serde snake_case label (parquet column value).
#[must_use]
pub const fn provenance_label(provenance: Provenance) -> &'static str {
    match provenance {
        Provenance::RealTimeIndicative => "real_time_indicative",
        Provenance::SettlementFinal => "settlement_final",
        Provenance::Corrected => "corrected",
        Provenance::Synthetic => "synthetic",
        Provenance::Omitted => "omitted",
    }
}

fn check_location_dir(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(ErcotError::InvalidParam(format!(
            "location name {name:?} must be non-empty and match [A-Za-z0-9_.-]+"
        )))
    }
}

fn entry_key(entry: &ManifestEntry) -> (String, String, String) {
    (
        entry.signal.clone(),
        entry.date.clone(),
        entry.location.clone(),
    )
}

fn check_unique_ts(
    mut ts: impl Iterator<Item = OffsetDateTime>,
    context: &str,
) -> Result<()> {
    let mut prev: Option<OffsetDateTime> = None;
    for t in &mut ts {
        if let Some(p) = prev {
            if t <= p {
                return Err(ErcotError::InvalidParam(format!(
                    "{context}: timestamps not strictly increasing ({p} then {t})"
                )));
            }
        }
        prev = Some(t);
    }
    Ok(())
}

fn write_batch(path: &Path, schema: Arc<Schema>, batch: &RecordBatch) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let file = fs::File::create(path)?;
    let props = WriterProperties::builder()
        .set_compression(Compression::SNAPPY)
        .set_key_value_metadata(Some(vec![KeyValue::new(
            SCHEMA_VERSION_KEY.to_string(),
            SCHEMA_VERSION.to_string(),
        )]))
        .build();
    let mut writer = ArrowWriter::try_new(file, schema, Some(props))?;
    writer.write(batch)?;
    writer.close()?;
    Ok(())
}

fn price_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new(price_cols::TS, DataType::Int64, false),
        Field::new(price_cols::INTERVAL_SECS, DataType::UInt32, false),
        Field::new(price_cols::LOCATION, DataType::Utf8, false),
        Field::new(price_cols::LMP, DataType::Float64, false),
        Field::new(price_cols::ORDC, DataType::Float64, false),
        Field::new(price_cols::RDPA, DataType::Float64, false),
        Field::new(price_cols::PROVENANCE, DataType::Utf8, false),
        Field::new(price_cols::LMP_RAW, DataType::Float64, false),
    ]))
}

fn as_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new(as_cols::TS, DataType::Int64, false),
        Field::new(as_cols::PRODUCT, DataType::Utf8, false),
        Field::new(as_cols::MCPC, DataType::Float64, false),
        Field::new(as_cols::PROVENANCE, DataType::Utf8, false),
    ]))
}

fn load_schema() -> Arc<Schema> {
    Arc::new(Schema::new(vec![
        Field::new(load_cols::TS, DataType::Int64, false),
        Field::new(load_cols::LOAD, DataType::Float64, false),
        Field::new(load_cols::RESERVES, DataType::Float64, true),
    ]))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)]
mod tests {
    use super::*;
    use arrow::array::Array;
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    use time::macros::datetime;
    use time::Month;

    fn d(day: u8) -> Date {
        Date::from_calendar_date(2023, Month::August, day).unwrap()
    }

    fn sample(hour: u8, price: f64) -> PriceSample {
        PriceSample {
            ts: datetime!(2023-08-17 0:00 UTC) + time::Duration::hours(i64::from(hour)),
            interval_secs: 900,
            location: Location::LoadZone(crate::types::LoadZone::North),
            lmp_usd_per_mwh: price,
            ordc_adder_usd_per_mwh: 0.0,
            rdpa_adder_usd_per_mwh: 0.0,
            provenance: Provenance::SettlementFinal,
        }
    }

    #[test]
    fn price_partition_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let rows = vec![sample(1, 20.0), sample(0, 10.5), sample(2, -5.0)];
        let raw: Vec<f64> = rows.iter().map(|r| r.lmp_usd_per_mwh + 1.0).collect();
        let rel = write_price_partition(
            tmp.path(),
            SIGNAL_RTM_SPP,
            d(17),
            &Location::LoadZone(crate::types::LoadZone::North),
            &rows,
            Some(&raw),
        )
        .unwrap();
        assert_eq!(rel, "rtm_spp/date=2023-08-17/location=LZ_NORTH.parquet");
        assert!(tmp.path().join(&rel).exists());

        // Read back: schema version metadata + sorted ts + values.
        let file = fs::File::open(tmp.path().join(&rel)).unwrap();
        let builder = ParquetRecordBatchReaderBuilder::try_new(file).unwrap();
        let kv = builder.metadata().file_metadata().key_value_metadata().unwrap();
        assert!(kv
            .iter()
            .any(|k| k.key == SCHEMA_VERSION_KEY && k.value.as_deref() == Some("1")));
        let batches: Vec<RecordBatch> = builder
            .build()
            .unwrap()
            .collect::<std::result::Result<_, _>>()
            .unwrap();
        assert_eq!(batches.len(), 1);
        let batch = &batches[0];
        assert_eq!(batch.num_rows(), 3);
        assert_eq!(batch.schema().field(0).name(), price_cols::TS);
        assert_eq!(batch.schema().field(7).name(), price_cols::LMP_RAW);
        let ts = batch
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(ts.value(0), sample(0, 0.0).ts.unix_timestamp());
        assert!(ts.value(0) < ts.value(1) && ts.value(1) < ts.value(2));
        let lmp = batch
            .column(3)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert_eq!(lmp.value(0), 10.5);
        let raw_col = batch
            .column(7)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert_eq!(raw_col.value(0), 11.5);
        let prov = batch
            .column(6)
            .as_any()
            .downcast_ref::<StringArray>()
            .unwrap();
        assert_eq!(prov.value(0), "settlement_final");
        let secs = batch
            .column(1)
            .as_any()
            .downcast_ref::<UInt32Array>()
            .unwrap();
        assert_eq!(secs.value(0), 900);
    }

    #[test]
    fn price_partition_rejects_duplicate_ts() {
        let tmp = tempfile::tempdir().unwrap();
        let rows = vec![sample(0, 1.0), sample(0, 2.0)];
        let err = write_price_partition(
            tmp.path(),
            SIGNAL_RTM_SPP,
            d(17),
            &Location::LoadZone(crate::types::LoadZone::North),
            &rows,
            None,
        );
        assert!(matches!(err, Err(ErcotError::InvalidParam(_))));
    }

    #[test]
    fn as_and_load_partitions_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let t0 = datetime!(2023-08-17 5:00 UTC);
        let as_rows = vec![
            (t0, AsProduct::Ecrs, 12.5, Provenance::SettlementFinal),
            (t0, AsProduct::Rrs, 7.25, Provenance::SettlementFinal),
            (
                t0 + time::Duration::hours(1),
                AsProduct::Ecrs,
                9.0,
                Provenance::SettlementFinal,
            ),
        ];
        let rel = write_as_partition(tmp.path(), d(17), &as_rows).unwrap();
        assert_eq!(rel, "as_mcpc/date=2023-08-17/location=ALL.parquet");

        let load_rows = vec![(t0, 65_000.0, Some(3_100.0)), (t0 + time::Duration::hours(1), 64_000.0, None)];
        let rel = write_load_partition(tmp.path(), d(17), &load_rows).unwrap();
        assert_eq!(rel, "system_load/date=2023-08-17/location=ALL.parquet");

        // Nullable reserves round-trips as null.
        let file = fs::File::open(tmp.path().join(&rel)).unwrap();
        let batch = ParquetRecordBatchReaderBuilder::try_new(file)
            .unwrap()
            .build()
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        let reserves_col = batch
            .column(2)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap();
        assert!(reserves_col.is_valid(0));
        assert!(reserves_col.is_null(1));
    }

    #[test]
    fn manifest_upsert_is_deterministic() {
        let tmp = tempfile::tempdir().unwrap();
        let meta = ManifestMeta {
            rules_version: "v2025".to_string(),
            source_report: "13061".to_string(),
            source_doc_ids: vec![9, 3],
            ingested_at: "2026-07-27T00:00:00Z".to_string(),
        };
        let entry = |signal: &str, date: &str, loc: &str, rows: u64| ManifestEntry {
            signal: signal.to_string(),
            date: date.to_string(),
            location: loc.to_string(),
            path: format!("{signal}/date={date}/location={loc}.parquet"),
            rows,
            provenance: Provenance::SettlementFinal,
        };
        let m1 = upsert_manifest(
            tmp.path(),
            &meta,
            &[entry(SIGNAL_RTM_SPP, "2023-08-18", "LZ_SOUTH", 96), entry(SIGNAL_RTM_SPP, "2023-08-17", "LZ_NORTH", 96)],
        )
        .unwrap();
        // Sorted by (signal, date, location).
        assert_eq!(m1.entries[0].date, "2023-08-17");
        assert_eq!(m1.entries[1].date, "2023-08-18");
        assert_eq!(m1.source_doc_ids, vec![3, 9]);

        // Second upsert: replace one entry, add another; doc IDs union.
        let meta2 = ManifestMeta { source_doc_ids: vec![10], ..meta.clone() };
        let m2 = upsert_manifest(
            tmp.path(),
            &meta2,
            &[
                entry(SIGNAL_RTM_SPP, "2023-08-17", "LZ_NORTH", 100),
                entry(SIGNAL_DAM_SPP, "2023-08-17", "LZ_NORTH", 24),
            ],
        )
        .unwrap();
        assert_eq!(m2.entries.len(), 3);
        assert_eq!(m2.entries[0].signal, SIGNAL_DAM_SPP); // dam_spp sorts before rtm_spp
        assert_eq!(m2.entries[0].rows, 24);
        assert_eq!(m2.entries[1].rows, 100);
        assert_eq!(m2.source_doc_ids, vec![3, 9, 10]);

        // Byte-identical serialization for identical input sequences.
        let tmp2 = tempfile::tempdir().unwrap();
        upsert_manifest(
            tmp2.path(),
            &meta,
            &[entry(SIGNAL_RTM_SPP, "2023-08-18", "LZ_SOUTH", 96), entry(SIGNAL_RTM_SPP, "2023-08-17", "LZ_NORTH", 96)],
        )
        .unwrap();
        upsert_manifest(
            tmp2.path(),
            &meta2,
            &[
                entry(SIGNAL_RTM_SPP, "2023-08-17", "LZ_NORTH", 100),
                entry(SIGNAL_DAM_SPP, "2023-08-17", "LZ_NORTH", 24),
            ],
        )
        .unwrap();
        let a = fs::read_to_string(tmp.path().join("manifest.json")).unwrap();
        let b = fs::read_to_string(tmp2.path().join("manifest.json")).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn provenance_labels_match_serde() {
        for p in [
            Provenance::RealTimeIndicative,
            Provenance::SettlementFinal,
            Provenance::Corrected,
            Provenance::Synthetic,
            Provenance::Omitted,
        ] {
            let serde_name = serde_json::to_string(&p).unwrap();
            assert_eq!(serde_name, format!("\"{}\"", provenance_label(p)));
        }
    }

    #[test]
    fn empty_rows_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let err = write_price_partition(
            tmp.path(),
            SIGNAL_RTM_SPP,
            d(17),
            &Location::LoadZone(crate::types::LoadZone::North),
            &[],
            None,
        );
        assert!(matches!(err, Err(ErcotError::InvalidParam(_))));
        let err = write_as_partition(tmp.path(), d(17), &[]);
        assert!(matches!(err, Err(ErcotError::InvalidParam(_))));
        let err = write_load_partition(tmp.path(), d(17), &[]);
        assert!(matches!(err, Err(ErcotError::InvalidParam(_))));
    }
}
