//! ERCOT report parsers (spec D.3.1 / D.3.3).
//!
//! Supported inputs (auto-detected by extension or magic bytes):
//! - `.xlsx` yearly historical reports via `calamine` (monthly sheets;
//!   report 13061 RTM LZ/HB, 13060 DAM LZ/HB, 13091 DAM AS MCPC).
//! - `.csv` current-format reports.
//! - `.zip` containing CSV(s).
//!
//! All parsers convert CPT hour-ending report rows to UTC interval-start
//! samples via [`crate::cpt`], preserving the fall-back repeated hour
//! (25-hour day). Parsed rows carry [`Provenance::SettlementFinal`]; the
//! ORDC/RDPA adder split is not available from these reports, so adder
//! columns are `0.0` (documented in the crate README).
//!
//! ERCOT's historical RTM file duplicates every load-zone row verbatim;
//! parsers deduplicate on `(ts, location)` / `(ts, product)` keeping the
//! first occurrence and count the drops in [`ParseStats`].

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Cursor, Read};

use calamine::Reader as _;
use time::{Date, Month};

use crate::cpt::cpt_interval_to_utc;
use crate::error::{ErcotError, Result};
use crate::types::{AsPrice, AsProduct, Location, PriceSample, Provenance};

/// Ingested report family.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportKind {
    /// Historical RTM load-zone / hub SPPs (report 13061, 15-min).
    RtmSpp,
    /// Historical DAM load-zone / hub SPPs (report 13060, hourly).
    DamSpp,
    /// Historical DAM AS clearing prices for capacity (report 13091, hourly).
    AsMcpc,
}

impl ReportKind {
    /// ERCOT MIS report type ID (verified 2026-07-27; see crate README).
    #[must_use]
    pub const fn report_type_id(self) -> u32 {
        match self {
            Self::RtmSpp => 13061,
            Self::DamSpp => 13060,
            Self::AsMcpc => 13091,
        }
    }

    /// Canonical signal name (archive directory).
    #[must_use]
    pub const fn signal(self) -> &'static str {
        match self {
            Self::RtmSpp => crate::schema::SIGNAL_RTM_SPP,
            Self::DamSpp => crate::schema::SIGNAL_DAM_SPP,
            Self::AsMcpc => crate::schema::SIGNAL_AS_MCPC,
        }
    }

    /// CLI name (`rtm-spp` / `dam-spp` / `as-mcpc`).
    #[must_use]
    pub const fn cli_name(self) -> &'static str {
        match self {
            Self::RtmSpp => "rtm-spp",
            Self::DamSpp => "dam-spp",
            Self::AsMcpc => "as-mcpc",
        }
    }
}

impl std::str::FromStr for ReportKind {
    type Err = ErcotError;

    fn from_str(s: &str) -> Result<Self> {
        match s {
            "rtm-spp" => Ok(Self::RtmSpp),
            "dam-spp" => Ok(Self::DamSpp),
            "as-mcpc" => Ok(Self::AsMcpc),
            other => Err(ErcotError::InvalidParam(format!(
                "unknown report kind {other:?} (expected rtm-spp|dam-spp|as-mcpc)"
            ))),
        }
    }
}

impl std::fmt::Display for ReportKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.cli_name())
    }
}

/// On-disk report container format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportFormat {
    /// Excel workbook (yearly historical reports).
    Xlsx,
    /// Plain CSV text.
    Csv,
    /// Zip archive containing CSV file(s) or an XLSX workbook (ERCOT serves
    /// historical workbooks zipped).
    CsvZip,
}

impl ReportFormat {
    /// Detect from a file extension (`.xlsx` / `.csv` / `.zip`).
    #[must_use]
    pub fn from_path(path: &std::path::Path) -> Option<Self> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "xlsx" | "xlsm" => Some(Self::Xlsx),
            "csv" => Some(Self::Csv),
            "zip" => Some(Self::CsvZip),
            _ => None,
        }
    }

    /// Detect from magic bytes. XLSX and zip-of-CSV share the `PK` magic;
    /// the archive is disambiguated by looking for an `xl/` member.
    #[must_use]
    pub fn sniff(bytes: &[u8]) -> Self {
        if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
            if zip_contains_xlsx(bytes) {
                Self::Xlsx
            } else {
                Self::CsvZip
            }
        } else {
            Self::Csv
        }
    }
}

/// Parse outcome statistics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ParseStats {
    /// Data rows read from the source (before dedup).
    pub rows_read: u64,
    /// Rows dropped as exact duplicates (ERCOT duplicates load-zone rows).
    pub duplicates_skipped: u64,
    /// Rows/cells skipped because a price cell was empty.
    pub empty_skipped: u64,
}

/// Parsed report payload plus statistics.
#[derive(Debug)]
pub struct ParsedReport<T> {
    /// Normalized rows, sorted by timestamp.
    pub rows: T,
    /// Parse statistics.
    pub stats: ParseStats,
}

