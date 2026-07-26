//! AC-bus energy-conservation property tests for a composed home: PV
//! array, battery fleet, inverters, and load, stepped as a full
//! `SimWorld` at dt = 1 s under adversarial dispatch.
//!
//! Every term below is a REALIZED value from the per-tick truth record
//! (or a per-tick delta of a cumulative meter) - never a requested
//! setpoint. Ramp limits, SOC-window clamps, curtailment, and clipping
//! all make realized power differ from requested power; the balances
//! must hold for what the home actually did.
//!
//! Invariants defended, per tick (not merely per run):
//!
//! 1. Main-panel bus balance. The metering stage closes every tick with
//!    `p_grid = p_load - p_pv_ac - p_batt_ac + p_standby`: PV output and
//!    battery discharge feed the bus; the load, the battery/controller
//!    standby draw (booked as extra AC-side consumption), battery
//!    charging, and grid export drain it. Splitting the same signed
//!    terms by direction, the realized powers into the bus equal the
//!    realized powers out:
//!
//!    ```text
//!    p_pv_ac + max(p_batt_ac, 0) + max(p_grid, 0)
//!        == p_load + p_standby + max(-p_batt_ac, 0) + max(-p_grid, 0)
//!    ```
//!
//!    The signed bookkeeping form is the pipeline's own arithmetic on
//!    the same recorded terms, so it must reproduce bit-exactly; the
//!    direction-split form only reorders those additions, so it carries
//!    a 1e-9 relative tolerance for summation-order rounding.
//!
//! 2. PV conversion stage: realized PV AC out never exceeds realized PV
//!    DC in (the stage cannot create energy). When the array has a
//!    dedicated string inverter, the per-tick clipped-DC (delta of the
//!    clip counter) plus AC out also never exceeds DC in: the remainder
//!    is conversion heat, which is physically non-negative.
//!
//! 3. Hybrid DC-bus boundary (DC-coupled compositions whose array lands
//!    on the shared inverter's MPPTs): let the bus DC be PV DC plus the
//!    summed battery terminal DC. When the bus nets positive, the AC
//!    admitted from it never exceeds the bus DC (one conversion, eta <=
//!    1, any overflow is curtailed upstream or clipped). When the bus
//!    nets negative, the AC draw through the inverter covers the DC the
//!    battery absorbed, the difference being non-negative loss.
//!
//! 4. SOC window: mean SOC stays within [0, 1] under charge-while-full,
//!    discharge-while-empty, and zero-crossing dither dispatch.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::LazyLock;

use batsim_core::battery::BatteryConfig;
use batsim_core::dispatch::{ControlMode, DispatchAction, ScheduledDispatch};
use batsim_core::engine::{AmbientFeed, SimWorld};
use batsim_core::home::Home;
use batsim_core::telemetry::HomeTruth;
use batsim_core::time::SimClock;
use batsim_core::topology::{build_devices, HomeBuildConfig};
use batsim_registry::{Coupling, Registry};
use proptest::prelude::*;

/// Timestep for every run: one second, so a meter's per-tick energy
/// delta (Wh) times 3600 is that tick's average power (W).
const DT_S: u32 = 1;
/// Ticks per property run: four simulated minutes.
const TICKS: u64 = 240;
/// Relative tolerance for identities that reorder the pipeline's float
/// additions (summation-order rounding only).
const REL_TOL: f64 = 1e-9;
/// Absolute tolerance floor (W) so near-silent ticks still pass.
const ABS_TOL_W: f64 = 1e-6;

/// Catalog batteries spanning every coupling: AC-coupled, DC-coupled
/// hybrid, and microinverter-based (AC-terminal).
const CATALOG_MODELS: &[&str] = &[
    "tesla.powerwall_2",
    "tesla.powerwall_3",
    "sonnen.sonnenbatterie_10_ac",
    "sonnen.sonnenbatterie_10_hybrid",
    "sonnen.ecolinx",
    "sonnen.sonnencore_plus",
    "enphase.iq_battery_5p",
    "solaredge.home_battery_400v",
];

