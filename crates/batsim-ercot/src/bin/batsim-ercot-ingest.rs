//! `batsim-ercot-ingest`: ERCOT MIS → Parquet archive ingestion pipeline
//! (spec D.3.3).
//!
//! Subcommands:
//! - `fetch`: download a yearly report from ERCOT MIS and ingest it.
//! - `import`: ingest a local report file (`.xlsx` / `.csv` / `.zip`).
//! - `verify`: re-check an archive against its manifest.
//! - `synth`: write one synthetic day (requires `batsim_ercot::synthetic`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use arrow::array::{Array, Int64Array};
use batsim_ercot::ingest;
use batsim_ercot::rules::ErcotRules;
use batsim_ercot::schema::{ManifestEntry, SIGNALS, SIGNAL_DAM_SPP, SIGNAL_RTM_SPP};
use batsim_ercot::synthetic::{Season, SyntheticParams, SyntheticPriceGenerator};
use batsim_ercot::{cpt, ErcotError, PriceSource, Provenance, Result, TimeRange};
use clap::{Parser, Subcommand};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use time::macros::format_description;
use time::{Date, OffsetDateTime};

use batsim_ercot::types::{AsPrice, Location, PriceSample};

/// ERCOT MIS ingestion pipeline (spec D.3.3).
#[derive(Parser)]
#[command(name = "batsim-ercot-ingest", version, about)]
struct Cli {
    /// Subcommand.
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Download a yearly report from ERCOT MIS and ingest it.
    Fetch {
        /// Report family.
        #[arg(long, value_parser = parse_report_kind)]
        report: ingest::ReportKind,
        /// Calendar year (e.g. 2023).
        #[arg(long)]
        year: i32,
        /// Archive root to write into.
        #[arg(long)]
        out: PathBuf,
    },
    /// Ingest a local report file (.xlsx / .csv / .zip-of-csv).
    Import {
        /// Report family.
        #[arg(long, value_parser = parse_report_kind)]
        report: ingest::ReportKind,
        /// Local report file.
        #[arg(long)]
        file: PathBuf,
        /// Archive root to write into.
        #[arg(long)]
        out: PathBuf,
    },
    /// Verify an archive against its manifest.json.
    Verify {
        /// Archive root.
        #[arg(long)]
        root: PathBuf,
    },
    /// Generate one synthetic day and write it to the archive.
    Synth {
        /// Archive root to write into.
        #[arg(long)]
        out: PathBuf,
        /// CPT operating day (YYYY-MM-DD).
        #[arg(long)]
        date: String,
        /// Settlement point (e.g. LZ_HOUSTON).
        #[arg(long)]
        location: String,
        /// Generator seed.
        #[arg(long)]
        seed: u64,
        /// RTM cadence in seconds.
        #[arg(long, default_value_t = 900)]
        interval_secs: u32,
    },
}