/// Parse an RTM or DAM SPP report into normalized price samples.
///
/// `interval_secs` is inferred from the maximum `Delivery Interval` value
/// found in the file (`4 → 900 s`, `1 → 3600 s`); per ERCOT's historical
/// files this is uniform across the report.
///
/// # Errors
/// - `Parse`: malformed container, missing columns, or bad cell values.
/// - `Time`: a row falls in the spring-forward CPT gap or is out of range.
pub fn parse_spp_report(
    kind: ReportKind,
    bytes: &[u8],
    format: ReportFormat,
) -> Result<ParsedReport<Vec<PriceSample>>> {
    if kind == ReportKind::AsMcpc {
        return Err(ErcotError::InvalidParam(
            "parse_spp_report: use parse_as_report for as-mcpc".to_string(),
        ));
    }
    let context = format!("{} report ({format:?})", kind.cli_name());
    let tables = read_tables(bytes, format, &context)?;
    let mut stats = ParseStats::default();
    let mut raw: Vec<SppRow> = Vec::new();
    for table in &tables {
        let cols = SppColumns::find(&table.headers, &context)?;
        for cells in &table.rows {
            if cells.iter().all(Cell::is_empty) {
                continue;
            }
            stats.rows_read += 1;
            if let Some(row) = read_spp_row(&cols, cells, &context, &mut stats)? {
                raw.push(row);
            }
        }
    }
    if raw.is_empty() {
        return Err(ErcotError::Parse {
            context,
            detail: "no data rows found".to_string(),
        });
    }
    let intervals_per_hour = raw.iter().map(|r| r.interval).max().unwrap_or(1);
    if !matches!(intervals_per_hour, 1 | 4 | 12) {
        return Err(ErcotError::Parse {
            context,
            detail: format!(
                "unsupported Delivery Interval cadence: {intervals_per_hour} intervals/hour (expected 1, 4, or 12)"
            ),
        });
    }
    let interval_secs = u32::from(60 / intervals_per_hour) * 60;
    let mut fallback_coverage: BTreeMap<Date, BTreeSet<(u8, u8, bool)>> = BTreeMap::new();
    for row in &raw {
        if crate::cpt::is_fall_back_day(row.date) {
            fallback_coverage.entry(row.date).or_default().insert((
                row.hour_ending,
                row.interval,
                row.repeated_hour,
            ));
        }
    }
    for (date, covered) in &fallback_coverage {
        let expected = 25 * usize::from(intervals_per_hour);
        if covered.len() != expected {
            return Err(ErcotError::Parse {
                context: context.clone(),
                detail: format!(
                    "fall-back operating day {date} covers {} of {expected} intervals (repeated-hour rows missing or flag values empty)",
                    covered.len()
                ),
            });
        }
    }
    let mut seen: BTreeMap<(i64, String), PriceSample> = BTreeMap::new();
    for row in raw {
        let ts = cpt_interval_to_utc(
            row.date,
            row.hour_ending,
            row.interval,
            intervals_per_hour,
            row.repeated_hour,
        )?;
        let key = (ts.unix_timestamp(), row.name.clone());
        let sample = PriceSample {
            ts,
            interval_secs,
            location: Location::from_settlement_point(&row.name),
            lmp_usd_per_mwh: row.price,
            // Adder split unavailable from these reports (see module docs).
            ordc_adder_usd_per_mwh: 0.0,
            rdpa_adder_usd_per_mwh: 0.0,
            provenance: Provenance::SettlementFinal,
        };
        match seen.entry(key) {
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert(sample);
            }
            std::collections::btree_map::Entry::Occupied(_) => {
                stats.duplicates_skipped += 1;
            }
        }
    }
    Ok(ParsedReport {
        rows: seen.into_values().collect(),
        stats,
    })
}

/// Parse a DAM AS clearing-price (MCPC) report into hourly AS prices.
///
/// Product columns are matched tolerantly: a header whose normalized form
/// contains the product name (`REGUP`, `REGDN`, `RRS`, `NSPIN`, `ECRS`) is
/// accepted, with headers also containing `MCPC` preferred. Products absent
/// from the file (e.g. ECRS before 2023-06) are skipped.
///
/// # Errors
/// - `Parse`: malformed container, missing date/hour columns, or bad cells.
/// - `Time`: a row falls in the spring-forward CPT gap or is out of range.
pub fn parse_as_report(bytes: &[u8], format: ReportFormat) -> Result<ParsedReport<Vec<AsPrice>>> {
    let context = format!("as-mcpc report ({format:?})");
    let tables = read_tables(bytes, format, &context)?;
    let mut stats = ParseStats::default();
    let mut seen: BTreeMap<(i64, AsProduct), AsPrice> = BTreeMap::new();
    let mut fallback_hours: BTreeMap<Date, BTreeSet<(u8, bool)>> = BTreeMap::new();
    for table in &tables {
        let cols = AsColumns::find(&table.headers, &context)?;
        for cells in &table.rows {
            if cells.iter().all(Cell::is_empty) {
                continue;
            }
            stats.rows_read += 1;
            read_as_row(
                &cols,
                cells,
                &context,
                &mut stats,
                &mut seen,
                &mut fallback_hours,
            )?;
        }
    }
    if seen.is_empty() {
        return Err(ErcotError::Parse {
            context,
            detail: "no data rows found".to_string(),
        });
    }
    for (date, hours) in &fallback_hours {
        if hours.len() != 25 {
            return Err(ErcotError::Parse {
                context: context.clone(),
                detail: format!(
                    "fall-back operating day {date} covers {} of 25 hours (repeated-hour rows missing or flag values empty)",
                    hours.len()
                ),
            });
        }
    }
    Ok(ParsedReport {
        rows: seen.into_values().collect(),
        stats,
    })
}

// ---------------------------------------------------------------------------
// Raw row extraction
// ---------------------------------------------------------------------------

/// A raw SPP report row (pre time-conversion).
struct SppRow {
    date: Date,
    hour_ending: u8,
    interval: u8,
    repeated_hour: bool,
    name: String,
    price: f64,
}

struct SppColumns {
    date: usize,
    hour: usize,
    interval: Option<usize>,
    flag: Option<usize>,
    name: usize,
    price: usize,
}

impl SppColumns {
    fn find(headers: &[String], context: &str) -> Result<Self> {
        Ok(Self {
            date: find_column(headers, &["DELIVERYDATE"], &["DATE"])
                .ok_or_else(|| missing(context, "Delivery Date"))?,
            hour: find_column(headers, &["DELIVERYHOUR", "HOURENDING"], &[])
                .ok_or_else(|| missing(context, "Delivery Hour / Hour Ending"))?,
            // Hourly reports (13060) have no interval column; implied 1.
            interval: find_column(headers, &["DELIVERYINTERVAL"], &[]),
            flag: find_column(headers, &["REPEATEDHOURFLAG", "DSTFLAG"], &[]),
            name: find_column(headers, &["SETTLEMENTPOINTNAME", "SETTLEMENTPOINT"], &[])
                .ok_or_else(|| missing(context, "Settlement Point Name"))?,
            price: find_column(headers, &["SETTLEMENTPOINTPRICE"], &["PRICE"])
                .ok_or_else(|| missing(context, "Settlement Point Price"))?,
        })
    }
}

