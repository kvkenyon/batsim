# batsim-ercot

ERCOT market integration for the residential battery fleet simulator (spec
Part D): normalized market data types, the `PriceSource` trait, Parquet
replay, a seeded synthetic generator, settlement, and the ERCOT MIS
ingestion pipeline documented here.

## Ingestion pipeline (spec D.3.3)

```
ERCOT MIS (xlsx / csv / zip-of-csv) ──► parse/normalize (CPT → UTC)
    ──► canonical Arrow tables ──► Parquet partitions
    ──► <signal>/date=YYYY-MM-DD/location=<LOC>.parquet + manifest.json
```

Library code lives in `src/ingest/` (pure: no network, no wall clock); the
`batsim-ercot-ingest` binary drives it. All timestamps are UTC
interval-START (`interval_start_utc`, i64 epoch seconds) with explicit
`interval_secs`. Date partitions are CPT operating days; the fall-back
25-hour day is preserved end to end (100 intervals on 2023-11-05, verified
by round-trip tests). Location-less signals (`as_mcpc`, `system_load`) use
the location directory `ALL`. Every Parquet file carries
`batsim.schema_version = "1"` in its key-value metadata; readers refuse
unknown versions.

### Binary usage

```sh
# Download a yearly report from ERCOT MIS and ingest it:
batsim-ercot-ingest fetch --report rtm-spp --year 2023 --out /data/ercot

# Ingest a local file (.xlsx, .csv, or .zip-of-csv; auto-detected):
batsim-ercot-ingest import --report rtm-spp \
    --file rpt.00013061.0000000000000000.RTMLZHBSPP_2023.xlsx --out /data/ercot

# Verify an archive against its manifest (schema version, row counts,
# strictly-increasing timestamps per partition):
batsim-ercot-ingest verify --root /data/ercot

# Generate one synthetic day (rtm_spp + dam_spp + as_mcpc):
batsim-ercot-ingest synth --out /data/ercot --date 2023-08-17 \
    --location LZ_HOUSTON --seed 42 --interval-secs 900
```

Report kinds: `rtm-spp` (historical RTM load-zone/hub SPPs, 15-min),
`dam-spp` (historical DAM load-zone/hub SPPs, hourly), `as-mcpc` (DAM AS
clearing prices for capacity, hourly).

### Parsing notes

- **Cadence inference**: `interval_secs` derives from the maximum
  `Delivery Interval` value in the file (4 → 900 s; 1 → 3600 s for DAM).
- **Dedup**: ERCOT's historical RTM workbook repeats every load-zone row
  verbatim (hub rows appear once). The parser deduplicates on
  `(ts, location)` keeping the first occurrence and reports the count;
  a normal day therefore yields 96 intervals per location, not 192.
- **DST**: CPT hour-ending rows convert via `cpt::cpt_interval_to_utc`;
  the repeated fall-back hour (`Repeated Hour Flag = Y`) maps to the CST
  occurrence and is never deduplicated away.
- **Tolerant headers**: column matching normalizes case/punctuation
  (`Delivery Date` ≡ `DeliveryDate`); AS product columns match by
  contained product name (`REGUP` matches `REGUP MCPC`), so both the
  historical and current report layouts parse. Missing products (ECRS
  before 2023-06) are skipped.
- **Empty price cells** are skipped and counted; malformed values are
  hard parse errors.

### Provenance and the ORDC/RDPA adder split

Rows parsed from the historical MIS reports carry
`Provenance::SettlementFinal` (48-h-delayed, settlement-quality series).
The ORDC scarcity-adder / RDPA split is **not ingested in v1**: the adder
reports moved behind the data.ercot.com registration wall (see the
verification log below). Adder columns are written as `0.0` and must be
read as *omitted*, not measured zero, per the `Provenance` convention in
`types.rs`; consumers use `lmp_usd_per_mwh` (which already includes any
adders ERCOT folded into the settlement point price). Synthetic data is
stamped `Provenance::Synthetic` by the `synth` subcommand.

### Manifest

`manifest.json` records schema version, rules version
(`ErcotRules::meta.protocol_version`), source report ID, MIS DocIDs,
ingest timestamp, and one entry per partition (signal, date, location,
relative path, row count, provenance). Writes are upserts keyed by
`(signal, date, location)` with entries sorted and pretty-printed JSON, so
serialization is deterministic. Library writers take the ingest timestamp
as a parameter — no hidden wall clock.

