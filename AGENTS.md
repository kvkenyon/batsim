# Project agent memory

batsim is an ERCOT-only residential battery fleet simulator written in Rust: physics-faithful virtual Tesla Powerwall 2/3, Enphase IQ Battery, SolarEdge Home Battery, and sonnen systems. Today it ships as two library crates; an OpenAPI-first HTTP API for dispatch-strategy testing is planned next. The authoritative build brief for upcoming work is `docs/residential-battery-fleet-simulator-spec.md` - follow it; do not duplicate its content here. Human-facing docs live in `docs/` (architecture, device registry, physics models, testing); runnable usage demos live in `crates/batsim-core/examples/`.

## Implementation state

Complete: crates `batsim-core` (tick engine, battery/inverter/load/PV physics, home pipeline) and `batsim-registry` (21-entry JSON catalog under `registry/`, integrity-checked loader, HomeSystem composer). Not started: HTTP API server, ERCOT market integration, outages, thermal, degradation, vendor mimicry.

## Engineering loop

`cargo check --workspace` -> `cargo clippy --workspace --all-targets -- -D warnings` (pedantic denied via workspace lints; curated allows documented in root `Cargo.toml`) -> `cargo test --workspace`. Toolchain pinned 1.83.0 via `rust-toolchain.toml`; dep versions must stay 1.83-compatible (check edition2024 conflicts when bumping). Regenerate goldens only with `INSTA_UPDATE=always cargo test -p batsim-core --test golden` and review the diff. The determinism test takes ~2 min in debug.

## Binding design decisions (documented in code)

- `ac_to_dc` is conservation-true (DC delivered = AC x eta); dividing by efficiency on the charge path would invent energy.
- DC-coupled `rte_ac_coupled` = battery-curve product x eta_hyb^2 (grid charge on a hybrid is a double conversion); catalog curves are renormalized so measured round-trip efficiency matches each datasheet figure.
- Standby draw derives from `self_discharge_frac_per_day` and is metered AC-side.
- The ambient feed stands in directly as cell temperature (thermal and degradation models are future work); PV-sourced hybrid charging never crosses the AC battery meter.
- `pv.dc_ac_ratio` is applied as the PV path's AC cap (`kw_dc / dc_ac_ratio`), tighter than the inverter nameplate; the overhang is `pv_clipped`.
- A `DCCoupledHybrid` battery with `integrated_inverter == true` and no declared hybrid gets its `InverterUnit` synthesized from the compatible catalog entry (the PW3 *is* its own hybrid); an explicitly declared hybrid must carry `quantity >= ` the integrated head-unit count or composition errors.
- Inverter `quantity` aggregates by scaling both the AC rating and the efficiency-curve x-axis by N, i.e. N units share the flow equally. Per-module PV units (microinverter / battery-integrated ratings) scale to the array DC nameplate instead of capping it at one unit.
- PV priority at a shared hybrid inverter is enforced in the battery stage by curtailing the hybrid *discharge command* to the AC headroom PV leaves (`batt_clipped`); the inverter stage then treats realized pack DC as non-negotiable and curtails only PV, which is losslessly curtailable at the MPPT. Clip counters are attributed by actual bus share, never by the config flag.
- Every transcendental on a simulation path goes through `batsim-core/src/math.rs` (the `libm` crate), never the `f64` intrinsic methods: platform libms differ by an ulp and the golden traces hash every tick. Correctly rounded ops (`sqrt`, `ceil`, `floor`, `round`, `abs`) stay intrinsic. Golden snapshots are therefore bit-exact across macOS/Linux; regressions here fail CI on the other platform.

## Maintaining this file

Keep this file for knowledge useful to almost every future agent session in this project.
Do not repeat what the codebase already shows; point to the authoritative file or command instead.
Prefer rewriting or pruning existing entries over appending new ones.
When updating this file, preserve this bar for all agents and keep entries concise.
