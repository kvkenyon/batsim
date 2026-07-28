# batsim

An ERCOT-only residential battery fleet simulator, written in Rust.

batsim provides physics-faithful virtual residential battery systems —
Tesla Powerwall 2/3, Enphase IQ Battery, SolarEdge Home Battery, and
sonnen — behind an OpenAPI-first HTTP API, so dispatch strategies can be
developed and tested against realistic fleet behavior and real ERCOT
market history without touching real hardware.

## Status

Core engine, device registry, the HTTP API, and ERCOT backtesting are
complete: homes and fleets CRUD with deterministic fleet manifests, scenario
bindings (time, prices, weather, outages, seed), virtual time control,
fleet dispatch with jittered per-device execution latency, audit log,
idempotency, telemetry series and live streams, all documented by an
OpenAPI document generated from code. Backtests replay a real ERCOT day
from normalized MIS archives (ingested by `batsim-ercot-ingest`) against a
fleet and settle per-interval energy, ancillary services, 4CP, and
retailer margin into a deterministic P&L report. Outage physics,
snapshots, and vendor-API mimicry land in later milestones.

## Quick start

```bash
cargo build -p batsim-server
./target/debug/batsim --config config/batsim.toml &
curl -s localhost:8080/v1/system/health
```

Interactive API docs (Swagger UI) live at `http://localhost:8080/docs`;
the OpenAPI 3.1 document is served live at `/openapi.json` and vendored
at `api/openapi.json` (regenerate with `batsim --dump-openapi`; CI fails
if the vendored copy drifts).

A minimal session:

```bash
curl -s -X POST localhost:8080/v1/fleets -H 'content-type: application/json' -d @examples/fleet-100.json
curl -s -X POST localhost:8080/v1/scenarios -H 'content-type: application/json' -d @examples/scenario-day.json
curl -s -X POST localhost:8080/v1/scenarios/scn_01J…:activate
curl -s -X POST localhost:8080/v1/sim:start
curl -N 'localhost:8080/v1/telemetry/stream?fleet_id=flt_01J…'
```

## API surface

All routes are under `/v1` and return RFC 9457 problem documents on
error. Every mutating POST accepts an `Idempotency-Key` header; every
dispatch takes a client-supplied `command_id` so retries can never
execute twice.

- `/v1/registry/*` — device catalog queries and version.
- `/v1/homes` — simulated home CRUD (config changes while paused).
- `/v1/fleets` — manifests expand deterministically into homes;
  re-applying a manifest yields an identical expansion hash.
- `/v1/scenarios` — bind a time range, price source, weather feed,
  outage schedule, and seed; one active scenario at a time.
- `/v1/sim:*` — virtual time: start/pause/resume/stop, synchronous step
  and run-until, speed multiplier.
- `/v1/dispatch` — kW setpoints, reserve SOC, operating modes, PV
  curtailment; jittered per-device execution latency; audit log.
- `/v1/telemetry/*` — columnar history (1 s to 1 h buckets, settlement
  aligned at 5 minutes) plus SSE and WebSocket live streams with
  filtering and downsampling.
- `/v1/backtests` — replay an ERCOT operating day against a fleet with
  an inline dispatch strategy; per-interval settlement streams over the
  telemetry path and a final settlement report endpoint returns the P&L.
- `/v1/system/*` — health, version, redacted config.

Scenario prices come from a static, synthetic, or replay source: replay
binds a normalized ERCOT Parquet archive (`<data_dir>/ercot`, written by
`batsim-ercot-ingest` from real ERCOT MIS yearly reports) to a
settlement point. A worked scarcity-day backtest (baseline vs heuristic,
real 2023-08-17 data) lives in `examples/ercot-scarcity-backtest/`.

## Generating clients

Clients are generated from the live document, never handwritten:

```bash
npx @openapitools/openapi-generator-cli generate \
  -i http://localhost:8080/openapi.json \
  -g python -o clients/python --additional-properties=packageName=batsim_client
```

