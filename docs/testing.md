# Testing

batsim's correctness story is layered: unit tests pin physics anchors, property tests defend
invariants over random inputs, golden traces snapshot end-to-end behavior per device model, an
RTE conformance gate ties the catalog to the engine, and a determinism gate makes every run
bit-reproducible. Over the HTTP API, server integration tests pin route behavior, schemathesis
contract tests pin the wire contract to the OpenAPI document, and a generated-client end-to-end
test drives a fleet through the real API. This guide covers how to run each layer, what it
defends, and where to add a test when behavior changes.

## The engineering loop

The toolchain is pinned to Rust 1.83.0 via `rust-toolchain.toml` (with `clippy` and `rustfmt`
components). Bump it deliberately, never incidentally.

```sh
cargo check --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Lint policy lives in the root `Cargo.toml` under `[workspace.lints]`: `missing_docs = "deny"`,
`unsafe_code = "forbid"`, and clippy pedantic at deny with a small curated allow list
(`cast_precision_loss`, `cast_possible_truncation`, `cast_sign_loss`, `doc_markdown`,
`module_name_repetitions`). `unwrap_used` and `expect_used` are denied in production code; every
test target and `#[cfg(test)]` module carries a scoped
`#![allow(clippy::unwrap_used, clippy::expect_used)]`, so tests may unwrap freely.

One timing note: the determinism gate steps 10 homes x 86,400 ticks three times and takes
about 2 minutes in a debug build. That is normal; do not mistake it for a hang. Everything else
in the suite is seconds.

## Suite map

| Layer | Location | What it is |
|---|---|---|
| Unit tests | `#[cfg(test)]` modules in `crates/batsim-core/src/{load,pv,battery,chemistry,inverter}.rs` and `crates/batsim-registry/src/{load,validate,system}.rs` | Physics anchors, catalog integrity, validation rules |
| Golden traces | `crates/batsim-core/tests/golden.rs` + `tests/golden/*.snap` | Per-model 48 h SOC traces, insta snapshots |
| Determinism gate | `crates/batsim-core/tests/determinism.rs` | Bit-identical reruns, serial vs parallel |
| Property tests | `crates/batsim-core/tests/prop_energy.rs` | Energy conservation, SOC window, clamping |
| RTE conformance | `crates/batsim-core/tests/rte_conformance.rs` | Measured vs catalog round-trip efficiency |
| Composition | `crates/batsim-core/tests/composition.rs` | Multi-device homes, coupling edge cases |
| Server API integration | `crates/batsim-server/tests/api.rs` | Route behavior, problem documents, idempotency, dispatch audit |
| Shared helpers | `crates/batsim-core/tests/common/mod.rs` | Austin site, golden epoch, standard home builders |

The integration tests share `tests/common/mod.rs`: an Austin, TX site (30.27, -97.74), the
golden epoch `2025-06-15T00:00:00Z`, a standard 2400 sqft load archetype, a no-cloud-noise PV
site, and `build_world` / `one_battery_system` helpers that compose a valid `SystemSpec` for any
catalog battery (adding the vendor-required hybrid inverter and controller when the model
demands them).

## API contract and end-to-end layers

Three gates pin the HTTP API to its document; `just ci` runs all of them:

- **Vendored-spec freshness** (`just spec`): `batsim --dump-openapi` must reproduce
  `api/openapi.json` exactly; CI diffs the two. Regenerate the vendored copy with the same
  command, never by hand.
- **Contract tests** (`just contract`): schemathesis runs its full check set against a fixture
  server's live `/openapi.json`. The per-operation exemptions and the excluded stream paths are
  documented in `schemathesis.toml` (the authoritative list) - the stream paths are infinite
  streams and an upgrade handshake, not request/response pairs.
- **Generated-client E2E** (`just e2e`): `examples/python-e2e/run.sh` generates a Python client
  from the live document, then creates a 100-home fleet, binds a scenario, dispatches, and reads
  telemetry through it.

## Golden SOC traces (`tests/golden.rs`)

For every standalone catalog battery (expansion packs are skipped: their
`continuous_discharge_power_kw` is 0), `golden_soc_traces` builds one home with PV and steps a
fixed 48 h scenario at dt = 1 s. The scenario exercises three regimes in sequence:

