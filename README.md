# batsim

A deterministic residential battery fleet simulator for ERCOT (Texas)
territory, written in Rust.

batsim simulates homes equipped with real OEM battery systems - Tesla
Powerwall 2/3, Enphase IQ Battery, SolarEdge Home Battery, and sonnen -
so dispatch strategies can be developed and tested against realistic
fleet behavior without touching real hardware.

## What you can do today

batsim is currently a Rust library (two crates) with no network surface:

- **`batsim-core`** - the simulation engine. A virtual-clock tick loop
  (1-60 s steps) drives each home through a fixed per-tick pipeline:
  load, PV, dispatch, battery, inverter, metering, telemetry. Physics
  includes a split-efficiency battery SOC model with Thevenin voltage
  sag, LFP/NMC chemistry modules, load-dependent inverter efficiency,
  Texas residential load synthesis, and a solar position / clear-sky /
  cloud-variability PV pipeline.
- **`batsim-registry`** - a validated, integrity-checked catalog of 21
  OEM device entries (11 batteries, 4 controllers, 5 inverters, 1 PV
  preset) shipped as JSON and embedded into the binary. Every catalog
  value carries a provenance marker (`spec` or `estimated`).

Everything is deterministic: one master seed reproduces a run bit-for
bit, including under rayon parallelism. Conformance tests hold every
standalone catalog battery's measured round-trip efficiency within 0.5
percentage
points of its datasheet figure.

## Coming next

An OpenAPI-first HTTP API for creating homes, scheduling dispatch
actions, and reading telemetry, followed by ERCOT market integration,
outage simulation, thermal and degradation models, and vendor API
mimicry.

## Quickstart

Requires a Rust toolchain (pinned to 1.83.0 via `rust-toolchain.toml`).

```sh
# Simulate one Powerwall 3 home for 24 h and print its SOC trace
cargo run -p batsim-core --example single_home_trace

# Simulate a small fleet across device families and report energy
cargo run -p batsim-core --example fleet_energy

# Browse the device catalog
cargo run -p batsim-core --example catalog_browser

# Same seed twice: prove the results are bit-identical
cargo run -p batsim-core --example determinism_demo
```

Minimal library usage:

```rust
use batsim_core::battery::BatteryConfig;
use batsim_core::engine::{AmbientFeed, SimWorld};
use batsim_core::home::Home;
use batsim_core::time::SimClock;
use batsim_core::topology::{build_devices, HomeBuildConfig};
use batsim_registry::Registry;

let registry = Registry::embedded()?;
let mut world = SimWorld::new(
    SimClock::from_rfc3339("2025-06-15T00:00:00Z", 60)?,
    42, // master seed
    AmbientFeed::Constant(30.0),
)?;
// Compose a HomeSystem document, validate it, build devices, add a home.
// See examples/single_home_trace.rs for the full flow.
world.step_n(1440); // 24 h at 60 s ticks
```

## Documentation

- [Architecture guide](docs/architecture.md) - crates, tick pipeline,
  determinism design
- [Device registry guide](docs/device-registry.md) - catalog format,
  adding a device, provenance markers
- [Physics models guide](docs/physics-models.md) - battery, chemistry,
  inverter, load, and PV models with measured accuracy figures
- [Testing guide](docs/testing.md) - golden traces, determinism gate,
  property tests
- [Data sources](assets/DATA_SOURCES.md) - provenance of every load/PV
  shape table

## Development

```sh
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

The founding design document (still the build contract for upcoming
work) is
[`docs/residential-battery-fleet-simulator-spec.md`](docs/residential-battery-fleet-simulator-spec.md).