fn read_spp_row(
    cols: &SppColumns,
    cells: &[Cell],
    context: &str,
    stats: &mut ParseStats,
) -> Result<Option<SppRow>> {
    let price = match cell_at(cells, cols.price) {
        cell if cell.is_empty() => {
            stats.empty_skipped += 1;
            return Ok(None);
        }
        cell => cell_f64(cell, context, "Settlement Point Price")?,
    };
    let name = cell_string(cell_at(cells, cols.name), context, "Settlement Point Name")?;
    if name.is_empty() {
        stats.empty_skipped += 1;
        return Ok(None);
    }
    let date = cell_date(cell_at(cells, cols.date), context)?;
    if cols.flag.is_none() && crate::cpt::is_fall_back_day(date) {
        return Err(ErcotError::Parse {
            context: context.to_string(),
            detail: "fall-back operating day requires a Repeated Hour Flag column".to_string(),
        });
    }
    Ok(Some(SppRow {
        date,
        hour_ending: cell_hour_ending(cell_at(cells, cols.hour), context)?,
        interval: match cols.interval {
            Some(i) => cell_u8(cell_at(cells, i), context, "Delivery Interval")?,
            None => 1,
        },
        repeated_hour: cols.flag.is_some_and(|i| cell_flag(cell_at(cells, i))),
        name,
        price,
    }))
}

struct AsColumns {
    date: usize,
    hour: usize,
    flag: Option<usize>,
    products: Vec<(usize, AsProduct)>,
}

impl AsColumns {
    fn find(headers: &[String], context: &str) -> Result<Self> {
        let date = find_column(headers, &["DELIVERYDATE"], &["DATE"])
            .ok_or_else(|| missing(context, "Delivery Date"))?;
        let hour = find_column(headers, &["HOURENDING", "DELIVERYHOUR"], &[])
            .ok_or_else(|| missing(context, "Hour Ending"))?;
        let flag = find_column(headers, &["REPEATEDHOURFLAG", "DSTFLAG"], &[]);
        let mut products: Vec<(usize, AsProduct)> = Vec::new();
        for product in AsProduct::ALL {
            if let Some(idx) = find_product_column(headers, product.dam_column()) {
                products.push((idx, product));
            }
        }
        if products.is_empty() {
            return Err(missing(context, "any AS product MCPC column"));
        }
        Ok(Self {
            date,
            hour,
            flag,
            products,
        })
    }
}