1. Overnight self-consumption: the battery discharges against the load.
2. Morning PV charge back to full.
3. At 20:00 UTC (tick `20 * 3600`, battery full), two scheduled dispatches land: switch to
   `ControlMode::Manual` and hold a 3 kW setpoint, driving SOC back down to the reserve floor.

Each snapshot records a JSON summary: `soc_samples_per_min` (per-minute SOC, rounded to 1e-6),
`final_soc`, and meter totals in kWh (`main_import_kwh`, `main_export_kwh`, `batt_import_kwh`,
`batt_export_kwh`, `pv_kwh`, `standby_kwh`). A second snapshot per model holds a SHA-256 over
the full per-tick truth series, so per-tick equivalence is exact while the samples and counters
make drift human-reviewable. Snapshots live in `tests/golden/` as
`golden__golden_soc_traces@<model>.snap` (summary) and `...-2.snap` (hash), 20 files for the 10
standalone models.

Regeneration rule: never hand-edit a snapshot. Regenerate only with

```sh
INSTA_UPDATE=always cargo test -p batsim-core --test golden
```

and review the diff like a physics change, because it is one. An unexplained SOC or meter diff
is a bug in the change, not in the snapshot.

## Determinism gate (`tests/determinism.rs`)

`determinism_check` builds a mixed fleet of 10 homes (one per standalone catalog battery,
cycled) on the golden epoch, seed `0xDEAD_BEEF`, with a diurnal-sine ambient feed (mean 30 C,
amplitude 6 C), and steps 24 h at 1 s ticks. It then hashes, with SHA-256, the serialized
`SimWorld` plus the full truth telemetry archive of every home, and asserts:

- two same-seed serial runs produce the identical digest, and
- a rayon-parallel run (`SimWorld::step_parallel`) produces the identical digest too.

This is the release gate for any engine change. The simulator's value proposition is that a
seed fully determines a run; if a change introduces HashMap iteration order, float-sensitive
reordering across threads, or unseeded randomness, this test is what catches it. Any PR that
touches stepping, dispatch, or the truth pipeline must leave it green. See
[architecture.md](architecture.md) for the stepping model it protects, and try
`cargo run -p batsim-core --example determinism_demo` for the same property in miniature.

## Property tests (`tests/prop_energy.rs`)

`energy_conservation_and_soc_window` is a proptest (64 cases) over random device parameters x
random setpoint sequences. Parameters: chemistry (LFP or NMC), coupling (AC-coupled,
DC-coupled hybrid, microinverter-based), usable energy 3-20 kWh, continuous power 2-12 kW
(peak 1.5x continuous), initial SOC 0-1. Setpoint sequences are 50-300 one-second steps mixing
full-range swings with adversarial holds at full charge, zero, and full discharge (up to 15 kW
either way). Three invariants are defended per tick:

1. **Setpoint clamping.** Realized terminal power never exceeds the pre-step dynamic limits
   (`max_discharge_w()` / `max_charge_w()`, captured before the step), so Thevenin sag and
   thermal derate can only shrink what the unit does, never grow it.
2. **Energy conservation.** The stored-energy delta equals the integral of *realized* terminal
   power through the efficiency stages: discharge removes `p_term / eta_dis` per tick, charge
   stores `|p_term| * eta_chg * eta_coul`. Tolerance is `1e-9 * usable_wh + 1e-6` Wh per tick.
   Evaluating against realized (not requested) power is the point: ramp limits, min on/off, and
   window clamps change what the unit actually did, and the identity must hold for that.
3. **SOC window.** SOC stays within `[0, 1]` (with 1e-9 slack) under all inputs, including
   charge-while-full and discharge-while-empty.

Cumulative charge/discharge counters are additionally asserted monotonic (non-negative) at the
end of each sequence.

## RTE conformance (`tests/rte_conformance.rs`)

`rte_conformance` is a catalog-vs-engine contract. Each standalone battery is run through a
standard profile: charge at 0.5 C (half the usable kWh in kW, clamped to the continuous charge
rating) for 2 h, rest 10 minutes, then discharge at 0.5 C to the SOC cutoff (with a 12 h
non-termination guard). The measured AC-path round-trip efficiency must land within 0.5
percentage points of the entry's declared `rte_ac_coupled`.