static REGISTRY: LazyLock<Registry> =
    LazyLock::new(|| Registry::embedded().expect("embedded registry"));

fn registry() -> &'static Registry {
    &REGISTRY
}

/// Tolerance for one comparison: relative on the larger side, floored.
fn tol_w(scale: f64) -> f64 {
    REL_TOL.mul_add(scale.abs(), ABS_TOL_W)
}

/// One scripted dispatch action, with setpoints expressed as fractions
/// of the model's nameplate so strategies stay model-independent.
#[derive(Debug, Clone, Copy)]
enum Cmd {
    /// Switch control mode.
    Mode(ControlMode),
    /// Manual setpoint as a nameplate fraction: positive fractions scale
    /// with the discharge rating, negative with the charge rating.
    Setpoint(f64),
    /// Backup-reserve floor (fraction of usable).
    Reserve(f64),
}

/// Adversarial block archetypes appended to the random event stream.
#[derive(Debug, Clone, Copy)]
enum BlockKind {
    /// Request full charge for the whole run (charges while full).
    ChargeHold,
    /// Request full discharge for the whole run (discharges while empty).
    DischargeHold,
    /// Alternate small charge/discharge setpoints every tick.
    Dither,
    /// Flip between full discharge and full charge every fifteen ticks.
    FullSwing,
}

fn arb_script() -> impl Strategy<Value = Vec<(u64, Cmd)>> {
    let mode = prop::sample::select(vec![
        ControlMode::Idle,
        ControlMode::Manual,
        ControlMode::SelfConsumption,
        ControlMode::BackupReserveHold,
    ])
    .prop_map(Cmd::Mode);
    let setpoint =
        prop_oneof![Just(-1.2), Just(1.2), Just(0.0), (-1.0..1.0f64),].prop_map(Cmd::Setpoint);
    let reserve = (0.0..0.8f64).prop_map(Cmd::Reserve);
    let events = prop::collection::vec((0..TICKS, prop_oneof![mode, setpoint, reserve]), 2..10);
    let block = (
        prop::sample::select(vec![
            BlockKind::ChargeHold,
            BlockKind::DischargeHold,
            BlockKind::Dither,
            BlockKind::FullSwing,
        ]),
        0..TICKS,
        20..50u64,
        0.01..0.1f64,
    )
        .prop_map(|(kind, t0, len, frac)| {
            let t0 = t0.min(TICKS.saturating_sub(len + 1));
            let mut v = vec![(t0, Cmd::Mode(ControlMode::Manual))];
            match kind {
                BlockKind::ChargeHold => v.push((t0, Cmd::Setpoint(-1.2))),
                BlockKind::DischargeHold => v.push((t0, Cmd::Setpoint(1.2))),
                BlockKind::Dither => {
                    for k in 0..len {
                        let sp = if k % 2 == 0 { frac } else { -frac };
                        v.push((t0 + k, Cmd::Setpoint(sp)));
                    }
                }
                BlockKind::FullSwing => {
                    for k in 0..(len / 15).max(2) {
                        let sp = if k % 2 == 0 { 1.2 } else { -1.2 };
                        v.push((t0 + k * 15, Cmd::Setpoint(sp)));
                    }
                }
            }
            v
        });
    (events, block).prop_map(|(mut events, block)| {
        events.extend(block);
        events
    })
}

/// Turn fractional setpoints into watts against the model's nameplates.
fn materialize(script: &[(u64, Cmd)], max_dis_w: f64, max_chg_w: f64) -> Vec<ScheduledDispatch> {
    script
        .iter()
        .map(|&(execute_at_tick, cmd)| {
            let action = match cmd {
                Cmd::Mode(mode) => DispatchAction::SetMode(mode),
                Cmd::Setpoint(frac) => DispatchAction::SetManualSetpoint(if frac >= 0.0 {
                    frac * max_dis_w
                } else {
                    frac * max_chg_w
                }),
                Cmd::Reserve(frac) => DispatchAction::SetReserve(frac),
            };
            ScheduledDispatch {
                execute_at_tick,
                action,
            }
        })
        .collect()
}

