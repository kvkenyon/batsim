//! Parquet-archive replay source (spec D.3.2 / D.3.3).
//!
//! [`Replay`] eagerly loads every partition intersecting a requested
//! [`TimeRange`] from a canonical archive (`<root>/<signal>/date=YYYY-MM-DD/
//! location=<LOC>.parquet`) into in-memory indexes, then serves
//! [`PriceSource`] queries from memory — no I/O on any sim-loop path.
//!
//! Indexes are `BTreeMap`-keyed: prices by `(location, ts)` (nested maps,
//! outer key the canonical settlement-point name so tick-loop lookups are
//! allocation-free), AS prices by `(product, ts)`, system signals by `ts`.
//!
//! Coverage policy: partitions missing for some days are tolerated (a
//! signal may legitimately be absent for a stretch; queries over the gap
//! return empty vectors and callers needing gaplessness validate
//! themselves). A requested signal with ZERO rows in the requested range is
//! loud: [`ErcotError::DataNotFound`]. Unknown Parquet schema versions are
//! refused ([`ErcotError::SchemaVersion`]) before any column is read.

use std::collections::BTreeMap;
use std::fs::File;
use std::path::{Path, PathBuf};

use arrow::array::{Array, Float64Array, Int32Array, Int64Array, RecordBatch, StringArray, UInt32Array};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use time::macros::format_description;
use time::{Date, Duration, OffsetDateTime};

use crate::cpt;
use crate::error::{ErcotError, Result};
use crate::schema::{self, as_cols, load_cols, price_cols};
use crate::source::PriceSource;
use crate::types::{AsPrice, AsProduct, Location, PriceSample, Provenance, SystemSignal, TimeRange};

/// In-memory market-signal index shared by [`Replay`] (loaded from Parquet)
/// and [`crate::synthetic::SyntheticPriceGenerator`] (generated eagerly).
///
/// All maps are `BTreeMap`s: deterministic iteration order, no hash-order
/// dependence, `O(log n)` range queries.
#[derive(Debug, Default)]
pub(crate) struct SourceIndex {
    /// RTM SPPs: settlement-point name -> (unix ts -> sample).
    rt: BTreeMap<String, BTreeMap<i64, PriceSample>>,
    /// DAM SPPs: settlement-point name -> (unix ts -> sample).
    dam: BTreeMap<String, BTreeMap<i64, PriceSample>>,
    /// AS MCPCs keyed by (product, unix ts).
    as_: BTreeMap<(AsProduct, i64), AsPrice>,
    /// System signals keyed by unix ts.
    sys: BTreeMap<i64, SystemSignal>,
}

impl SourceIndex {
    /// Insert one RTM sample (last write wins on key collision).
    pub(crate) fn insert_rt(&mut self, sample: PriceSample) {
        self.rt
            .entry(sample.location.settlement_point())
            .or_default()
            .insert(sample.ts.unix_timestamp(), sample);
    }

    /// Insert one DAM sample (last write wins on key collision).
    pub(crate) fn insert_dam(&mut self, sample: PriceSample) {
        self.dam
            .entry(sample.location.settlement_point())
            .or_default()
            .insert(sample.ts.unix_timestamp(), sample);
    }

    /// Insert one AS price (last write wins on key collision).
    pub(crate) fn insert_as(&mut self, price: AsPrice) {
        self.as_.insert((price.product, price.ts.unix_timestamp()), price);
    }

    /// Insert one system signal (last write wins on key collision).
    pub(crate) fn insert_sys(&mut self, signal: SystemSignal) {
        self.sys.insert(signal.ts.unix_timestamp(), signal);
    }

    /// RTM samples for `loc` in `[start, end)`, ordered by ts.
    pub(crate) fn rt_spps(&self, loc: &Location, r: TimeRange) -> Vec<PriceSample> {
        price_query(&self.rt, loc, r)
    }

    /// DAM samples for `loc` in `[start, end)`, ordered by ts.
    pub(crate) fn dam_spps(&self, loc: &Location, r: TimeRange) -> Vec<PriceSample> {
        price_query(&self.dam, loc, r)
    }

    /// The RTM sample whose interval contains `ts`, if any (pure lookup:
    /// latest interval-start `<= ts`, then the interval-length check).
    pub(crate) fn rt_spp_at(&self, loc: &Location, ts: OffsetDateTime) -> Option<&PriceSample> {
        let inner = self.rt.get(loc.settlement_point().as_str())?;
        let (_, sample) = inner.range(..=ts.unix_timestamp()).next_back()?;
        let interval_end = sample.ts.unix_timestamp() + i64::from(sample.interval_secs);
        (ts.unix_timestamp() < interval_end).then_some(sample)
    }

    /// AS prices in `[start, end)`, ordered by `(ts, product)`.
    pub(crate) fn as_prices(&self, r: TimeRange) -> Vec<AsPrice> {
        let start = r.start.unix_timestamp();
        let end = r.end.unix_timestamp();
        let mut out: Vec<AsPrice> = AsProduct::ALL
            .iter()
            .flat_map(|p| {
                self.as_
                    .range((*p, start)..(*p, end))
                    .map(|(_, price)| price.clone())
            })
            .collect();
        out.sort_by_key(|price| (price.ts.unix_timestamp(), price.product));
        out
    }