Path construction is coupling-aware. AC-coupled units are metered at their terminal (the
terminal *is* AC). DC-coupled hybrids are metered through their compatible catalog hybrid
inverter: grid charge is a double conversion (`AC * eta_hyb` into the pack, `DC / eta_hyb` back
out), and both directions use realized, not requested, power. Standby/tare draw is excluded
from the integral; it models gateway self-consumption, not the conversion path.

A standalone entry missing `rte_ac_coupled` fails the gate outright. For calibration work, an
ignored diagnostic prints measured vs declared per device:

```sh
cargo test -p batsim-core --test rte_conformance rte_report -- --ignored --nocapture
```

The registry-side mirror, `ac_path_rte_calibration_holds` in `crates/batsim-registry/src/load.rs`,
checks the same 0.5 pp bound analytically from the catalog curves
(`eta_chg x eta_coul x eta_dis`, times `eta_hyb^2` for hybrids at the 0.5 C power point), so a
curve edit that breaks calibration fails before the engine even runs. See
[device-registry.md](device-registry.md) for the catalog schema this calibrates against.

## Composition tests (`tests/composition.rs`)

Composition covers device-combination edge cases at the home boundary, each named after the
failure it prevents:

- `integrated_inverter_battery_gets_a_synthesized_ac_path` - a Powerwall 3 document with an
  empty `inverters[]` still gets its integrated hybrid synthesized, and a 3 kW manual discharge
  shows up as battery AC power at the panel (not lost on the DC bus).
- `inverter_quantity_aggregates_rated_ac` - two inverters double rated AC, and fleet power P
  runs at the per-unit efficiency of P/2.
- `duplicate_same_topology_inverter_entries_are_rejected` and
  `hybrid_quantity_below_integrated_head_units_is_rejected` - malformed inverter line items fail
  at `build_devices` with a named error.
- `dc_ac_ratio_caps_the_pv_path_and_books_the_overhang_as_pv_clipped` - a 14 kW array at ratio
  3.5 is capped at 4 kW AC, the cap binds, and the curtailed overhang lands on the
  `pv_clipped` meter; the same array at ratio 1.0 delivers strictly more.
- `shared_cap_curtails_the_command_not_the_pack` - PV-priority curtailment: with the hybrid
  saturated by PV at solar noon, a 5 kW discharge command is curtailed at the shared AC cap and
  the shortfall is booked to `batt_clipped`, but every watt that left the pack still reaches AC
  (curtailment happens before integration, never after).
- `mixed_string_pv_and_hybrid_never_converts_the_array_twice` - with a dedicated string PV
  inverter plus an integrated hybrid, AC out never exceeds DC in (the array is converted once),
  and PV still delivers real energy.
- `mixed_coupling_home_splits_the_setpoint_once` - a PW2 (AC terminal) plus a PW3 (DC terminal)
  under one 8 kW manual setpoint realize ~8 kW total, pro-rata, with both units carrying a
  share; the setpoint is split once, never applied per terminal class.
- `per_module_pv_inverter_scales_to_the_array` - a per-module microinverter (0.64 kW) named for
  an 8 kW array yields 13 units, not a 0.64 kW cap.
- `dispatch_scheduled_out_of_order_still_fires_on_time` - dispatches scheduled late-tick-first
  still fire exactly at their tick and hold past it.

## Registry tests

In `crates/batsim-registry/src`:

- **Loader integrity** (`load.rs`). `tampered_entry_fails_integrity_and_names_file` flips one
  byte in a copied entry (nameplate `14.0` -> `94.0`, still valid JSON) and requires an
  `Integrity` error naming the file; `tampered_catalog_hash_fails_integrity` corrupts
  `catalog_sha256` in the manifest and requires the same class of error.
  `embedded_catalog_loads_and_counts` pins the embedded catalog at 11 batteries;
  `spec_nameplate_values_and_provenance` pins nameplate values and their `Provenance` markers
  (`Spec` / `Estimated`) per entry, plus the rule that every efficiency curve is `Estimated`
  with at least 2 points.
- **Shadowing** (`load.rs`). `shadow_dir_overrides_one_entry_and_keeps_others` loads an external
  tree containing only a re-rated PW2: the shadow wins, the other 10 batteries survive, and the
  shadow is recorded in `RegistrySource::External`. `from_dir_without_manifest_synthesizes_one`
  builds a manifest from disk content; `load_without_shadow_dir_is_embedded` confirms the
  default is the embedded catalog.