/// Assert every per-tick invariant on one realized truth record.
///
/// `clip_p_w` is this tick's clipped-PV power (clip-counter delta as
/// watts); it is only consulted when `string_pv` marks a composition
/// whose array converts at a dedicated string inverter. `hybrid_bus`
/// marks a DC-coupled composition whose array lands on the shared
/// inverter's MPPTs.
fn assert_tick(
    t: &HomeTruth,
    hybrid_bus: bool,
    string_pv: bool,
    clip_p_w: f64,
    ctx: &str,
) -> Result<(), TestCaseError> {
    let tick = t.tick;

    // Invariant 1, bookkeeping form: the telemetry stage derives p_grid
    // from the other four recorded terms with exactly this float
    // expression, so the recomputation must match bit-exactly.
    let grid_check = t.p_load_w - t.p_pv_ac_w - t.p_batt_ac_w + t.p_standby_w;
    prop_assert!(
        (t.p_grid_w - grid_check).abs() <= 0.0,
        "{ctx} tick {tick}: grid bookkeeping not exact: recorded {} vs derived {grid_check}",
        t.p_grid_w
    );

    // Invariant 1, direction-split form: the same summands as the
    // bookkeeping identity but added in a different order, so a 1e-9
    // relative tolerance covers reorder rounding (a few ULPs of the
    // largest term) and nothing more.
    let feed = t.p_pv_ac_w + t.p_batt_ac_w.max(0.0) + t.p_grid_w.max(0.0);
    let drain = t.p_load_w + t.p_standby_w + (-t.p_batt_ac_w).max(0.0) + (-t.p_grid_w).max(0.0);
    let tol = tol_w(feed.max(drain));
    prop_assert!(
        (feed - drain).abs() <= tol,
        "{ctx} tick {tick}: bus balance violated: in {feed} W vs out {drain} W (tol {tol} W)"
    );

    // Physical sign sanity on realized terms.
    prop_assert!(
        t.p_load_w >= 0.0,
        "{ctx} tick {tick}: negative load {}",
        t.p_load_w
    );
    prop_assert!(
        t.p_standby_w >= 0.0,
        "{ctx} tick {tick}: negative standby {}",
        t.p_standby_w
    );
    prop_assert!(
        t.p_pv_dc_w >= 0.0,
        "{ctx} tick {tick}: negative PV DC {}",
        t.p_pv_dc_w
    );
    prop_assert!(
        t.p_pv_ac_w >= 0.0,
        "{ctx} tick {tick}: negative PV AC {}",
        t.p_pv_ac_w
    );

    // Invariant 2: the PV stage cannot deliver more AC than DC in.
    let tol = tol_w(t.p_pv_dc_w);
    prop_assert!(
        t.p_pv_ac_w <= t.p_pv_dc_w + tol,
        "{ctx} tick {tick}: PV stage creates energy: {} W AC from {} W DC",
        t.p_pv_ac_w,
        t.p_pv_dc_w
    );
    if string_pv {
        // Dedicated string inverter: DC in equals AC out plus heat plus
        // clipped DC, and heat is non-negative, so AC + clip <= DC.
        let residual = t.p_pv_dc_w - t.p_pv_ac_w - clip_p_w;
        prop_assert!(
            residual >= -tol,
            "{ctx} tick {tick}: string-PV stage loses track of {residual} W: dc {} vs ac {} + clip {clip_p_w}",
            t.p_pv_dc_w,
            t.p_pv_ac_w
        );
    }

    // Invariant 3: hybrid DC bus cannot create energy in either
    // direction. The bus DC is summed in the pipeline's own order
    // (PV first, then per-unit terminal DC), so reproduction is exact.
    if hybrid_bus {
        let batt_dc: f64 = t.units.iter().map(|u| u.p_term_w).sum();
        let bus_dc = t.p_pv_dc_w + batt_dc;
        let tol = tol_w(bus_dc);
        if bus_dc > 0.0 {
            let ac_out = t.p_pv_ac_w + t.p_batt_ac_w;
            prop_assert!(
                t.p_batt_ac_w >= 0.0,
                "{ctx} tick {tick}: discharging hybrid bus but battery AC is {}",
                t.p_batt_ac_w
            );
            prop_assert!(
                ac_out >= 0.0 && ac_out <= bus_dc + tol,
                "{ctx} tick {tick}: hybrid bus creates energy: {ac_out} W AC from {bus_dc} W DC"
            );
        } else if bus_dc < 0.0 {
            // Deficit bus: PV is fully absorbed by the battery DC-DC
            // path and the inverter draws the remainder from AC.
            prop_assert!(
                t.p_pv_ac_w.abs() <= 0.0,
                "{ctx} tick {tick}: deficit hybrid bus still admits PV AC {}",
                t.p_pv_ac_w
            );
            prop_assert!(
                t.p_batt_ac_w <= 0.0,
                "{ctx} tick {tick}: charging hybrid bus but battery AC is {}",
                t.p_batt_ac_w
            );
            let ac_draw = -t.p_batt_ac_w;
            prop_assert!(
                ac_draw >= -bus_dc - tol,
                "{ctx} tick {tick}: hybrid charge loses energy: {ac_draw} W AC for {} W DC",
                -bus_dc
            );
        } else {
            prop_assert!(
                t.p_pv_ac_w.abs() <= 0.0 && t.p_batt_ac_w.abs() <= 0.0,
                "{ctx} tick {tick}: idle hybrid bus but AC flows: pv {} batt {}",
                t.p_pv_ac_w,
                t.p_batt_ac_w
            );
        }
    }

    // Invariant 4: SOC window under adversarial dispatch.
    prop_assert!(
        (-1e-9..=1.0 + 1e-9).contains(&t.soc_mean),
        "{ctx} tick {tick}: soc out of window: {}",
        t.soc_mean
    );
    Ok(())
}