### Test fixture

`tests/fixtures/rtm_sample_2023-08-17.xlsx` (10 KB, one day, 192 rows,
monthly-sheet layout mirroring report 13061) was generated with:

```sh
python3 - <<'EOF'
import openpyxl
wb = openpyxl.Workbook(); ws = wb.active; ws.title = "Aug"
ws.append(["Delivery Date","Delivery Hour","Delivery Interval",
           "Repeated Hour Flag","Settlement Point Name",
           "Settlement Point Type","Settlement Point Price"])
for name, typ, base in [("LZ_NORTH","LZ",10.0),("HB_NORTH","HU",9.0)]:
    n = 0
    for h in range(1,25):
        for i in range(1,5):
            n += 1
            ws.append(["08/17/2023", h, i, "N", name, typ, round(base + n*0.5, 2)])
wb.save("crates/batsim-ercot/tests/fixtures/rtm_sample_2023-08-17.xlsx")
EOF
```

## D.8 verification log (MUST-VERIFY checklist)

All endpoints and IDs below verified **2026-07-27** against
`mis.ercot.com` (document list JSON + download). Rules constants live in
`config/ercot_rules.v2025.toml` (verification date recorded there).

| Signal | Report (ERCOT name) | Type ID | URL pattern | Format | Cadence | Delay | Verified |
|---|---|---|---|---|---|---|---|
| RTM SPP | Historical RTM Load Zone and Hub Prices (`RTMLZHBSPP`) | 13061 | `misapp/servlets/IceDocListJsonWS?reportTypeId=<id>` → `misdownload/servlets/mirDownload?doclookupId=<DocID>` | xlsx, yearly, monthly sheets | 15-min | 48-h (settlement final) | 2026-07-27 (2023 file parsed; see below) |
| DAM SPP | Historical DAM Load Zone and Hub Prices (`DAMLZHBSPP`) | 13060 | same | xlsx, yearly | hourly | 48-h | 2026-07-27 |
| AS MCPC | DAM Clearing Prices for Capacity (historical) | 13091 | same | xlsx or csv, yearly | hourly | 48-h | 2026-07-27 |

Explicit checklist notes:

1. **ORDC/RDPA adder reports** (`NP6-792-ER` historical RTM ORDC adders,
   `NP6-793-ER` historical RTM reliability-deployment adders): no longer
   anonymously downloadable from MIS; they moved to the data.ercot.com
   portal, which requires registration. **The adder split is not ingested
   in v1** — adder columns are `0.0` and documented as omitted (see
   *Provenance* above). Revisit with portal credentials in a later
   milestone.
2. **Pre-RTC+B cadence**: historical data (through the RTC+B cutover) is
   15-minute settlement cadence in report 13061 even though SCED runs
   every 5 minutes; the ingest layer infers cadence from the file rather
   than hardcoding it.
3. **RTC+B cutover 2025-12-05**: Real-Time Co-Optimization + Batteries
   went live 2025-12-05, discontinuing/replacing several legacy MIS
   reports (including the near-real-time ORDC adders/reserves report
   `NP6-323-CD`). Post-cutover real-time products need re-verification
   before a `Live` adapter is built; the historical yearly reports used
   here are unaffected. Replay remains the primary backtesting mode.
4. **HCAP/LCAP**: High system-wide offer cap $5,000/MWh and low cap
   $2,000/MWh per PUCT rule 16 TAC §25.505(g) (HCAP lowered from $9,000
   effective 2022-01-01, approved December 2021) and ERCOT Nodal
   Protocols §6.4.9 (Emergency Pricing Program trigger: sustained hours
   at HCAP in a rolling window). Values carried in
   `config/ercot_rules.v2025.toml`; verified 2026-07-27. Uri-era replays
   must use the pre-2022 $9,000 cap (also in the rules file).

### Real-data validation (report 13061, 2023 file)

Parsed `rpt.00013061.0000000000000000.RTMLZHBSPP_2023.xlsx` (22 MB,
12 monthly sheets) with `batsim-ercot-ingest import`. Numbers recorded in
the validation log section below (updated at each re-verification).

## Scope rules

ERCOT only; no other ISO. No network I/O on any simulation path — the
`fetch` code runs only inside the ingest binary. Determinism: no wall
clock, no thread RNG, and no `HashMap` iteration-order dependence in
library outputs (`BTreeMap` everywhere order matters).