fn read_as_row(
    cols: &AsColumns,
    cells: &[Cell],
    context: &str,
    stats: &mut ParseStats,
    seen: &mut BTreeMap<(i64, AsProduct), AsPrice>,
    fallback_hours: &mut BTreeMap<Date, BTreeSet<(u8, bool)>>,
) -> Result<()> {
    let date = cell_date(cell_at(cells, cols.date), context)?;
    if cols.flag.is_none() && crate::cpt::is_fall_back_day(date) {
        return Err(ErcotError::Parse {
            context: context.to_string(),
            detail: "fall-back operating day requires a Repeated Hour Flag column".to_string(),
        });
    }
    let hour = cell_hour_ending(cell_at(cells, cols.hour), context)?;
    let repeated = cols.flag.is_some_and(|i| cell_flag(cell_at(cells, i)));
    let ts = cpt_interval_to_utc(date, hour, 1, 1, repeated)?;
    let mut recorded = false;
    for (idx, product) in &cols.products {
        let price = match cell_at(cells, *idx) {
            cell if cell.is_empty() => {
                stats.empty_skipped += 1;
                continue;
            }
            cell => cell_f64(cell, context, product.dam_column())?,
        };
        recorded = true;
        let row = AsPrice {
            ts,
            product: *product,
            mcpc_usd_per_mw: price,
            provenance: Provenance::SettlementFinal,
        };
        match seen.entry((ts.unix_timestamp(), *product)) {
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert(row);
            }
            std::collections::btree_map::Entry::Occupied(_) => {
                stats.duplicates_skipped += 1;
            }
        }
    }
    if recorded && crate::cpt::is_fall_back_day(date) {
        fallback_hours
            .entry(date)
            .or_default()
            .insert((hour, repeated));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Table reading (xlsx / csv / zip)
// ---------------------------------------------------------------------------

/// One normalized sheet: header row + data rows of typed cells.
struct Table {
    headers: Vec<String>,
    rows: Vec<Vec<Cell>>,
}

/// A typed table cell.
#[derive(Debug, Clone, PartialEq)]
enum Cell {
    Empty,
    Num(f64),
    Text(String),
}

impl Cell {
    fn is_empty(&self) -> bool {
        matches!(self, Self::Empty) || matches!(self, Self::Text(s) if s.trim().is_empty())
    }
}

fn cell_at(cells: &[Cell], idx: usize) -> &Cell {
    cells.get(idx).unwrap_or(&Cell::Empty)
}

fn read_tables(bytes: &[u8], format: ReportFormat, context: &str) -> Result<Vec<Table>> {
    match format {
        ReportFormat::Xlsx => read_xlsx(bytes, context),
        ReportFormat::Csv => Ok(vec![table_from_csv_bytes(bytes, context)?]),
        ReportFormat::CsvZip => read_csv_zip(bytes, context),
    }
}

fn read_xlsx(bytes: &[u8], context: &str) -> Result<Vec<Table>> {
    let mut workbook: calamine::Xlsx<_> = calamine::open_workbook_from_rs(Cursor::new(bytes))
        .map_err(|e: calamine::XlsxError| ErcotError::Parse {
            context: context.to_string(),
            detail: format!("xlsx open: {e}"),
        })?;
    let mut tables = Vec::new();
    for sheet in workbook.sheet_names().clone() {
        let range = workbook
            .worksheet_range(&sheet)
            .map_err(|e| ErcotError::Parse {
                context: format!("{context} sheet {sheet}"),
                detail: e.to_string(),
            })?;
        tables.push(table_from_range(&range, context)?);
    }
    Ok(tables)
}

fn table_from_range(range: &calamine::Range<calamine::Data>, context: &str) -> Result<Table> {
    let mut rows = range.rows();
    let header_row = rows.next().ok_or_else(|| ErcotError::Parse {
        context: context.to_string(),
        detail: "sheet has no header row".to_string(),
    })?;
    let headers: Vec<String> = header_row.iter().map(data_text).collect();
    let mut out = Vec::with_capacity(range.height().saturating_sub(1));
    for row in rows {
        out.push(
            row.iter()
                .map(|d| data_cell(d, context))
                .collect::<Result<Vec<_>>>()?,
        );
    }
    Ok(Table { headers, rows: out })
}

fn data_cell(data: &calamine::Data, context: &str) -> Result<Cell> {
    Ok(match data {
        calamine::Data::Empty => Cell::Empty,
        calamine::Data::Int(v) => Cell::Num(*v as f64),
        calamine::Data::Float(v) => Cell::Num(*v),
        calamine::Data::String(s) => Cell::Text(s.trim().to_string()),
        calamine::Data::Bool(b) => Cell::Text(b.to_string()),
        calamine::Data::DateTime(dt) => Cell::Num(dt.as_f64()),
        calamine::Data::DateTimeIso(s) | calamine::Data::DurationIso(s) => Cell::Text(s.clone()),
        calamine::Data::Error(e) => {
            return Err(ErcotError::Parse {
                context: context.to_string(),
                detail: format!("cell error value: {e}"),
            });
        }
    })
}

fn data_text(data: &calamine::Data) -> String {
    match data {
        calamine::Data::String(s) => s.trim().to_string(),
        calamine::Data::Int(v) => v.to_string(),
        calamine::Data::Float(v) => v.to_string(),
        _ => String::new(),
    }
}

/// Minimal RFC-4180 CSV reader (quotes, escaped quotes, CRLF, BOM).
fn table_from_csv_bytes(bytes: &[u8], context: &str) -> Result<Table> {
    let text = std::str::from_utf8(bytes).map_err(|e| ErcotError::Parse {
        context: context.to_string(),
        detail: format!("not UTF-8: {e}"),
    })?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let records = parse_csv_records(text, context)?;
    let mut iter = records.into_iter();
    let headers = iter.next().ok_or_else(|| ErcotError::Parse {
        context: context.to_string(),
        detail: "CSV has no header row".to_string(),
    })?;
    let headers: Vec<String> = headers.iter().map(|h| h.trim().to_string()).collect();
    let rows = iter
        .map(|r| r.into_iter().map(Cell::Text).collect())
        .collect();
    Ok(Table { headers, rows })
}

fn parse_csv_records(text: &str, context: &str) -> Result<Vec<Vec<String>>> {
    let mut records: Vec<Vec<String>> = Vec::new();
    let mut record: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if in_quotes {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    field.push('"');
                    chars.next();
                }
                '"' => in_quotes = false,
                _ => field.push(c),
            }
        } else {
            match c {
                '"' if field.is_empty() => in_quotes = true,
                ',' => {
                    record.push(std::mem::take(&mut field));
                }
                '\n' | '\r' => {
                    if c == '\r' && chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    record.push(std::mem::take(&mut field));
                    if record.len() > 1 || !record[0].trim().is_empty() {
                        records.push(std::mem::take(&mut record));
                    } else {
                        record.clear();
                    }
                }
                _ => field.push(c),
            }
        }
    }
    if in_quotes {
        return Err(ErcotError::Parse {
            context: context.to_string(),
            detail: "unterminated quoted CSV field".to_string(),
        });
    }
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        records.push(record);
    }
    Ok(records)
}

/// Decompressed-size caps for zip entries (decompression-bomb guard; a
/// yearly XLSX/CSV report is O(100 MiB), far below these limits).
const MAX_ZIP_ENTRY_BYTES: u64 = 1 << 30;
const MAX_ZIP_TOTAL_BYTES: u64 = 4 << 30;

fn read_csv_zip(bytes: &[u8], context: &str) -> Result<Vec<Table>> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| ErcotError::Parse {
        context: context.to_string(),
        detail: format!("zip open: {e}"),
    })?;
    let mut tables = Vec::new();
    let mut total_bytes: u64 = 0;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| ErcotError::Parse {
            context: context.to_string(),
            detail: format!("zip entry {i}: {e}"),
        })?;
        let entry_name = entry.name().to_string();
        let ext = entry_name
            .rsplit('.')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let is_csv = ext == "csv";
        let is_xlsx = ext == "xlsx" || ext == "xlsm";
        if !is_csv && !is_xlsx {
            continue;
        }
        let entry_context = format!("{context} entry {entry_name}");
        let mut buf = Vec::new();
        entry
            .by_ref()
            .take(MAX_ZIP_ENTRY_BYTES + 1)
            .read_to_end(&mut buf)?;
        if buf.len() as u64 > MAX_ZIP_ENTRY_BYTES {
            return Err(ErcotError::Parse {
                context: entry_context,
                detail: format!("decompressed entry exceeds {MAX_ZIP_ENTRY_BYTES} bytes"),
            });
        }
        total_bytes += buf.len() as u64;
        if total_bytes > MAX_ZIP_TOTAL_BYTES {
            return Err(ErcotError::Parse {
                context: entry_context,
                detail: format!("decompressed archive exceeds {MAX_ZIP_TOTAL_BYTES} bytes"),
            });
        }
        if is_csv {
            tables.push(table_from_csv_bytes(&buf, &entry_context)?);
        } else {
            tables.extend(read_xlsx(&buf, &entry_context)?);
        }
    }
    if tables.is_empty() {
        return Err(ErcotError::Parse {
            context: context.to_string(),
            detail: "zip contains no .csv/.xlsx entry".to_string(),
        });
    }
    Ok(tables)
}