fn parse_report_kind(s: &str) -> std::result::Result<ingest::ReportKind, String> {
    s.parse().map_err(|e: ErcotError| e.to_string())
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<()> {
    match cli.command {
        Command::Fetch { report, year, out } => run_fetch(report, year, &out),
        Command::Import { report, file, out } => run_import(report, &file, &out),
        Command::Verify { root } => run_verify(&root),
        Command::Synth {
            out,
            date,
            location,
            seed,
            interval_secs,
        } => run_synth(&out, &date, &location, seed, interval_secs),
    }
}

// ---------------------------------------------------------------------------
// fetch / import
// ---------------------------------------------------------------------------

fn run_fetch(report: ingest::ReportKind, year: i32, out: &Path) -> Result<()> {
    let agent = ingest::http_agent();
    eprintln!(
        "fetch: listing MIS documents for report {} (type {})",
        report,
        report.report_type_id()
    );
    let docs = ingest::list_documents(&agent, report.report_type_id())?;
    let doc = ingest::find_year_document(&docs, year)?;
    eprintln!(
        "fetch: downloading DocID {} ({:?})",
        doc.doc_id, doc.friendly_name
    );
    let stage = std::env::temp_dir().join(format!(
        "batsim-ercot-ingest-{}-{}",
        std::process::id(),
        doc.doc_id
    ));
    let result = (|| {
        let file = ingest::download_document(&agent, &doc, &stage)?;
        eprintln!("fetch: downloaded {}", file.display());
        let bytes = std::fs::read(&file)?;
        let format = ingest::ReportFormat::from_path(&file)
            .unwrap_or_else(|| ingest::ReportFormat::sniff(&bytes));
        run_ingest(report, &bytes, format, out, vec![doc.doc_id])
    })();
    let _ = std::fs::remove_dir_all(&stage);
    result
}

fn run_import(report: ingest::ReportKind, file: &Path, out: &Path) -> Result<()> {
    let bytes = std::fs::read(file)?;
    let format = ingest::ReportFormat::from_path(file)
        .unwrap_or_else(|| ingest::ReportFormat::sniff(&bytes));
    eprintln!(
        "import: {} ({format:?}, {} bytes)",
        file.display(),
        bytes.len()
    );
    run_ingest(report, &bytes, format, out, Vec::new())
}

/// Parse → partition → write → manifest. Shared by `fetch` and `import`.
fn run_ingest(
    report: ingest::ReportKind,
    bytes: &[u8],
    format: ingest::ReportFormat,
    out: &Path,
    doc_ids: Vec<u64>,
) -> Result<()> {
    let entries = match report {
        ingest::ReportKind::RtmSpp | ingest::ReportKind::DamSpp => {
            let parsed = ingest::parse_spp_report(report, bytes, format)?;
            eprintln!(
                "parse: {} rows read, {} kept, {} duplicates dropped, {} empty",
                parsed.stats.rows_read,
                parsed.rows.len(),
                parsed.stats.duplicates_skipped,
                parsed.stats.empty_skipped
            );
            write_price_groups(report.signal(), parsed.rows, out)?
        }
        ingest::ReportKind::AsMcpc => {
            let parsed = ingest::parse_as_report(bytes, format)?;
            eprintln!(
                "parse: {} rows read, {} kept, {} duplicates dropped, {} empty",
                parsed.stats.rows_read,
                parsed.rows.len(),
                parsed.stats.duplicates_skipped,
                parsed.stats.empty_skipped
            );
            write_as_groups(parsed.rows, out)?
        }
    };
    let meta = ingest::ManifestMeta {
        rules_version: ErcotRules::current()?.meta.protocol_version,
        source_report: report.report_type_id().to_string(),
        source_doc_ids: doc_ids,
        ingested_at: now_rfc3339(),
    };
    let manifest = ingest::upsert_manifest(out, &meta, &entries)?;
    eprintln!(
        "manifest: {} entries total ({} new/updated), rules {}",
        manifest.entries.len(),
        entries.len(),
        meta.rules_version
    );
    Ok(())
}

/// Group samples by (operating day, location) and write one partition each.
fn write_price_groups(
    signal: &str,
    samples: Vec<PriceSample>,
    out: &Path,
) -> Result<Vec<ManifestEntry>> {
    let mut groups: BTreeMap<(Date, String), (Location, Vec<PriceSample>)> = BTreeMap::new();
    for sample in samples {
        let day = batsim_ercot::cpt::operating_day(sample.ts);
        let key = (day, sample.location.settlement_point());
        groups
            .entry(key)
            .or_insert_with(|| (sample.location.clone(), Vec::new()))
            .1
            .push(sample);
    }
    let mut entries = Vec::with_capacity(groups.len());
    for ((day, loc_name), (location, rows)) in groups {
        let max_price = rows
            .iter()
            .map(|r| r.lmp_usd_per_mwh)
            .fold(f64::NEG_INFINITY, f64::max);
        let n = rows.len() as u64;
        let provenance = rows[0].provenance;
        let rel = ingest::write_price_partition(out, signal, day, &location, &rows, None)?;
        eprintln!("wrote {rel} ({n} rows, max ${max_price:.2}/MWh)");
        entries.push(ManifestEntry {
            signal: signal.to_string(),
            date: fmt_date(day),
            location: loc_name,
            path: rel,
            rows: n,
            provenance,
        });
    }
    Ok(entries)
}

/// Group AS prices by operating day and write one partition each.
fn write_as_groups(rows: Vec<AsPrice>, out: &Path) -> Result<Vec<ManifestEntry>> {
    let mut groups: BTreeMap<
        Date,
        Vec<(OffsetDateTime, batsim_ercot::AsProduct, f64, Provenance)>,
    > = BTreeMap::new();
    for row in rows {
        groups
            .entry(batsim_ercot::cpt::operating_day(row.ts))
            .or_default()
            .push((row.ts, row.product, row.mcpc_usd_per_mw, row.provenance));
    }
    let mut entries = Vec::with_capacity(groups.len());
    for (day, day_rows) in groups {
        let n = day_rows.len() as u64;
        let provenance = day_rows[0].3;
        let rel = ingest::write_as_partition(out, day, &day_rows)?;
        eprintln!("wrote {rel} ({n} rows)");
        entries.push(ManifestEntry {
            signal: batsim_ercot::schema::SIGNAL_AS_MCPC.to_string(),
            date: fmt_date(day),
            location: ingest::ALL_LOCATION.to_string(),
            path: rel,
            rows: n,
            provenance,
        });
    }
    Ok(entries)
}

// ---------------------------------------------------------------------------
// verify
// ---------------------------------------------------------------------------

struct VerifyRow {
    signal: String,
    date: String,
    location: String,
    rows_expected: u64,
    detail: std::result::Result<String, String>,
}

fn run_verify(root: &Path) -> Result<()> {
    let manifest = ingest::read_manifest(root)?
        .ok_or_else(|| ErcotError::InvalidParam(format!("{}: no manifest.json", root.display())))?;
    eprintln!(
        "verify: {} entries, schema v{}, rules {}, ingested {}",
        manifest.entries.len(),
        manifest.schema_version,
        manifest.rules_version,
        manifest.ingested_at
    );
    let mut rows_out = Vec::with_capacity(manifest.entries.len());
    for entry in &manifest.entries {
        if !SIGNALS.contains(&entry.signal.as_str()) {
            rows_out.push(VerifyRow {
                signal: entry.signal.clone(),
                date: entry.date.clone(),
                location: entry.location.clone(),
                rows_expected: entry.rows,
                detail: Err(format!("unknown signal {:?}", entry.signal)),
            });
            continue;
        }
        let detail = verify_partition(&root.join(&entry.path), entry.rows);
        rows_out.push(VerifyRow {
            signal: entry.signal.clone(),
            date: entry.date.clone(),
            location: entry.location.clone(),
            rows_expected: entry.rows,
            detail,
        });
    }
    println!(
        "{:<12} {:<12} {:<12} {:>8}  CHECK",
        "SIGNAL", "DATE", "LOCATION", "ROWS"
    );
    for row in &rows_out {
        let check = match &row.detail {
            Ok(note) => format!("ok ({note})"),
            Err(e) => format!("FAIL: {e}"),
        };
        println!(
            "{:<12} {:<12} {:<12} {:>8}  {}",
            row.signal, row.date, row.location, row.rows_expected, check
        );
    }
    let failed = rows_out.iter().filter(|r| r.detail.is_err()).count();
    println!(
        "{} partition(s): {} ok, {} failed",
        rows_out.len(),
        rows_out.len() - failed,
        failed
    );
    if failed > 0 {
        return Err(ErcotError::InvalidParam(format!(
            "verification failed for {failed} partition(s)"
        )));
    }
    Ok(())
}

/// Check one partition: schema version, row count, strictly increasing ts.
fn verify_partition(path: &Path, rows_expected: u64) -> std::result::Result<String, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open: {e}"))?;
    let builder =
        ParquetRecordBatchReaderBuilder::try_new(file).map_err(|e| format!("parquet: {e}"))?;
    let metadata = builder.metadata().file_metadata().clone();
    let version = metadata
        .key_value_metadata()
        .and_then(|kv| {
            kv.iter()
                .find(|k| k.key == batsim_ercot::schema::SCHEMA_VERSION_KEY)
                .and_then(|k| k.value.clone())
        })
        .ok_or_else(|| "missing batsim.schema_version".to_string())?;
    if version != batsim_ercot::schema::SCHEMA_VERSION.to_string() {
        return Err(format!("schema version {version}"));
    }
    let product_idx = batch_column_index(&builder, batsim_ercot::schema::as_cols::PRODUCT);
    let mut count: u64 = 0;
    // Price/load partitions: ts strictly increasing. AS partitions hold one
    // row per (ts, product), so the pair must be strictly increasing.
    let mut prev_ts: Option<i64> = None;
    let mut prev_pair: Option<(i64, u8)> = None;
    for batch in builder.build().map_err(|e| format!("reader: {e}"))? {
        let batch = batch.map_err(|e| format!("batch: {e}"))?;
        let ts = batch
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .ok_or_else(|| "interval_start_utc not Int64".to_string())?;
        let products = product_idx
            .map(|i| {
                batch
                    .column(i)
                    .as_any()
                    .downcast_ref::<arrow::array::StringArray>()
                    .ok_or_else(|| "product not Utf8".to_string())
            })
            .transpose()?;
        for i in 0..ts.len() {
            let v = ts.value(i);
            if let Some(rank_for) = products {
                let rank = product_rank(rank_for.value(i))?;
                if prev_pair.is_some_and(|p| (v, rank) <= p) {
                    return Err(format!(
                        "(ts, product) not strictly increasing at row {count}"
                    ));
                }
                prev_pair = Some((v, rank));
            } else {
                if prev_ts.is_some_and(|p| v <= p) {
                    return Err(format!("timestamps not strictly increasing at row {count}"));
                }
                prev_ts = Some(v);
            }
            count += 1;
        }
    }
    if count != rows_expected {
        return Err(format!("row count {count} != manifest {rows_expected}"));
    }
    Ok(format!("v{version}"))
}