/// Step the world `TICKS` times with the dispatch script queued,
/// asserting every invariant on every tick of home 0.
fn run_and_assert(
    world: &mut SimWorld,
    script: &[ScheduledDispatch],
    hybrid_bus: bool,
    string_pv: bool,
    ctx: &str,
) -> Result<(), TestCaseError> {
    for cmd in script {
        world.dispatch(0, *cmd).expect("home 0 exists");
    }
    let mut prev_clip_wh = world.home(0).expect("home 0").meters().pv_clipped.wh;
    for _ in 0..TICKS {
        world.step();
        let home = world.home(0).expect("home 0");
        let truth = home.truth().last().expect("truth recorded");
        let clip_wh = home.meters().pv_clipped.wh;
        let clip_p_w = (clip_wh - prev_clip_wh) * 3600.0 / f64::from(DT_S);
        prev_clip_wh = clip_wh;
        assert_tick(truth, hybrid_bus, string_pv, clip_p_w, ctx)?;
    }
    Ok(())
}

/// Coupling-derived composition flags for the model under test.
fn composition(model_id: &str, with_pv: bool) -> (bool, bool) {
    let model = registry().battery(model_id).expect("catalog battery");
    let hybrid_bus = matches!(model.coupling, Coupling::DCCoupledHybrid);
    (hybrid_bus, with_pv && !hybrid_bus)
}