fn zip_contains_xlsx(bytes: &[u8]) -> bool {
    let Ok(mut archive) = zip::ZipArchive::new(Cursor::new(bytes)) else {
        return false;
    };
    for i in 0..archive.len() {
        if let Ok(entry) = archive.by_index(i) {
            if entry.name().starts_with("xl/") {
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Header matching + cell conversion
// ---------------------------------------------------------------------------

/// Normalize a header for matching: uppercase, alphanumeric only.
fn norm_header(header: &str) -> String {
    header
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// Find a column by exact normalized names first, then by substring.
fn find_column(headers: &[String], exact: &[&str], contains: &[&str]) -> Option<usize> {
    let normalized: Vec<String> = headers.iter().map(|h| norm_header(h)).collect();
    for want in exact {
        if let Some(i) = normalized.iter().position(|h| h == want) {
            return Some(i);
        }
    }
    for want in contains {
        if let Some(i) = normalized.iter().position(|h| h.contains(want)) {
            return Some(i);
        }
    }
    None
}

/// Tolerant AS product column match: header contains the product name;
/// headers also containing `MCPC` are preferred.
fn find_product_column(headers: &[String], product: &str) -> Option<usize> {
    let normalized: Vec<String> = headers.iter().map(|h| norm_header(h)).collect();
    let mut fallback = None;
    for (i, h) in normalized.iter().enumerate() {
        if h.contains(product) {
            if h.contains("MCPC") {
                return Some(i);
            }
            if fallback.is_none() {
                fallback = Some(i);
            }
        }
    }
    fallback
}

fn missing(context: &str, column: &str) -> ErcotError {
    ErcotError::Parse {
        context: context.to_string(),
        detail: format!("missing required column: {column}"),
    }
}

fn cell_string(cell: &Cell, context: &str, column: &str) -> Result<String> {
    match cell {
        Cell::Text(s) => Ok(s.trim().to_string()),
        Cell::Num(v) => Ok(v.to_string()),
        Cell::Empty => Err(ErcotError::Parse {
            context: context.to_string(),
            detail: format!("empty {column}"),
        }),
    }
}

fn cell_f64(cell: &Cell, context: &str, column: &str) -> Result<f64> {
    let bad = |detail: String| ErcotError::Parse {
        context: context.to_string(),
        detail: format!("bad {column}: {detail}"),
    };
    match cell {
        Cell::Num(v) => Ok(*v),
        Cell::Text(s) => s
            .trim()
            .parse::<f64>()
            .map_err(|e| bad(format!("{s:?} ({e})"))),
        Cell::Empty => Err(bad("empty".to_string())),
    }
}

fn cell_u8(cell: &Cell, context: &str, column: &str) -> Result<u8> {
    let v = cell_f64(cell, context, column)?;
    if (1.0..256.0).contains(&v) && v.fract() == 0.0 {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Ok(v as u8)
    } else {
        Err(ErcotError::Parse {
            context: context.to_string(),
            detail: format!("bad {column}: {v} not an integer in 1..=255"),
        })
    }
}

fn cell_flag(cell: &Cell) -> bool {
    match cell {
        Cell::Text(s) => s.trim_start().starts_with(['Y', 'y']),
        _ => false,
    }
}

/// Parse an ERCOT delivery date: `MM/DD/YYYY` (or 2-digit year), ISO
/// `YYYY-MM-DD` (possibly with a trailing time), or an Excel serial number.
fn cell_date(cell: &Cell, context: &str) -> Result<Date> {
    match cell {
        Cell::Num(serial) => excel_serial_to_date(*serial, context),
        Cell::Text(s) => parse_date_str(s.trim(), context),
        Cell::Empty => Err(ErcotError::Parse {
            context: context.to_string(),
            detail: "empty Delivery Date".to_string(),
        }),
    }
}

fn excel_serial_to_date(serial: f64, context: &str) -> Result<Date> {
    let days = serial.floor();
    if !(1.0..=200_000.0).contains(&days) {
        return Err(ErcotError::Parse {
            context: context.to_string(),
            detail: format!("Excel date serial out of range: {serial}"),
        });
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let days = days as i64;
    let epoch = Date::from_calendar_date(1899, Month::December, 30)
        .map_err(|e| ErcotError::Time(e.to_string()))?;
    epoch
        .checked_add(time::Duration::days(days))
        .ok_or_else(|| ErcotError::Time(format!("Excel date serial overflow: {serial}")))
}

fn parse_date_str(s: &str, context: &str) -> Result<Date> {
    let bad = || ErcotError::Parse {
        context: context.to_string(),
        detail: format!("bad Delivery Date: {s:?}"),
    };
    if s.contains('/') {
        let parts: Vec<&str> = s.split('/').collect();
        if parts.len() != 3 {
            return Err(bad());
        }
        let month: u8 = parts[0].trim().parse().map_err(|_| bad())?;
        let day: u8 = parts[1].trim().parse().map_err(|_| bad())?;
        let mut year: i32 = parts[2].trim().parse().map_err(|_| bad())?;
        if (0..100).contains(&year) {
            year += 2000;
        }
        let month = Month::try_from(month).map_err(|_| bad())?;
        Date::from_calendar_date(year, month, day).map_err(|_| bad())
    } else if s.len() >= 10 && s.as_bytes()[4] == b'-' {
        // ISO YYYY-MM-DD, possibly followed by a time component.
        let (Some(year), Some(month), Some(day)) = (s.get(0..4), s.get(5..7), s.get(8..10)) else {
            return Err(bad());
        };
        let year: i32 = year.parse().map_err(|_| bad())?;
        let month: u8 = month.parse().map_err(|_| bad())?;
        let day: u8 = day.parse().map_err(|_| bad())?;
        let month = Month::try_from(month).map_err(|_| bad())?;
        Date::from_calendar_date(year, month, day).map_err(|_| bad())
    } else {
        Err(bad())
    }
}

/// Parse an ERCOT hour-ending value: integer `1..=24`, `HHMM` (`"2400"`),
/// or `HH:MM` (`"24:00"`); minutes must be zero.
fn cell_hour_ending(cell: &Cell, context: &str) -> Result<u8> {
    match cell {
        Cell::Num(v) => hour_from_f64(*v, context),
        Cell::Text(s) => hour_from_str(s.trim(), context),
        Cell::Empty => Err(ErcotError::Parse {
            context: context.to_string(),
            detail: "empty hour-ending cell".to_string(),
        }),
    }
}

fn hour_from_f64(v: f64, context: &str) -> Result<u8> {
    let bad = || ErcotError::Parse {
        context: context.to_string(),
        detail: format!("hour ending {v} not in 1..=24 (or HHMM with MM=00)"),
    };
    if v.fract() != 0.0 || !(1.0..=2400.0).contains(&v) {
        return Err(bad());
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let n = v as u32;
    if (1..=24).contains(&n) {
        return u8::try_from(n).map_err(|_| bad());
    }
    // Numeric HHMM form (some reports write 100..2400).
    hour_checked(n / 100, n % 100).ok_or_else(bad)
}

fn hour_from_str(s: &str, context: &str) -> Result<u8> {
    let bad = || ErcotError::Parse {
        context: context.to_string(),
        detail: format!("bad hour ending: {s:?}"),
    };
    if let Some((hh, mm)) = s.split_once(':') {
        let hour: u32 = hh.trim().parse().map_err(|_| bad())?;
        let minute: u32 = mm.trim().parse().map_err(|_| bad())?;
        return hour_checked(hour, minute).ok_or_else(bad);
    }
    if s.chars().all(|c| c.is_ascii_digit()) && !s.is_empty() {
        let value: u32 = s.parse().map_err(|_| bad())?;
        if (1..=24).contains(&value) {
            return u8::try_from(value).map_err(|_| bad());
        }
        if s.len() >= 3 {
            // HHMM form ("100".."2400").
            return hour_checked(value / 100, value % 100).ok_or_else(bad);
        }
        return Err(bad());
    }
    Err(bad())
}

fn hour_checked(hour: u32, minute: u32) -> Option<u8> {
    if minute == 0 && (1..=24).contains(&hour) {
        u8::try_from(hour).ok()
    } else {
        None
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)]
mod tests {
    use super::*;
    use time::macros::datetime;

    const RTM_CSV: &str = "\
Delivery Date,Delivery Hour,Delivery Interval,Repeated Hour Flag,Settlement Point Name,Settlement Point Type,Settlement Point Price
08/17/2023,1,1,N,HB_NORTH,HU,22.48
08/17/2023,1,2,N,HB_NORTH,HU,22.71
08/17/2023,1,3,N,HB_NORTH,HU,22.34
08/17/2023,1,4,N,HB_NORTH,HU,21.98
08/17/2023,1,1,N,LZ_NORTH,LZ,23.10
08/17/2023,1,2,N,LZ_NORTH,LZ,23.20
08/17/2023,1,3,N,LZ_NORTH,LZ,23.30
08/17/2023,1,4,N,LZ_NORTH,LZ,5197.60
08/17/2023,1,1,N,LZ_NORTH,LZ,23.10
08/17/2023,1,2,N,LZ_NORTH,LZ,23.20
08/17/2023,1,3,N,LZ_NORTH,LZ,23.30
08/17/2023,1,4,N,LZ_NORTH,LZ,5197.60
08/17/2023,2,1,N,HB_NORTH,HU,25.01
";

    #[test]
    fn rtm_csv_parses_and_dedupes_zone_rows() {
        let out =
            parse_spp_report(ReportKind::RtmSpp, RTM_CSV.as_bytes(), ReportFormat::Csv).unwrap();
        assert_eq!(out.stats.rows_read, 13);
        assert_eq!(out.stats.duplicates_skipped, 4);
        assert_eq!(out.rows.len(), 9);
        for row in &out.rows {
            assert_eq!(row.interval_secs, 900);
            assert_eq!(row.provenance, Provenance::SettlementFinal);
            assert_eq!(row.ordc_adder_usd_per_mwh, 0.0);
            assert_eq!(row.rdpa_adder_usd_per_mwh, 0.0);
        }
        // Sorted by ts; first row is 00:00 CPT = 05:00 UTC (CDT).
        assert_eq!(out.rows[0].ts, datetime!(2023-08-17 05:00 UTC));
        assert_eq!(
            out.rows[0].location,
            Location::from_settlement_point("HB_NORTH")
        );
        // The spiked LZ_NORTH interval 4.
        let spiked = out
            .rows
            .iter()
            .find(|r| r.lmp_usd_per_mwh > 5000.0)
            .unwrap();
        assert_eq!(spiked.lmp_usd_per_mwh, 5197.60);
        assert_eq!(spiked.location, Location::from_settlement_point("LZ_NORTH"));
        assert_eq!(spiked.ts, datetime!(2023-08-17 05:45 UTC));
    }

    #[test]
    fn dam_csv_is_hourly() {
        let mut csv = String::from(
            "Delivery Date,Delivery Hour,Delivery Interval,Settlement Point Name,Settlement Point Type,Settlement Point Price\n",
        );
        for h in 1..=24 {
            csv.push_str(&format!("03/10/2023,{h},1,LZ_WEST,LZ,30.{h:02}\n"));
        }
        let out = parse_spp_report(ReportKind::DamSpp, csv.as_bytes(), ReportFormat::Csv).unwrap();
        assert_eq!(out.rows.len(), 24);
        assert_eq!(out.stats.duplicates_skipped, 0);
        for row in &out.rows {
            assert_eq!(row.interval_secs, 3600);
        }
        // 2023-03-10 is CST: hour ending 1 = 06:00 UTC.
        assert_eq!(out.rows[0].ts, datetime!(2023-03-10 06:00 UTC));
        assert_eq!(out.rows[23].ts, datetime!(2023-03-11 05:00 UTC));
    }

    #[test]
    fn fall_back_day_yields_100_strictly_increasing_intervals() {
        // 2023-11-05: hour ending 2 occurs twice (flag Y on the repeat).
        let mut csv = String::from(
            "Delivery Date,Delivery Hour,Delivery Interval,Repeated Hour Flag,Settlement Point Name,Settlement Point Type,Settlement Point Price\n",
        );
        for h in 1..=24u8 {
            let passes: &[(&str, f64)] = if h == 2 {
                &[("N", 20.0), ("Y", 30.0)]
            } else {
                &[("N", 10.0)]
            };
            for (flag, base) in passes {
                for i in 1..=4 {
                    csv.push_str(&format!(
                        "11/05/2023,{h},{i},{flag},LZ_NORTH,LZ,{}\n",
                        base + f64::from(i)
                    ));
                }
            }
        }
        let out = parse_spp_report(ReportKind::RtmSpp, csv.as_bytes(), ReportFormat::Csv).unwrap();
        assert_eq!(out.rows.len(), 100);
        assert_eq!(out.stats.duplicates_skipped, 0);
        for w in out.rows.windows(2) {
            assert!(w[0].ts < w[1].ts);
            assert_eq!(w[1].ts - w[0].ts, time::Duration::minutes(15));
        }
        // First 01:00 occurrence is CDT (06:00 UTC), the repeat CST (07:00 UTC).
        let first = out.rows.iter().find(|r| r.lmp_usd_per_mwh == 21.0).unwrap();
        let repeat = out.rows.iter().find(|r| r.lmp_usd_per_mwh == 31.0).unwrap();
        assert_eq!(first.ts, datetime!(2023-11-05 06:00 UTC));
        assert_eq!(repeat.ts, datetime!(2023-11-05 07:00 UTC));
    }

    #[test]
    fn fall_back_day_missing_repeated_rows_errors() {
        // Flag column present but no repeated-hour rows: 24 hours, not 25.
        let mut csv = String::from(
            "Delivery Date,Delivery Hour,Delivery Interval,Repeated Hour Flag,Settlement Point Name,Settlement Point Type,Settlement Point Price\n",
        );
        for h in 1..=24u8 {
            for i in 1..=4 {
                csv.push_str(&format!("11/05/2023,{h},{i},N,LZ_NORTH,LZ,10.0\n"));
            }
        }
        let err =
            parse_spp_report(ReportKind::RtmSpp, csv.as_bytes(), ReportFormat::Csv).unwrap_err();
        assert!(matches!(err, ErcotError::Parse { .. }));
        assert!(err.to_string().contains("covers 96 of 100"));
    }

    #[test]
    fn fall_back_day_empty_flag_values_error() {
        // Repeated-hour rows present but flag values empty: second 01:00
        // occurrence collapses onto the first, again 24 distinct hours.
        let mut csv = String::from(
            "Delivery Date,Delivery Hour,Delivery Interval,Repeated Hour Flag,Settlement Point Name,Settlement Point Type,Settlement Point Price\n",
        );
        for h in 1..=24u8 {
            let passes: &[f64] = if h == 2 { &[20.0, 30.0] } else { &[10.0] };
            for base in passes {
                for i in 1..=4 {
                    csv.push_str(&format!("11/05/2023,{h},{i},,LZ_NORTH,LZ,{base}\n"));
                }
            }
        }
        let err =
            parse_spp_report(ReportKind::RtmSpp, csv.as_bytes(), ReportFormat::Csv).unwrap_err();
        assert!(matches!(err, ErcotError::Parse { .. }));
        assert!(err.to_string().contains("covers 96 of 100"));
    }

    #[test]
    fn as_fall_back_day_missing_repeated_hour_errors() {
        // Hourly AS report on the 25-hour day with only 24 distinct hours.
        let mut csv =
            String::from("Delivery Date,Hour Ending,Repeated Hour Flag,REGUP MCPC,RRS MCPC\n");
        for h in 1..=24 {
            csv.push_str(&format!("11/05/2023,{h},N,1.1,2.2\n"));
        }
        let err = parse_as_report(csv.as_bytes(), ReportFormat::Csv).unwrap_err();
        assert!(matches!(err, ErcotError::Parse { .. }));
        assert!(err.to_string().contains("covers 24 of 25 hours"));
    }

    #[test]
    fn as_fall_back_day_with_25_hours_passes() {
        let mut csv =
            String::from("Delivery Date,Hour Ending,Repeated Hour Flag,REGUP MCPC,RRS MCPC\n");
        for h in 1..=24 {
            let passes: &[&str] = if h == 2 { &["N", "Y"] } else { &["N"] };
            for flag in passes {
                csv.push_str(&format!("11/05/2023,{h},{flag},1.1,2.2\n"));
            }
        }
        let out = parse_as_report(csv.as_bytes(), ReportFormat::Csv).unwrap();
        assert_eq!(out.rows.len(), 50);
    }

    #[test]
    fn dam_hourly_layout_without_interval_column() {
        // Real 13060 layout: "Hour Ending", "Settlement Point", no interval.
        let csv = "Delivery Date,Hour Ending,Repeated Hour Flag,Settlement Point,Settlement Point Price\n\
                   08/17/2023,01:00,N,HB_BUSAVG,10.36\n\
                   08/17/2023,02:00,N,HB_BUSAVG,9.99\n";
        let out = parse_spp_report(ReportKind::DamSpp, csv.as_bytes(), ReportFormat::Csv).unwrap();
        assert_eq!(out.rows.len(), 2);
        assert_eq!(out.rows[0].interval_secs, 3600);
        assert_eq!(out.rows[0].ts, datetime!(2023-08-17 05:00 UTC));
        assert_eq!(out.rows[1].ts, datetime!(2023-08-17 06:00 UTC));
        assert_eq!(out.rows[0].lmp_usd_per_mwh, 10.36);
    }

    #[test]
    fn as_csv_tolerant_headers_and_hour_formats() {
        let mut csv = String::from("Delivery Date,Hour Ending,REGUP MCPC,REGDN MCPC,RRS MCPC,NSPIN MCPC,ECRS MCPC,DST Flag\n");
        for h in 1..=24 {
            csv.push_str(&format!("08/17/2023,{h:02}00,1.1,2.2,3.3,4.4,5.5,N\n"));
        }
        let out = parse_as_report(csv.as_bytes(), ReportFormat::Csv).unwrap();
        assert_eq!(out.rows.len(), 24 * 5);
        assert_eq!(out.stats.duplicates_skipped, 0);
        let ecrs5 = out
            .rows
            .iter()
            .find(|r| r.product == AsProduct::Ecrs && r.ts == datetime!(2023-08-17 09:00 UTC))
            .unwrap();
        assert_eq!(ecrs5.mcpc_usd_per_mw, 5.5);
        assert_eq!(ecrs5.provenance, Provenance::SettlementFinal);
        assert!(out.rows.windows(2).all(|w| w[0].ts <= w[1].ts));
    }

    #[test]
    fn as_missing_product_columns_are_skipped() {
        let csv = "Delivery Date,Hour Ending,REGUP,RRS\n08/17/2023,1,9.9,8.8\n";
        let out = parse_as_report(csv.as_bytes(), ReportFormat::Csv).unwrap();
        assert_eq!(out.rows.len(), 2);
        assert!(out.rows.iter().all(|r| r.product != AsProduct::Ecrs));
    }

    #[test]
    fn csv_quoted_fields_and_crlf() {
        let csv = "Delivery Date,Delivery Hour,Delivery Interval,Repeated Hour Flag,Settlement Point Name,Settlement Point Type,Settlement Point Price\r\n\"08/17/2023\",1,1,N,\"LZ_NORTH\",LZ,\"1,234.50\"\r\n";
        let err = parse_spp_report(ReportKind::RtmSpp, csv.as_bytes(), ReportFormat::Csv);
        // Thousands separators are not valid floats: parse error, not a panic.
        assert!(matches!(err, Err(ErcotError::Parse { .. })));
        let csv_ok = csv.replace("\"1,234.50\"", "1234.50");
        let out =
            parse_spp_report(ReportKind::RtmSpp, csv_ok.as_bytes(), ReportFormat::Csv).unwrap();
        assert_eq!(out.rows.len(), 1);
        assert_eq!(out.rows[0].lmp_usd_per_mwh, 1234.50);
    }

    #[test]
    fn zip_of_csv_round_trip() {
        use std::io::Write;
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut cursor);
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            writer.start_file("report.csv", options).unwrap();
            writer.write_all(RTM_CSV.as_bytes()).unwrap();
            writer.finish().unwrap();
        }
        let bytes = cursor.into_inner();
        assert_eq!(ReportFormat::sniff(&bytes), ReportFormat::CsvZip);
        let out = parse_spp_report(ReportKind::RtmSpp, &bytes, ReportFormat::CsvZip).unwrap();
        assert_eq!(out.rows.len(), 9);
        assert_eq!(out.stats.duplicates_skipped, 4);
    }

    #[test]
    fn sniff_distinguishes_xlsx_from_csv_zip() {
        assert_eq!(ReportFormat::sniff(b"Delivery Date,x"), ReportFormat::Csv);
        let mut cursor = Cursor::new(Vec::new());
        {
            use std::io::Write;
            let mut writer = zip::ZipWriter::new(&mut cursor);
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            writer.start_file("xl/workbook.xml", options).unwrap();
            writer.write_all(b"<workbook/>").unwrap();
            writer.finish().unwrap();
        }
        assert_eq!(
            ReportFormat::sniff(&cursor.into_inner()),
            ReportFormat::Xlsx
        );
    }

    #[test]
    fn hour_ending_formats() {
        let ctx = "test";
        assert_eq!(hour_from_str("1", ctx).unwrap(), 1);
        assert_eq!(hour_from_str("24", ctx).unwrap(), 24);
        assert_eq!(hour_from_str("0100", ctx).unwrap(), 1);
        assert_eq!(hour_from_str("2400", ctx).unwrap(), 24);
        assert_eq!(hour_from_str("01:00", ctx).unwrap(), 1);
        assert_eq!(hour_from_str("24:00", ctx).unwrap(), 24);
        assert!(hour_from_str("25", ctx).is_err());
        assert!(hour_from_str("0130", ctx).is_err());
        assert!(hour_from_str("0", ctx).is_err());
        assert!(hour_from_str("0000", ctx).is_err());
        assert_eq!(hour_from_f64(2400.0, ctx).unwrap(), 24);
        assert_eq!(hour_from_f64(100.0, ctx).unwrap(), 1);
        assert!(hour_from_f64(130.0, ctx).is_err());
    }

    #[test]
    fn date_formats() {
        let ctx = "test";
        let want = Date::from_calendar_date(2023, Month::August, 17).unwrap();
        assert_eq!(parse_date_str("08/17/2023", ctx).unwrap(), want);
        assert_eq!(
            parse_date_str("8/7/23", ctx).unwrap(),
            Date::from_calendar_date(2023, Month::August, 7).unwrap()
        );
        assert_eq!(parse_date_str("2023-08-17", ctx).unwrap(), want);
        assert_eq!(parse_date_str("2023-08-17 00:00:00", ctx).unwrap(), want);
        assert!(parse_date_str("17/08/2023", ctx).is_err());
        assert!(parse_date_str("garbage", ctx).is_err());
        // Excel serial for 2023-08-17 is 45155.
        assert_eq!(excel_serial_to_date(45155.0, ctx).unwrap(), want);
    }

    #[test]
    fn missing_column_is_a_parse_error() {
        let csv = "Delivery Date,Foo\n08/17/2023,1\n";
        let err = parse_spp_report(ReportKind::RtmSpp, csv.as_bytes(), ReportFormat::Csv);
        assert!(matches!(err, Err(ErcotError::Parse { .. })));
    }

    #[test]
    fn report_kind_ids_and_cli_names() {
        assert_eq!(ReportKind::RtmSpp.report_type_id(), 13061);
        assert_eq!(ReportKind::DamSpp.report_type_id(), 13060);
        assert_eq!(ReportKind::AsMcpc.report_type_id(), 13091);
        assert_eq!("rtm-spp".parse::<ReportKind>().unwrap(), ReportKind::RtmSpp);
        assert!("nope".parse::<ReportKind>().is_err());
        assert_eq!(ReportKind::AsMcpc.signal(), crate::schema::SIGNAL_AS_MCPC);
    }
}
