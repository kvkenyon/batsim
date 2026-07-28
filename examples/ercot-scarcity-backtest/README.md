# Worked example: ERCOT scarcity-day backtest (2023-08-17)

Replays the real ERCOT scarcity day of **2023-08-17** (LZ_NORTH real-time
SPP peaked at **$5,197.60/MWh** at 19:30 CPT; the 17:00-21:00 CPT window
averaged ~$3,588/MWh against an off-peak mean of ~$9/MWh) against a
simulated 100-home Tesla Powerwall 3 fleet, twice:

1. **baseline** - no dispatch (self-consumption; PV surplus exports at SPP).
2. **heuristic** - charge 5 kW/home 01:00-05:00 CPT (~$9/MWh), discharge
   5 kW/home 17:00-21:00 CPT into the evening peak, carry a 0.4 MW ECRS
   award 17:00-19:00 CPT (scored as a deployment from measured battery
   delivery), and score an illustrative 4CP candidate at 17:00 CPT (the
   first discharge interval, so the metered-before-after baseline is
   unpolluted).

Both runs settle every 15-minute interval (the 2023 pre-RTC+B cadence)
against settlement-final historical prices and print a P&L comparison.

## Data (one-time, ~70 MB download from ERCOT MIS)

```bash
cargo run -p batsim-ercot --bin batsim-ercot-ingest -- fetch --report rtm-spp --year 2023 --out data/ercot
cargo run -p batsim-ercot --bin batsim-ercot-ingest -- fetch --report dam-spp --year 2023 --out data/ercot
cargo run -p batsim-ercot --bin batsim-ercot-ingest -- fetch --report as-mcpc --year 2023 --out data/ercot
```

Reports ingested (settlement-final historical series, see
`crates/batsim-ercot/README.md` for the verification log):

- Historical RTM Load Zone and Hub Prices (report type 13061, NP6-785-ER)
- Historical DAM Load Zone and Hub Prices (report type 13060, NP4-180-ER)
- Historical DAM Clearing Prices for Capacity (report type 13091)

## Run

```bash
cargo run -p batsim-server -- --data-dir data &
examples/ercot-scarcity-backtest/run.sh
```

`run.sh` creates the fleet (`fleet.json`), starts both backtests
(`backtest-baseline.json`, `backtest-heuristic.json`), waits for each to
settle, and prints the comparison. A run takes a few minutes in a debug
build (100 homes x 86,400 1-second ticks).

## Measured output (debug build, this repo)

```
component                         baseline     heuristic         delta
----------------------------------------------------------------------
wholesale_usd                      3151.25       4503.18       1351.93
retail_avoided_usd                  225.33        420.16        194.83
charging_cost_usd                    61.43        245.60        184.18
as_net_usd                            0.00       2086.18       2086.18
four_cp_usd                          -0.00       4357.83       4357.83
margin_usd                         3376.58      11367.36       7990.78
----------------------------------------------------------------------
provenance: settlement_final | rules: v2025 | intervals: 96 x 900s | homes: 100
```

Notes:

- The baseline already earns $3,151 exporting PV into the midday/evening
  ramp - 2023-08-17 was that kind of day.
- ECRS revenue reflects the real 2023-08-17 DAM clearing prices, which
  spiked with scarcity; the settlement engine does not hard-code AS price
  levels (spec D.1.3).
- The 4CP number is `candidate_unconfirmed`: 415 kW of measured fleet
  net-load reduction x $3.5/kW-mo x 3 (12 months x 1/4 annual allocation).
  2023's actual 4CP intervals were in June-September; the candidate here is
  illustrative - the 4CP watch flags candidates automatically when the
  archive carries a system-load signal.
- `provenance: settlement_final` is derived from the loaded data, not the
  request; a synthetic archive produces `synthetic`.