(Fern and Stainless accept the same document.) A complete example —
generate the client, then create a 100-home fleet, run a scenario,
dispatch it, and read telemetry — runs with one command:

```bash
examples/python-e2e/run.sh
```

## CLI

`batsimctl` mirrors the API one-to-one and prints JSON by default:

```bash
batsimctl fleets create examples/fleet-100.json
batsimctl sim step 3600
batsimctl dispatch run - --idempotency-key $UUID < command.json
batsimctl telemetry fleet-series <fleet-id> --resolution 5m --agg sum
batsimctl backtests start examples/ercot-scarcity-backtest/backtest-baseline.json
```

## Configuration

File + environment + flags, in increasing precedence. The file defaults
to `config/batsim.toml`; environment overrides use `BATSIM_` with `__`
for nesting (`BATSIM_SERVER__PORT=9090`). `--print-config` shows the
effective, redacted configuration.

Auth is off by default (single-tenant local). Set `auth.api_keys` (or
`BATSIM_AUTH__API_KEYS=k1,k2`) to require bearer tokens; read-only keys
are supported too.

## Testing

```bash
cargo test --workspace                 # unit + integration, incl. determinism
cargo clippy --workspace --all-targets -- -D warnings
examples/python-e2e/run.sh             # generated-client E2E

# contract tests against a running server
schemathesis run http://localhost:8080/openapi.json --checks all \
  --exclude-path /v1/telemetry/stream --exclude-path /v1/telemetry/ws
```

The `schemathesis.toml` in this repo documents the per-operation
exemptions; the two stream paths are excluded because they are infinite
streams and an upgrade handshake, not request/response pairs.

## Layout

- `crates/batsim-core` — synchronous physics engine (no async, no I/O).
- `crates/batsim-registry` — embedded, integrity-checked device catalog.
- `crates/batsim-server` — axum shell, OpenAPI assembly, engine thread.
- `crates/batsim-cli` — `batsimctl` admin CLI.
- `crates/batsim-ercot` — ERCOT market data: `PriceSource` replay over
  Parquet, synthetic generator, settlement engine, MIS ingest binary.
- `web/` — 3D fleet console (React + TypeScript, MapLibre + three.js).
- `registry/` — catalog JSON (batteries, inverters, controllers, PV).
- `api/openapi.json` — vendored API document (CI-checked).
- `examples/python-e2e/` — generated-client end-to-end drive.
- `examples/ercot-scarcity-backtest/` — real-day backtest with P&L.
- `docs/residential-battery-fleet-simulator-spec.md` — the build brief.

## Web console

The `web/` app is a pure client of the HTTP API: a MapLibre map of the
ERCOT territory with lensable fleet markers, a three.js street-level
neighborhood driven by live telemetry, and a click-to-inspect panel for
any home. Operator tools turn it into a control surface: build mode
places homes from the registry catalog by clicking inside an ERCOT zone
polygon (with a two-step remove in the inspector), the dispatch console
sends zone- or fleet-scoped charge/discharge/idle commands and counts
per-home acknowledgements as they arrive, fleet scenarios save named
fleet-composition snapshots to browser local storage, and a time
scrubber drags through the demo replay tape (disabled in live mode).
Its TypeScript API types are generated from the vendored
OpenAPI document with one command (run from `web/`):

```bash
npm run gen:client        # openapi-typescript ../api/openapi.json
```

Regenerate whenever `api/openapi.json` changes; the generated file is
checked in and never hand-edited. Development workflow:

```bash
cd web
npm ci
npm run dev               # proxies /v1 to localhost:8080
npm test                  # unit tests
npm run build             # typecheck + production bundle
npm run test:e2e          # headless demo-mode smoke test
```

With no server running the console boots into a demo mode that replays a
recorded telemetry trace (bundled under `web/public/traces/demo/`) through
the same ingest pipeline as live data; `?demo=1` forces it. Re-record the
trace against a fresh world with `npm run record:demo`.