/// Column index by name in the parquet file's projected schema, if present.
fn batch_column_index(
    builder: &ParquetRecordBatchReaderBuilder<std::fs::File>,
    name: &str,
) -> Option<usize> {
    builder
        .schema()
        .fields()
        .iter()
        .position(|f| f.name() == name)
}

/// `AsProduct` declaration-order rank (writer sorts `(ts, product)` by it).
fn product_rank(name: &str) -> std::result::Result<u8, String> {
    batsim_ercot::AsProduct::ALL
        .iter()
        .position(|p| p.dam_column() == name)
        .and_then(|i| u8::try_from(i).ok())
        .ok_or_else(|| format!("unknown AS product {name:?}"))
}

// ---------------------------------------------------------------------------
// synth
// ---------------------------------------------------------------------------

fn run_synth(out: &Path, date: &str, location: &str, seed: u64, interval_secs: u32) -> Result<()> {
    let day = Date::parse(date, &format_description!("[year]-[month]-[day]"))
        .map_err(|e| ErcotError::InvalidParam(format!("--date: {e}")))?;
    let location = Location::from_settlement_point(location);
    if interval_secs < 60 || 3600 % interval_secs != 0 {
        return Err(ErcotError::InvalidParam(format!(
            "--interval-secs {interval_secs} must divide 3600 and be >= 60"
        )));
    }
    let season = match day.month() {
        time::Month::December | time::Month::January | time::Month::February => Season::Winter,
        time::Month::June | time::Month::July | time::Month::August | time::Month::September => {
            Season::Summer
        }
        _ => Season::Shoulder,
    };
    // Operating day [00:00 CPT, 24:00 CPT) as a UTC range (25 h on fall-back).
    let next = day
        .next_day()
        .ok_or_else(|| ErcotError::Time("date overflow".to_string()))?;
    let range = TimeRange::new(
        cpt::cpt_interval_to_utc(day, 1, 1, 1, false)?,
        cpt::cpt_interval_to_utc(next, 1, 1, 1, false)?,
    )?;
    let rules = ErcotRules::current()?;
    let params = SyntheticParams {
        seed,
        season,
        interval_secs,
        location: location.clone(),
        ..Default::default()
    };
    let generator = SyntheticPriceGenerator::new(params, range, &rules)?;
    let mut rt_rows = generator.rt_spps(&location, range)?;
    let mut dam_rows = generator.dam_spps(&location, range)?;
    let mut as_rows = generator.as_prices(range)?;
    for row in rt_rows.iter_mut().chain(dam_rows.iter_mut()) {
        row.provenance = Provenance::Synthetic;
    }
    for row in &mut as_rows {
        row.provenance = Provenance::Synthetic;
    }
    let mut entries = write_price_groups(SIGNAL_RTM_SPP, rt_rows, out)?;
    entries.extend(write_price_groups(SIGNAL_DAM_SPP, dam_rows, out)?);
    entries.extend(write_as_groups(as_rows, out)?);
    let meta = ingest::ManifestMeta {
        rules_version: rules.meta.protocol_version,
        source_report: "synthetic".to_string(),
        source_doc_ids: Vec::new(),
        ingested_at: now_rfc3339(),
    };
    let manifest = ingest::upsert_manifest(out, &meta, &entries)?;
    eprintln!("manifest: {} entries total", manifest.entries.len());
    Ok(())
}

// ---------------------------------------------------------------------------
// small helpers
// ---------------------------------------------------------------------------

fn fmt_date(date: Date) -> String {
    date.format(&format_description!("[year]-[month]-[day]"))
        .unwrap_or_else(|_| format!("{date:?}"))
}

fn now_rfc3339() -> String {
    OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_else(|_| "unknown".to_string())
}