- **Validation enumeration** (`validate.rs`). Every broken-entry rule reports *all* violations
  with field paths, not just the first: `broken_curve_is_enumerated` (too few points,
  non-ascending `x_kw`, efficiency outside [0, 1]), `bad_model_id_is_enumerated`,
  `inverted_soc_window_is_enumerated`, `usable_above_nameplate_is_enumerated`,
  `enphase_ceiling_rule_is_enforced` (continuous power bounded by microinverter count x per-unit
  power), `cross_reference_failures_are_enumerated`, `controller_and_preset_checks`, and
  `embedded_entries_pass_all_checks` as the positive control.
- **System documents** (`system.rs`). Home-system validation: unknown model ids are all
  enumerated, backup-capable systems require exactly one grid-forming controller, DC-coupled
  batteries require a compatible hybrid inverter, expansion-pack rules, SolarEdge hub capacity
  bounds, the Enphase microinverter ceiling, PV-inverter rules, per-line-item numeric bounds,
  and backup-path power resolution order.

See [device-registry.md](device-registry.md) for the loading and shadowing semantics under test.

## Unit test highlights (batsim-core)

- **Load** (`src/load.rs`). `annual_energy_within_band` integrates a full year at dt = 60 s and
  requires 8-16 MWh for the reference home; `fleet_of_200_load_factor_in_band` steps 200
  mixed-archetype homes over a July week and requires a fleet load factor of 0.45-0.60.
  `summer_afternoon_exceeds_overnight` (afternoon mean > 1.8x overnight) and
  `summer_peak_plausible_for_reference_home` (3-7 kW July-afternoon peak) pin the diurnal shape.
- **PV** (`src/pv.rs`). `solar_position_equinox_noon_elevation` requires culmination at
  `90 - lat` within 0.5 deg and azimuth due south within 1.5 deg;
  `solar_position_night_and_azimuth_quadrants` pins night elevation, morning/evening quadrants,
  and the 1320-1412 W/m^2 extraterrestrial band; `clear_sky_plausible_at_summer_noon` requires
  GHI 900-1100 W/m^2 at Austin summer noon.
- **Battery** (`src/battery.rs`). The `thevenin_sag_anchor_*` tests (mirrored at the pack level
  in `src/chemistry.rs`) anchor cold, empty deliverability: a PW3-shaped LFP unit at 5 % SOC and
  -5 C must deliver 40-60 % of its 11.5 kW nameplate, and full rating when warm at mid-SOC.
  `soc_accounting_split_efficiencies` and `energy_identity_matches_realized_power` pin the SOC
  bookkeeping the property test generalizes; `reserve_floor_grid_and_outage_release`,
  `ramp_slew_and_min_on_off_suppression`, and `expansion_packs_add_energy_only` cover control
  behavior. See [physics-models.md](physics-models.md) for the models these anchor.

## Adding a test for a new behavior

Pick the cheapest harness that defends the contract:

- **Unit test** for a physics anchor, formula, or boundary: add to the `#[cfg(test)]` module in
  the owning source file. Prefer anchors with physical meaning (a band, a quadrant, an exact
  efficiency) over asserting implementation details.
- **Property test** for an invariant that must hold for *all* inputs (conservation, clamping,
  windows): extend `prop_energy.rs` or add a sibling. If you find yourself writing a loop over
  representative values, that is a property test.
- **Golden trace** for end-to-end behavior of a device model or a scenario change: the snapshot
  diff *is* the review artifact, so regenerate with `INSTA_UPDATE=always` and read it.
- **Conformance gate** for a catalog change: `rte_conformance` and the registry's
  `ac_path_rte_calibration_holds` must both stay green; use the ignored `rte_report` diagnostic
  to pick calibrated curve points.
- **Composition test** for anything that only happens when devices share a home (caps, splits,
  double-counting).

Mechanics: test files and `#[cfg(test)]` modules already allow `unwrap`/`expect`; keep
production code unwrap-free. Any change to stepping, dispatch, or telemetry ordering must leave
the determinism gate green - budget the ~2 minute debug run. The four examples
(`single_home_trace`, `fleet_energy`, `catalog_browser`, `determinism_demo` via
`cargo run -p batsim-core --example <name>`) are useful smoke checks but are not a substitute
for the gates above.
