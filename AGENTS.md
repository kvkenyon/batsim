# Project agent memory

batsim is an ERCOT-only residential battery fleet simulator written in Rust: physics-faithful virtual Tesla Powerwall 2/3, Enphase IQ Battery, SolarEdge Home Battery, and sonnen systems behind an OpenAPI-first HTTP API for dispatch-strategy testing. The authoritative build brief is `docs/residential-battery-fleet-simulator-spec.md` — follow it; do not duplicate its content here.

## Implementation state

M1 (core engine + registry, spec 0.2) is complete on branch `fm/batsim-m1`: crates `batsim-core` (tick engine, battery/inverter/load/PV physics, home pipeline) and `batsim-registry` (21-entry JSON catalog under `registry/`, integrity-checked loader, HomeSystem composer). M2+ (server, ERCOT, outages, thermal, degradation, vendor mimicry) has not started.

## Engineering loop (spec C.8)

`cargo check --workspace` -> `cargo clippy --workspace --all-targets -- -D warnings` (pedantic denied via workspace lints; curated allows documented in root `Cargo.toml`) -> `cargo test --workspace`. Toolchain pinned 1.83.0 via `rust-toolchain.toml`; dep versions must stay 1.83-compatible (check edition2024 conflicts when bumping). Regenerate goldens only with `INSTA_UPDATE=always cargo test -p batsim-core --test golden` and review the diff. The determinism test takes ~2 min in debug.

## Binding decisions made in M1 (documented in code)

- D1: `ac_to_dc` is conservation-true (DC delivered = AC x eta); spec B.3.1's literal division formula invents energy on the charge path.
- D2: DC-coupled `rte_ac_coupled` = battery-curve product x eta_hyb^2 (double conversion per A.3.3); catalog curves renormalized per Part A section 5.
- Standby draw derives from `self_discharge_frac_per_day` (Part A section 5 fold-in) and is metered AC-side per B.3.2.
- M1 uses ambient feed directly as cell temperature (F7/F8 are M4); PV-sourced hybrid charging never crosses the AC battery meter.

## Maintaining this file

Keep this file for knowledge useful to almost every future agent session in this project.
Do not repeat what the codebase already shows; point to the authoritative file or command instead.
Prefer rewriting or pruning existing entries over appending new ones.
When updating this file, preserve this bar for all agents and keep entries concise.