    /// System signals in `[start, end)`, ordered by ts.
    pub(crate) fn system_signals(&self, r: TimeRange) -> Vec<SystemSignal> {
        self.sys
            .range(r.start.unix_timestamp()..r.end.unix_timestamp())
            .map(|(_, signal)| signal.clone())
            .collect()
    }
}

/// Shared price-table range query over a location-nested index.
fn price_query(
    map: &BTreeMap<String, BTreeMap<i64, PriceSample>>,
    loc: &Location,
    r: TimeRange,
) -> Vec<PriceSample> {
    map.get(loc.settlement_point().as_str()).map_or_else(Vec::new, |inner| {
        inner
            .range(r.start.unix_timestamp()..r.end.unix_timestamp())
            .map(|(_, sample)| sample.clone())
            .collect()
    })
}

/// Which price table a file feeds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PriceKind {
    /// Real-time market (cadence is measured for `interval_secs()`).
    Rt,
    /// Day-ahead market.
    Dam,
}

/// Outcome of loading one price signal: rows kept and cadence histogram.
#[derive(Debug, Default)]
struct PriceLoad {
    /// Rows within the requested range.
    rows: u64,
    /// `interval_secs` -> row count (used to report the RTM cadence).
    cadence: BTreeMap<u32, u64>,
}

/// Parquet-archive replay source: the primary backtesting `PriceSource`
/// (spec D.3.2). Deterministic and wall-clock-free after [`Replay::load`].
#[derive(Debug)]
pub struct Replay {
    /// In-memory index built at load time.
    index: SourceIndex,
    /// RTM cadence found in the loaded data (mode of `interval_secs`;
    /// ties resolve to the shorter cadence). `None` when no RTM signal
    /// was loaded.
    rt_interval_secs: Option<u32>,
}

impl Replay {
    /// Eagerly read every partition intersecting `range` for each signal in
    /// `signals` (any of `rtm_spp`, `dam_spp`, `as_mcpc`, `system_load`;
    /// duplicates are ignored) under `root`.
    ///
    /// # Errors
    /// - [`ErcotError::InvalidParam`] for an unknown signal name.
    /// - [`ErcotError::DataNotFound`] when a requested signal yields zero
    ///   rows in `range` (partial day coverage is tolerated).
    /// - [`ErcotError::SchemaVersion`] when a Parquet file's
    ///   `batsim.schema_version` metadata is missing, unparsable, or not
    ///   equal to [`schema::SCHEMA_VERSION`].
    /// - [`ErcotError::Parse`] on malformed rows (bad types, nulls in
    ///   required columns, unknown provenance/product labels).
    /// - [`ErcotError::Io`]/[`ErcotError::Parquet`] on read failures.
    pub fn load(root: impl AsRef<Path>, range: TimeRange, signals: &[&str]) -> Result<Self> {
        let root = root.as_ref();
        let mut index = SourceIndex::default();
        let mut cadence: BTreeMap<u32, u64> = BTreeMap::new();
        let first_day = cpt::operating_day(range.start);
        let last_day = cpt::operating_day(range.end - Duration::NANOSECOND);
        let mut seen: Vec<&str> = Vec::new();
        for &signal in signals {
            if seen.contains(&signal) {
                continue;
            }
            seen.push(signal);
            let rows = match signal {
                schema::SIGNAL_RTM_SPP => {
                    let load =
                        load_price_signal(root, signal, first_day, last_day, range, &mut index, PriceKind::Rt)?;
                    for (secs, count) in load.cadence {
                        *cadence.entry(secs).or_insert(0) += count;
                    }
                    load.rows
                }
                schema::SIGNAL_DAM_SPP => {
                    load_price_signal(root, signal, first_day, last_day, range, &mut index, PriceKind::Dam)?
                        .rows
                }
                schema::SIGNAL_AS_MCPC => {
                    load_as_signal(root, signal, first_day, last_day, range, &mut index)?
                }
                schema::SIGNAL_SYSTEM_LOAD => {
                    load_system_signal(root, signal, first_day, last_day, range, &mut index)?
                }
                other => {
                    return Err(ErcotError::InvalidParam(format!(
                        "unknown signal `{other}` (expected one of {:?})",
                        schema::SIGNALS
                    )));
                }
            };
            if rows == 0 {
                return Err(ErcotError::DataNotFound {
                    signal: signal.to_string(),
                    location: "ALL".to_string(),
                    start: range.start,
                    end: range.end,
                    root: root.display().to_string(),
                });
            }
        }
        // Cadence mode; BTreeMap iterates ascending so ties keep the
        // shorter cadence.
        let mut rt_interval_secs = None;
        let mut best_count = 0u64;
        for (secs, count) in cadence {
            if count > best_count {
                best_count = count;
                rt_interval_secs = Some(secs);
            }
        }
        Ok(Self { index, rt_interval_secs })
    }