/// Nameplate charge/discharge powers (W) sizing the full-swing setpoints.
fn nameplate_w(model_id: &str) -> (f64, f64) {
    let model = registry().battery(model_id).expect("catalog battery");
    (
        model.continuous_discharge_power_kw.value * 1000.0,
        model.continuous_charge_power_kw.value * 1000.0,
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Catalog compositions through the shared test world builder:
    /// midnight epoch, default initial SOC and reserve, PV priority on.
    #[test]
    fn bus_balance_catalog_compositions(
        model_id in prop::sample::select(CATALOG_MODELS),
        with_pv in any::<bool>(),
        seed in any::<u64>(),
        script in arb_script(),
    ) {
        let (max_dis_w, max_chg_w) = nameplate_w(model_id);
        let (hybrid_bus, string_pv) = composition(model_id, with_pv);
        let mut world = common::build_world(registry(), model_id, 1, seed, with_pv, true);
        let ctx = format!("catalog {model_id} pv={with_pv} seed={seed:#x}");
        let script = materialize(&script, max_dis_w, max_chg_w);
        run_and_assert(&mut world, &script, hybrid_bus, string_pv, &ctx)?;
    }

    /// Adversarial state corners: initial SOC at the window edges,
    /// reserve floors, random start hour (PV ramp and midday clipping),
    /// randomized ambient and PV-priority, plus the dispatch blocks.
    #[test]
    fn bus_balance_adversarial_state(
        (
            model_id,
            with_pv,
            seed,
            start_hour,
            initial_soc,
            reserve,
            pv_priority,
            t_mean_c,
            t_amp_c,
            oversize_array,
        ) in (
            prop::sample::select(CATALOG_MODELS),
            any::<bool>(),
            any::<u64>(),
            0..24u64,
            prop_oneof![Just(0.0), Just(0.02), Just(0.5), Just(0.98), Just(1.0), 0.0..1.0f64],
            prop_oneof![Just(0.0), Just(0.1), Just(0.2), Just(0.5), Just(0.8)],
            any::<bool>(),
            5.0..38.0f64,
            0.0..8.0f64,
            any::<bool>(),
        ),
        script in arb_script(),
    ) {
        let reg = registry();
        let (max_dis_w, max_chg_w) = nameplate_w(model_id);
        let (hybrid_bus, string_pv) = composition(model_id, with_pv);
        let mut spec = common::one_battery_system(reg, model_id, with_pv);
        spec.system.batteries[0].initial_soc_frac = initial_soc;
        spec.system.batteries[0].reserve_frac = reserve;
        if oversize_array {
            // Double the array so the inverter's AC rating (not the
            // DC/AC-ratio cap) is the bottleneck near solar noon: the
            // standard fixture array tops out below its clip threshold,
            // so this corner is what exercises clip booking.
            if let Some(pv) = &mut spec.system.pv {
                pv.kw_dc *= 2.0;
            }
        }
        let base_epoch = SimClock::from_rfc3339(common::GOLDEN_EPOCH, DT_S)
            .expect("epoch parses")
            .epoch_s();
        // Whole-hour offsets keep the epoch 5-minute aligned.
        let clock = SimClock::new(base_epoch + start_hour * 3600, DT_S).expect("aligned epoch");
        let mut world = SimWorld::new(
            clock,
            seed,
            AmbientFeed::DiurnalSine {
                mean_c: t_mean_c,
                amplitude_c: t_amp_c,
            },
        )
        .expect("world");
        let build_cfg = HomeBuildConfig {
            load: common::std_load_config(),
            pv_site: with_pv.then(common::std_pv_site),
            battery: BatteryConfig::default(),
            pv_priority,
        };
        let devices = build_devices(&spec, reg, &build_cfg, seed, 0).expect("devices build");
        world.add_home(Home::new(devices, true));
        let ctx = format!(
            "adversarial {model_id} pv={with_pv} soc0={initial_soc} reserve={reserve} \
             hour={start_hour} pv_priority={pv_priority} oversize_array={oversize_array} seed={seed:#x}"
        );
        let script = materialize(&script, max_dis_w, max_chg_w);
        run_and_assert(&mut world, &script, hybrid_bus, string_pv, &ctx)?;
    }
}
