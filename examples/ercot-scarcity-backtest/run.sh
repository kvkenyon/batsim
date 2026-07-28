#!/usr/bin/env bash
# End-to-end ERCOT scarcity-day backtest (spec Part D.6):
#   baseline (no dispatch) vs heuristic (charge off-peak, discharge into the
#   evening peak) against the real 2023-08-17 ERCOT scarcity day, with an
#   ECRS award and an illustrative 4CP candidate on the heuristic run.
#
# Prereqs:
#   1. Ingest the 2023 ERCOT archives (one-time, ~70 MB):
#        cargo run -p batsim-ercot --bin batsim-ercot-ingest -- fetch --report rtm-spp --year 2023 --out data/ercot
#        cargo run -p batsim-ercot --bin batsim-ercot-ingest -- fetch --report dam-spp --year 2023 --out data/ercot
#        cargo run -p batsim-ercot --bin batsim-ercot-ingest -- fetch --report as-mcpc --year 2023 --out data/ercot
#   2. Start the server against that data dir:
#        cargo run -p batsim-server -- --data-dir data
#
# Usage: examples/ercot-scarcity-backtest/run.sh
set -euo pipefail
cd "$(dirname "$0")"

BASE="${BATSIM_URL:-http://127.0.0.1:8080}"
DATA_DIR="${DATA_DIR:-data}"
DAY="2023-08-17"

need() { command -v "$1" >/dev/null || { echo "missing dependency: $1" >&2; exit 2; }; }
need curl; need python3

if [[ ! -f "$DATA_DIR/ercot/rtm_spp/date=$DAY/location=LZ_NORTH.parquet" ]]; then
  echo "No replay data for $DAY at $DATA_DIR/ercot - run the ingest commands in this script's header first." >&2
  exit 2
fi
curl -sf "$BASE/v1/system/health" >/dev/null || {
  echo "No batsim server at $BASE - start one: cargo run -p batsim-server -- --data-dir $DATA_DIR" >&2
  exit 2
}


echo "== Creating fleet (100 x Powerwall 3, LZ_NORTH)"
FLEET_ID=$(curl -sf -X POST "$BASE/v1/fleets" -H 'content-type: application/json' \
  --data @fleet.json | python3 -c "import json,sys;print(json.load(sys.stdin)['id'])")
echo "   fleet: $FLEET_ID"

run_backtest() {
  local label="$1" req="$2"
  local body id state
  body=$(sed "s/@FLEET_ID@/$FLEET_ID/" "$req")
  id=$(curl -sf -X POST "$BASE/v1/backtests" -H 'content-type: application/json' \
    -d "$body" | python3 -c "import json,sys;print(json.load(sys.stdin)['id'])")
  echo "== $label run started: $id" >&2
  while :; do
    state=$(curl -sf "$BASE/v1/backtests/$id" | python3 -c "import json,sys;print(json.load(sys.stdin)['state'])")
    case "$state" in
      settled) break ;;
      failed*) echo "   run FAILED: $state" >&2; exit 1 ;;
      *) sleep 5 ;;
    esac
  done
  echo "   settled: $id" >&2
  curl -sf "$BASE/v1/backtests/$id/settlement"
}

BASELINE_JSON=$(run_backtest "baseline" backtest-baseline.json)
HEURISTIC_JSON=$(run_backtest "heuristic" backtest-heuristic.json)

BASELINE_JSON="$BASELINE_JSON" HEURISTIC_JSON="$HEURISTIC_JSON" python3 - <<'PY'
import json, os

base = json.loads(os.environ["BASELINE_JSON"])
heur = json.loads(os.environ["HEURISTIC_JSON"])

def row(d):
    t = d["totals"]
    e = t["energy"]
    as_net = sum(p["net_usd"] for p in t["as"].values())
    return {
        "wholesale_usd": e["wholesale_usd"],
        "retail_avoided_usd": e["retail_avoided_cost_usd"],
        "charging_cost_usd": e["charging_cost_usd"],
        "as_net_usd": as_net,
        "four_cp_usd": t["four_cp"]["est_annual_savings_usd"],
        "margin_usd": t["retailer_margin_usd"],
        "provenance": d["provenance"],
    }

b, h = row(base), row(heur)
print()
print(f"{'component':<28}{'baseline':>14}{'heuristic':>14}{'delta':>14}")
print("-" * 70)
for k in ["wholesale_usd", "retail_avoided_usd", "charging_cost_usd",
          "as_net_usd", "four_cp_usd", "margin_usd"]:
    print(f"{k:<28}{b[k]:>14.2f}{h[k]:>14.2f}{h[k]-b[k]:>14.2f}")
print("-" * 70)
print(f"provenance: {b['provenance']} | rules: {base['rules_version']} | "
      f"intervals: {len(base['intervals'])} x {base['settlement_interval_secs']}s | "
      f"homes: {len(base['homes'])}")
print(f"strategy P&L uplift: ${h['margin_usd']-b['margin_usd']:.2f}")
PY