    /// The RTM sample whose interval contains `ts` (used by the sim tick
    /// loop; pure in-memory lookup).
    #[must_use]
    pub fn rt_spp_at(&self, loc: &Location, ts: OffsetDateTime) -> Option<&PriceSample> {
        self.index.rt_spp_at(loc, ts)
    }

    /// The RTM cadence (seconds) found in the loaded data, if an RTM signal
    /// was loaded.
    #[must_use]
    pub const fn interval_secs(&self) -> Option<u32> {
        self.rt_interval_secs
    }
}

impl PriceSource for Replay {
    fn dam_spps(&self, loc: &Location, r: TimeRange) -> Result<Vec<PriceSample>> {
        Ok(self.index.dam_spps(loc, r))
    }

    fn rt_spps(&self, loc: &Location, r: TimeRange) -> Result<Vec<PriceSample>> {
        Ok(self.index.rt_spps(loc, r))
    }

    fn as_prices(&self, r: TimeRange) -> Result<Vec<AsPrice>> {
        Ok(self.index.as_prices(r))
    }

    fn system_signals(&self, r: TimeRange) -> Result<Vec<SystemSignal>> {
        Ok(self.index.system_signals(r))
    }
}

/// Collect the Parquet files under `<root>/<signal>` whose `date=YYYY-MM-DD`
/// partition falls in `[first_day, last_day]` (CPT operating days), sorted
/// for deterministic read order.
fn partition_files(root: &Path, signal: &str, first_day: Date, last_day: Date) -> Result<Vec<PathBuf>> {
    let signal_dir = root.join(signal);
    let dir_entries = match std::fs::read_dir(&signal_dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let day_fmt = format_description!("[year]-[month]-[day]");
    let mut files = Vec::new();
    for entry in dir_entries {
        let entry = entry?;
        let dir_name = entry.file_name();
        let Some(dir_name) = dir_name.to_str() else { continue };
        let Some(date_str) = dir_name.strip_prefix("date=") else { continue };
        let Ok(date) = Date::parse(date_str, &day_fmt) else { continue };
        if date < first_day || date > last_day {
            continue;
        }
        for loc_entry in std::fs::read_dir(entry.path())? {
            let loc_entry = loc_entry?;
            let file_name = loc_entry.file_name();
            let Some(file_name) = file_name.to_str() else { continue };
            if file_name.starts_with("location=") && file_name.ends_with(".parquet") {
                files.push(loc_entry.path());
            }
        }
    }
    files.sort();
    Ok(files)
}

/// Open a Parquet file as a record-batch reader, refusing unknown schema
/// versions before any column is read.
fn open_reader(path: &Path) -> Result<parquet::arrow::arrow_reader::ParquetRecordBatchReader> {
    let builder = ParquetRecordBatchReaderBuilder::try_new(File::open(path)?)?;
    let found = builder
        .metadata()
        .file_metadata()
        .key_value_metadata()
        .and_then(|kvs| kvs.iter().find(|kv| kv.key == schema::SCHEMA_VERSION_KEY))
        .and_then(|kv| kv.value.as_deref())
        .and_then(|raw| raw.parse::<u32>().ok())
        .unwrap_or(0);
    if found != schema::SCHEMA_VERSION {
        return Err(ErcotError::SchemaVersion {
            path: path.display().to_string(),
            found,
            expected: schema::SCHEMA_VERSION,
        });
    }
    Ok(builder.build()?)
}

/// A required column, or a loud parse error naming the file.
fn column<'a, T: 'static>(batch: &'a RecordBatch, name: &str, path: &Path) -> Result<&'a T> {
    batch
        .column_by_name(name)
        .and_then(|array| array.as_any().downcast_ref::<T>())
        .ok_or_else(|| ErcotError::Parse {
            context: path.display().to_string(),
            detail: format!("missing or mistyped column `{name}`"),
        })
}

/// Reject nulls in required columns (loud, never silently zero-filled).
fn require_no_nulls(array: &dyn Array, name: &str, path: &Path) -> Result<()> {
    if array.null_count() > 0 {
        return Err(ErcotError::Parse {
            context: path.display().to_string(),
            detail: format!("null in required column `{name}`"),
        });
    }
    Ok(())
}

/// Integer column tolerant of the plausible writer choices (`UInt32` per
/// schema, `Int32`/`Int64` from other writers).
enum IntColumn<'a> {
    /// `UInt32` (canonical).
    U32(&'a UInt32Array),
    /// `Int32`.
    I32(&'a Int32Array),
    /// `Int64`.
    I64(&'a Int64Array),
}

impl IntColumn<'_> {
    /// Value at `row` as `i64`.
    fn value(&self, row: usize) -> i64 {
        match self {
            Self::U32(a) => i64::from(a.value(row)),
            Self::I32(a) => i64::from(a.value(row)),
            Self::I64(a) => a.value(row),
        }
    }

    /// Null count of the underlying array.
    fn null_count(&self) -> usize {
        match self {
            Self::U32(a) => Array::null_count(*a),
            Self::I32(a) => Array::null_count(*a),
            Self::I64(a) => Array::null_count(*a),
        }
    }
}

/// Read the `interval_secs` column in whatever integer width the writer used.
fn int_column<'a>(batch: &'a RecordBatch, name: &str, path: &Path) -> Result<IntColumn<'a>> {
    if let Ok(a) = column::<UInt32Array>(batch, name, path) {
        return Ok(IntColumn::U32(a));
    }
    if let Ok(a) = column::<Int32Array>(batch, name, path) {
        return Ok(IntColumn::I32(a));
    }
    Ok(IntColumn::I64(column::<Int64Array>(batch, name, path)?))
}

/// Parse a provenance label (serde `snake_case` name of [`Provenance`]).
fn parse_provenance(raw: &str, path: &Path) -> Result<Provenance> {
    match raw {
        "real_time_indicative" => Ok(Provenance::RealTimeIndicative),
        "settlement_final" => Ok(Provenance::SettlementFinal),
        "corrected" => Ok(Provenance::Corrected),
        "synthetic" => Ok(Provenance::Synthetic),
        "omitted" => Ok(Provenance::Omitted),
        other => Err(ErcotError::Parse {
            context: path.display().to_string(),
            detail: format!("unknown provenance label `{other}`"),
        }),
    }
}

/// Parse a UTC epoch-second timestamp.
fn parse_ts(unix: i64, path: &Path) -> Result<OffsetDateTime> {
    OffsetDateTime::from_unix_timestamp(unix).map_err(|e| ErcotError::Parse {
        context: path.display().to_string(),
        detail: format!("bad timestamp {unix}: {e}"),
    })
}

/// Load one price signal (`rtm_spp`/`dam_spp`) into the index.
fn load_price_signal(
    root: &Path,
    signal: &str,
    first_day: Date,
    last_day: Date,
    range: TimeRange,
    index: &mut SourceIndex,
    kind: PriceKind,
) -> Result<PriceLoad> {
    let mut load = PriceLoad::default();
    for path in partition_files(root, signal, first_day, last_day)? {
        for batch in open_reader(&path)? {
            let batch = batch?;
            let ts = column::<Int64Array>(&batch, price_cols::TS, &path)?;
            let interval = int_column(&batch, price_cols::INTERVAL_SECS, &path)?;
            let location = column::<StringArray>(&batch, price_cols::LOCATION, &path)?;
            let lmp = column::<Float64Array>(&batch, price_cols::LMP, &path)?;
            let ordc = column::<Float64Array>(&batch, price_cols::ORDC, &path)?;
            let rdpa = column::<Float64Array>(&batch, price_cols::RDPA, &path)?;
            let provenance = column::<StringArray>(&batch, price_cols::PROVENANCE, &path)?;
            require_no_nulls(ts, price_cols::TS, &path)?;
            require_no_nulls(location, price_cols::LOCATION, &path)?;
            require_no_nulls(lmp, price_cols::LMP, &path)?;
            require_no_nulls(ordc, price_cols::ORDC, &path)?;
            require_no_nulls(rdpa, price_cols::RDPA, &path)?;
            require_no_nulls(provenance, price_cols::PROVENANCE, &path)?;
            if interval.null_count() > 0 {
                return Err(ErcotError::Parse {
                    context: path.display().to_string(),
                    detail: format!("null in required column `{}`", price_cols::INTERVAL_SECS),
                });
            }
            for row in 0..batch.num_rows() {
                let ts_dt = parse_ts(ts.value(row), &path)?;
                if !range.contains(ts_dt) {
                    continue;
                }
                let interval_secs = u32::try_from(interval.value(row)).map_err(|_| ErcotError::Parse {
                    context: path.display().to_string(),
                    detail: format!("interval_secs out of range at row {row}"),
                })?;
                let sample = PriceSample {
                    ts: ts_dt,
                    interval_secs,
                    location: Location::from_settlement_point(location.value(row)),
                    lmp_usd_per_mwh: lmp.value(row),
                    ordc_adder_usd_per_mwh: ordc.value(row),
                    rdpa_adder_usd_per_mwh: rdpa.value(row),
                    provenance: parse_provenance(provenance.value(row), &path)?,
                };
                match kind {
                    PriceKind::Rt => index.insert_rt(sample),
                    PriceKind::Dam => index.insert_dam(sample),
                }
                *load.cadence.entry(interval_secs).or_insert(0) += 1;
                load.rows += 1;
            }
        }
    }
    Ok(load)
}

/// Load the `as_mcpc` signal into the index.
fn load_as_signal(
    root: &Path,
    signal: &str,
    first_day: Date,
    last_day: Date,
    range: TimeRange,
    index: &mut SourceIndex,
) -> Result<u64> {
    let mut rows = 0u64;
    for path in partition_files(root, signal, first_day, last_day)? {
        for batch in open_reader(&path)? {
            let batch = batch?;
            let ts = column::<Int64Array>(&batch, as_cols::TS, &path)?;
            let product = column::<StringArray>(&batch, as_cols::PRODUCT, &path)?;
            let mcpc = column::<Float64Array>(&batch, as_cols::MCPC, &path)?;
            let provenance = column::<StringArray>(&batch, as_cols::PROVENANCE, &path)?;
            require_no_nulls(ts, as_cols::TS, &path)?;
            require_no_nulls(product, as_cols::PRODUCT, &path)?;
            require_no_nulls(mcpc, as_cols::MCPC, &path)?;
            require_no_nulls(provenance, as_cols::PROVENANCE, &path)?;
            for row in 0..batch.num_rows() {
                let ts_dt = parse_ts(ts.value(row), &path)?;
                if !range.contains(ts_dt) {
                    continue;
                }
                let raw_product = product.value(row);
                let product_kind = AsProduct::ALL
                    .iter()
                    .find(|p| p.dam_column() == raw_product)
                    .copied()
                    .ok_or_else(|| ErcotError::Parse {
                        context: path.display().to_string(),
                        detail: format!("unknown AS product `{raw_product}`"),
                    })?;
                index.insert_as(AsPrice {
                    ts: ts_dt,
                    product: product_kind,
                    mcpc_usd_per_mw: mcpc.value(row),
                    provenance: parse_provenance(provenance.value(row), &path)?,
                });
                rows += 1;
            }
        }
    }
    Ok(rows)
}

/// Load the `system_load` signal into the index. Fuel mix is not part of
/// the archive schema, so replayed signals always report `fuel_mix: None`.
fn load_system_signal(
    root: &Path,
    signal: &str,
    first_day: Date,
    last_day: Date,
    range: TimeRange,
    index: &mut SourceIndex,
) -> Result<u64> {
    let mut rows = 0u64;
    for path in partition_files(root, signal, first_day, last_day)? {
        for batch in open_reader(&path)? {
            let batch = batch?;
            let ts = column::<Int64Array>(&batch, load_cols::TS, &path)?;
            let load = column::<Float64Array>(&batch, load_cols::LOAD, &path)?;
            let reserves = column::<Float64Array>(&batch, load_cols::RESERVES, &path)?;
            require_no_nulls(ts, load_cols::TS, &path)?;
            require_no_nulls(load, load_cols::LOAD, &path)?;
            for row in 0..batch.num_rows() {
                let ts_dt = parse_ts(ts.value(row), &path)?;
                if !range.contains(ts_dt) {
                    continue;
                }
                index.insert_sys(SystemSignal {
                    ts: ts_dt,
                    system_load_mw: load.value(row),
                    reserves_mw: (!reserves.is_null(row)).then(|| reserves.value(row)),
                    fuel_mix: None,
                });
                rows += 1;
            }
        }
    }
    Ok(rows)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::types::{LoadZone, TradingHub};
    use arrow::datatypes::{DataType, Field, Schema};
    use parquet::arrow::ArrowWriter;
    use parquet::file::metadata::KeyValue;
    use parquet::file::properties::WriterProperties;
    use std::sync::Arc;
    use tempfile::TempDir;
    use time::macros::datetime;

    fn prov_label(p: Provenance) -> String {
        serde_json::to_value(p).unwrap().as_str().unwrap().to_string()
    }

    fn sample(ts: OffsetDateTime, interval_secs: u32, loc: &Location, lmp: f64) -> PriceSample {
        PriceSample {
            ts,
            interval_secs,
            location: loc.clone(),
            lmp_usd_per_mwh: lmp,
            ordc_adder_usd_per_mwh: 0.0,
            rdpa_adder_usd_per_mwh: 0.0,
            provenance: Provenance::SettlementFinal,
        }
    }

    fn price_batch(rows: &[PriceSample]) -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new(price_cols::TS, DataType::Int64, false),
            Field::new(price_cols::INTERVAL_SECS, DataType::UInt32, false),
            Field::new(price_cols::LOCATION, DataType::Utf8, false),
            Field::new(price_cols::LMP, DataType::Float64, false),
            Field::new(price_cols::ORDC, DataType::Float64, false),
            Field::new(price_cols::RDPA, DataType::Float64, false),
            Field::new(price_cols::PROVENANCE, DataType::Utf8, false),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(rows.iter().map(|r| r.ts.unix_timestamp()).collect::<Vec<_>>())),
                Arc::new(UInt32Array::from(rows.iter().map(|r| r.interval_secs).collect::<Vec<_>>())),
                Arc::new(StringArray::from(
                    rows.iter().map(|r| r.location.settlement_point()).collect::<Vec<_>>(),
                )),
                Arc::new(Float64Array::from(rows.iter().map(|r| r.lmp_usd_per_mwh).collect::<Vec<_>>())),
                Arc::new(Float64Array::from(
                    rows.iter().map(|r| r.ordc_adder_usd_per_mwh).collect::<Vec<_>>(),
                )),
                Arc::new(Float64Array::from(
                    rows.iter().map(|r| r.rdpa_adder_usd_per_mwh).collect::<Vec<_>>(),
                )),
                Arc::new(StringArray::from(rows.iter().map(|r| prov_label(r.provenance)).collect::<Vec<_>>())),
            ],
        )
        .unwrap()
    }

    fn as_batch(rows: &[AsPrice]) -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new(as_cols::TS, DataType::Int64, false),
            Field::new(as_cols::PRODUCT, DataType::Utf8, false),
            Field::new(as_cols::MCPC, DataType::Float64, false),
            Field::new(as_cols::PROVENANCE, DataType::Utf8, false),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(rows.iter().map(|r| r.ts.unix_timestamp()).collect::<Vec<_>>())),
                Arc::new(StringArray::from(rows.iter().map(|r| r.product.dam_column()).collect::<Vec<_>>())),
                Arc::new(Float64Array::from(rows.iter().map(|r| r.mcpc_usd_per_mw).collect::<Vec<_>>())),
                Arc::new(StringArray::from(rows.iter().map(|r| prov_label(r.provenance)).collect::<Vec<_>>())),
            ],
        )
        .unwrap()
    }

    fn load_batch(rows: &[(OffsetDateTime, f64, Option<f64>)]) -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new(load_cols::TS, DataType::Int64, false),
            Field::new(load_cols::LOAD, DataType::Float64, false),
            Field::new(load_cols::RESERVES, DataType::Float64, true),
        ]));
        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(Int64Array::from(rows.iter().map(|r| r.0.unix_timestamp()).collect::<Vec<_>>())),
                Arc::new(Float64Array::from(rows.iter().map(|r| r.1).collect::<Vec<_>>())),
                Arc::new(Float64Array::from(rows.iter().map(|r| r.2).collect::<Vec<_>>())),
            ],
        )
        .unwrap()
    }

    /// Minimal archive writer honouring the schema.rs contract: partition
    /// path layout plus `batsim.schema_version` file metadata.
    fn write_parquet(root: &Path, signal: &str, date: Date, loc_dir: &str, batch: &RecordBatch, version: Option<u32>) {
        let date_str = date
            .format(&format_description!("[year]-[month]-[day]"))
            .unwrap();
        let path = root
            .join(signal)
            .join(format!("date={date_str}"))
            .join(format!("location={loc_dir}.parquet"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let kv = version.map(|v| vec![KeyValue::new(schema::SCHEMA_VERSION_KEY.to_string(), v.to_string())]);
        let props = WriterProperties::builder().set_key_value_metadata(kv).build();
        let mut writer = ArrowWriter::try_new(File::create(path).unwrap(), batch.schema(), Some(props)).unwrap();
        writer.write(batch).unwrap();
        writer.close().unwrap();
    }

    /// Partition rows by CPT operating day and write one file per (day, loc).
    fn write_price_rows(root: &Path, signal: &str, rows: &[PriceSample], version: Option<u32>) {
        let mut by_day_loc: BTreeMap<(Date, String), Vec<PriceSample>> = BTreeMap::new();
        for row in rows {
            by_day_loc
                .entry((cpt::operating_day(row.ts), row.location.settlement_point()))
                .or_default()
                .push(row.clone());
        }
        for ((day, loc), day_rows) in by_day_loc {
            write_parquet(root, signal, day, &loc, &price_batch(&day_rows), version);
        }
    }

    fn hub_north() -> Location {
        Location::Hub(TradingHub::North)
    }

    fn lz_houston() -> Location {
        Location::LoadZone(LoadZone::Houston)
    }

    /// Two full CPT summer days (CDT, UTC-5): 2023-08-17/18.
    fn two_day_range() -> TimeRange {
        TimeRange::new(datetime!(2023-08-17 05:00 UTC), datetime!(2023-08-19 05:00 UTC)).unwrap()
    }

    /// Write a complete small archive (all four signals, two locations for
    /// RTM) covering `range` at 900 s RTM / 3600 s DAM cadence.
    fn write_full_archive(root: &Path, range: TimeRange) {
        let mut rt_rows = Vec::new();
        let mut ts = range.start;
        while ts < range.end {
            rt_rows.push(sample(ts, 900, &hub_north(), 25.0));
            rt_rows.push(sample(ts, 900, &lz_houston(), 30.0));
            ts += Duration::seconds(900);
        }
        write_price_rows(root, schema::SIGNAL_RTM_SPP, &rt_rows, Some(schema::SCHEMA_VERSION));

        let mut dam_rows = Vec::new();
        let mut ts = range.start;
        while ts < range.end {
            dam_rows.push(sample(ts, 3600, &hub_north(), 28.0));
            ts += Duration::seconds(3600);
        }
        write_price_rows(root, schema::SIGNAL_DAM_SPP, &dam_rows, Some(schema::SCHEMA_VERSION));

        let mut as_rows = Vec::new();
        let mut ts = range.start;
        while ts < range.end {
            for (i, product) in AsProduct::ALL.iter().enumerate() {
                as_rows.push(AsPrice {
                    ts,
                    product: *product,
                    mcpc_usd_per_mw: 5.0 + i as f64,
                    provenance: Provenance::SettlementFinal,
                });
            }
            ts += Duration::seconds(3600);
        }
        let mut as_by_day: BTreeMap<Date, Vec<AsPrice>> = BTreeMap::new();
        for row in as_rows {
            as_by_day.entry(cpt::operating_day(row.ts)).or_default().push(row);
        }
        for (day, rows) in as_by_day {
            write_parquet(root, schema::SIGNAL_AS_MCPC, day, "ALL", &as_batch(&rows), Some(schema::SCHEMA_VERSION));
        }

        let mut sys_rows = Vec::new();
        let mut ts = range.start;
        while ts < range.end {
            sys_rows.push((ts, 50_000.0, Some(9_000.0)));
            ts += Duration::seconds(900);
        }
        let mut sys_by_day: BTreeMap<Date, Vec<(OffsetDateTime, f64, Option<f64>)>> = BTreeMap::new();
        for row in sys_rows {
            sys_by_day.entry(cpt::operating_day(row.0)).or_default().push(row);
        }
        for (day, rows) in sys_by_day {
            write_parquet(root, schema::SIGNAL_SYSTEM_LOAD, day, "ALL", &load_batch(&rows), Some(schema::SCHEMA_VERSION));
        }
    }

    const ALL_SIGNALS: [&str; 4] = [
        schema::SIGNAL_RTM_SPP,
        schema::SIGNAL_DAM_SPP,
        schema::SIGNAL_AS_MCPC,
        schema::SIGNAL_SYSTEM_LOAD,
    ];

    #[test]
    fn round_trip_write_then_replay() {
        let tmp = TempDir::new().unwrap();
        let range = two_day_range();
        write_full_archive(tmp.path(), range);

        let replay = Replay::load(tmp.path(), range, &ALL_SIGNALS).unwrap();

        // 48 h at 900 s = 192 RT samples per location, ordered, filtered.
        let north = replay.rt_spps(&hub_north(), range).unwrap();
        assert_eq!(north.len(), 192);
        assert!(north.windows(2).all(|w| w[0].ts < w[1].ts));
        assert!(north.iter().all(|s| s.location == hub_north()));
        assert!(north.iter().all(|s| (s.lmp_usd_per_mwh - 25.0).abs() < f64::EPSILON));

        let houston = replay.rt_spps(&lz_houston(), range).unwrap();
        assert_eq!(houston.len(), 192);
        assert!(houston.iter().all(|s| (s.lmp_usd_per_mwh - 30.0).abs() < f64::EPSILON));

        // Unknown location: empty, not an error.
        let nowhere = replay.rt_spps(&Location::Node("NOPE".to_string()), range).unwrap();
        assert!(nowhere.is_empty());

        // 48 hourly DAM samples, 240 AS rows ordered by (ts, product),
        // 192 system signals ordered by ts.
        let dam = replay.dam_spps(&hub_north(), range).unwrap();
        assert_eq!(dam.len(), 48);
        assert!(dam.iter().all(|s| s.interval_secs == 3600));
        let as_prices = replay.as_prices(range).unwrap();
        assert_eq!(as_prices.len(), 240);
        assert!(as_prices
            .windows(2)
            .all(|w| (w[0].ts, w[0].product) < (w[1].ts, w[1].product)));
        let sys = replay.system_signals(range).unwrap();
        assert_eq!(sys.len(), 192);
        assert!(sys.windows(2).all(|w| w[0].ts < w[1].ts));
        assert!(sys.iter().all(|s| s.reserves_mw == Some(9_000.0)));
        assert!(sys.iter().all(|s| s.fuel_mix.is_none()));

        // Interval containment lookups.
        let first = replay.rt_spp_at(&hub_north(), range.start + Duration::seconds(100)).unwrap();
        assert_eq!(first.ts, range.start);
        let on_boundary = replay.rt_spp_at(&hub_north(), range.start + Duration::seconds(900)).unwrap();
        assert_eq!(on_boundary.ts, range.start + Duration::seconds(900));
        assert!(replay.rt_spp_at(&hub_north(), range.end).is_none());
        assert!(replay.rt_spp_at(&hub_north(), range.start - Duration::seconds(1)).is_none());

        assert_eq!(replay.interval_secs(), Some(900));

        // Sub-range query is half-open.
        let sub = TimeRange::new(range.start, range.start + Duration::seconds(3600)).unwrap();
        assert_eq!(replay.rt_spps(&hub_north(), sub).unwrap().len(), 4);
    }

    #[test]
    fn partial_day_coverage_is_tolerated() {
        let tmp = TempDir::new().unwrap();
        let range = two_day_range();
        // Only the first day has data.
        let day_one = TimeRange::new(range.start, range.start + Duration::hours(24)).unwrap();
        let mut rows = Vec::new();
        let mut ts = day_one.start;
        while ts < day_one.end {
            rows.push(sample(ts, 900, &hub_north(), 25.0));
            ts += Duration::seconds(900);
        }
        write_price_rows(tmp.path(), schema::SIGNAL_RTM_SPP, &rows, Some(schema::SCHEMA_VERSION));

        let replay = Replay::load(tmp.path(), range, &[schema::SIGNAL_RTM_SPP]).unwrap();
        assert_eq!(replay.rt_spps(&hub_north(), range).unwrap().len(), 96);
        // The gap returns an empty vec, not an error.
        let gap = TimeRange::new(range.start + Duration::hours(30), range.end).unwrap();
        assert!(replay.rt_spps(&hub_north(), gap).unwrap().is_empty());
        assert!(replay.rt_spp_at(&hub_north(), range.start + Duration::hours(30)).is_none());
    }

    #[test]
    fn zero_rows_in_range_is_data_not_found() {
        let tmp = TempDir::new().unwrap();
        let range = two_day_range();
        write_full_archive(tmp.path(), range);
        // dam_spp was written, but a signal that was not written is absent.
        let err = Replay::load(tmp.path(), range, &[schema::SIGNAL_RTM_SPP, "as_mcpc_missing"]);
        assert!(matches!(err, Err(ErcotError::InvalidParam(_))));
        // A range disjoint from the archive: zero rows for a real signal.
        let far = TimeRange::new(datetime!(2024-01-01 00:00 UTC), datetime!(2024-01-02 00:00 UTC)).unwrap();
        let err = Replay::load(tmp.path(), far, &[schema::SIGNAL_RTM_SPP]);
        match err {
            Err(ErcotError::DataNotFound { signal, start, end, .. }) => {
                assert_eq!(signal, schema::SIGNAL_RTM_SPP);
                assert_eq!(start, far.start);
                assert_eq!(end, far.end);
            }
            other => panic!("expected DataNotFound, got {other:?}"),
        }
    }

    #[test]
    fn schema_version_mismatch_is_refused() {
        let tmp = TempDir::new().unwrap();
        let range = two_day_range();
        let rows = vec![sample(range.start, 900, &hub_north(), 25.0)];
        // Version 999: refused with the found version reported.
        write_price_rows(tmp.path(), schema::SIGNAL_RTM_SPP, &rows, Some(999));
        match Replay::load(tmp.path(), range, &[schema::SIGNAL_RTM_SPP]) {
            Err(ErcotError::SchemaVersion { found, expected, .. }) => {
                assert_eq!(found, 999);
                assert_eq!(expected, schema::SCHEMA_VERSION);
            }
            other => panic!("expected SchemaVersion, got {other:?}"),
        }
        // Missing version key: refused as version 0 (never silently read).
        let tmp2 = TempDir::new().unwrap();
        write_price_rows(tmp2.path(), schema::SIGNAL_RTM_SPP, &rows, None);
        match Replay::load(tmp2.path(), range, &[schema::SIGNAL_RTM_SPP]) {
            Err(ErcotError::SchemaVersion { found, .. }) => assert_eq!(found, 0),
            other => panic!("expected SchemaVersion, got {other:?}"),
        }
    }

    #[test]
    fn fall_back_day_keeps_all_25_hours() {
        let tmp = TempDir::new().unwrap();
        // 2023-11-05 CPT: 25 h day (CST/CDT transition), 100 x 900 s.
        let range = TimeRange::new(datetime!(2023-11-05 05:00 UTC), datetime!(2023-11-06 06:00 UTC)).unwrap();
        let mut rows = Vec::new();
        let mut ts = range.start;
        while ts < range.end {
            rows.push(sample(ts, 900, &hub_north(), 25.0));
            ts += Duration::seconds(900);
        }
        assert_eq!(rows.len(), 100);
        write_price_rows(tmp.path(), schema::SIGNAL_RTM_SPP, &rows, Some(schema::SCHEMA_VERSION));

        let replay = Replay::load(tmp.path(), range, &[schema::SIGNAL_RTM_SPP]).unwrap();
        let back = replay.rt_spps(&hub_north(), range).unwrap();
        assert_eq!(back.len(), 100);
        assert!(back.windows(2).all(|w| w[0].ts < w[1].ts));
        // The repeated local hour survives as two distinct UTC instants:
        // 01:30 CPT maps to both 06:30 UTC (CDT) and 07:30 UTC (CST).
        let first = replay.rt_spp_at(&hub_north(), datetime!(2023-11-05 06:30 UTC)).unwrap();
        let repeat = replay.rt_spp_at(&hub_north(), datetime!(2023-11-05 07:30 UTC)).unwrap();
        assert_eq!(first.ts, datetime!(2023-11-05 06:30 UTC));
        assert_eq!(repeat.ts, datetime!(2023-11-05 07:30 UTC));
        assert_eq!(repeat.ts - first.ts, Duration::hours(1));
    }

    #[test]
    fn provenance_labels_match_serde_names() {
        for (label, prov) in [
            ("real_time_indicative", Provenance::RealTimeIndicative),
            ("settlement_final", Provenance::SettlementFinal),
            ("corrected", Provenance::Corrected),
            ("synthetic", Provenance::Synthetic),
            ("omitted", Provenance::Omitted),
        ] {
            assert_eq!(serde_json::to_string(&prov).unwrap(), format!("\"{label}\""));
        }
    }
}
