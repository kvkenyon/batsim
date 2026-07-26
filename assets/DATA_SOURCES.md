# Data Sources (spec B.6.2 attribution requirement)

Part C's scenario config exposes `load_shape_version`; this file records
which dataset informs (or will inform) each load/PV shape table, and the
provenance class of every M1 table.

**M1 status: every table below is a synthetic engineering estimate**,
hand-calibrated to publicly known Texas residential magnitudes (RECS
annual totals, ERCOT seasonal peak timing). No ResStock, Pecan Street,
RECS, or NSRDB data is parsed at runtime (B.6.2: the runtime MUST NOT
parse ResStock CSVs on the hot path). The extraction/fitting pipeline is
M2+.

`load_shape_version` for the M1 tables: **`m1-synthetic-2026.07`**
(to be replaced by e.g. `resstock-2024.2-tx-v1` once the B.6.2 pipeline
lands).

## Load shape tables (`crates/batsim-core/src/load.rs`)

Reference home for all tables: 2400 sqft, 2.8 occupants, CentralAC,
Post2000, TX_Central. Table values are average kW by
{weekday, weekend} x {summer, winter, shoulder} x hour-of-day,
linear-interpolated within the hour.

| End use | M1 table provenance | Dataset that WILL inform it (role per B.6.2) |
| --- | --- | --- |
| HVAC (`HVAC_SHAPES`) | Estimated. Summer night-loaded shape peaking 16:00 (ERCOT 4CP coincidence target, B.6.3); winter morning resistance ramp. | **NREL ResStock** (end-use disaggregation, archetype shares, HVAC temperature response). Cross-checked against **RECS** Texas annual kWh quartiles (validation test, not runtime). |
| Water heat (`WATER_SHAPES`) | Estimated. Morning/evening draws on a standby baseline; winter x1.15, summer x0.92 (inlet temperature). | **ResStock** (DHW profiles); RECS totals check. |
| Plug/background (`PLUG_SHAPES`) | Estimated. Smooth background only (fridge cycling, electronics, standby); discrete spikes live in `R_app`. | **ResStock** (plug-load breakdown). |
| Lighting (`LIGHT_SHAPES`) | Estimated. Morning/evening peaks; seasonal rows scale with day length. | **ResStock**; RECS totals check. |
| Pool (`POOL_SHAPES`) | Estimated. Summer 08:00-16:00 window at 1.2 kW, shoulder halved, winter freeze-protection run; per-home +/-30 min offset. | **Pecan Street Dataport** (pool circuit schedules). |
| Appliance signatures (`SIGNATURES`, `ARRIVAL_*`) | Estimated. 12 fixed (power, duration) signatures, 0.3-4.5 kW, 1-90 min; Poisson rate ~8 events/day at reference occupancy, evening peak. | **Pecan Street Dataport** circuit-level data — primary source for spike signatures and ramp distributions (B.6.2). |
| Base residual `R_base` | AR(1), sigma = 60 W, 5-min correlation, 50 W vampire floor (B.6.3 values). | **Pecan Street** 1-min residual distributions (fitting target). |
| HVAC stochastic `R_hvac` | Duty-cycle model: shape x temperature multiplier / rated power; per-home period +/-10 % and phase +/-5 min (LoadPhase draws, B.6.3). Heat-pump aux strip +3-5 kW below 2 C. Cool setpoint 24 C / heat 20 C are mode constants. | **ResStock** duty-cycle statistics. |
| EV session model | Plug-in ~18:00 +/- 2 h daily (per-day substream draw), charge at `home_charge_kw` until `daily_miles x 0.28 kWh/mile`. M1: same schedule weekdays and weekends (simplification). | Scenario data (Part A `EvConfig`). |
| Min15 scaled noise | Single normal draw per 15-min block, sigma = 120 W (aggregate of `R_app` + `R_base` at 15-min scale, B.6.3 "shapes plus scaled noise"). | **Pecan Street** 15-min residual statistics. |
| Critical share | Constant per-home share of the last evaluation, U[0.25, 0.35] (B.6.4 default band; fridge + lights/plugs subset + network equipment). End-use-level critical table and backup-panel cap are M2+. | Scenario/Part A table (M2+). |
| Climate/vintage factors | Estimated. Zone (cool, heat): Gulf (1.10, 0.80), Central (1.00, 1.00), North (0.80, 1.45), West (1.05, 1.10). Vintage: pre-1980 x1.25, 1980-2000 x1.00, post-2000 x0.85. | **ResStock** archetype calibration. |

Plausibility anchors used for the M1 estimates (RECS 2020 Texas public
magnitudes): total 8-16 MWh/home-yr depending on archetype (measured M1
reference home: see `load::tests::annual_energy_within_band`), fleet
load factor 0.45-0.6 (B.6.3 mandatory band; see
`load::tests::fleet_of_200_load_factor_in_band`).

M1 simplifications recorded: climate zone enters only the HVAC scaling
(other end uses are zone-independent); civil time is fixed UTC-6 with no
DST (America/Chicago CDT ~= CST for load shapes); EV schedule identical
all days.

## PV model (`crates/batsim-core/src/pv.rs`)

| Component | Model | Citation / provenance |
| --- | --- | --- |
| Solar position | NOAA solar calculator series (Meeus-based; geom. mean longitude/anomaly, equation of center, apparent longitude, obliquity correction, equation of time). Accuracy <= 0.05 deg for 1950-2050 (B.7.2 PSA/NREL SPA-lite class). Pure-function, std f64 only. | NOAA Solar Calculator (Reda & Andreas class), Meeus, *Astronomical Algorithms*. |
| Extraterrestrial irradiance | G_sc = 1367 W/m^2 with eccentricity-correction day-angle series. | Spencer (1971); Iqbal (1983). |
| Clear-sky irradiance | **Hottel (1976)** beam transmittance, model A (23 km visibility standard atmosphere) at fixed 0.2 km altitude; **Liu & Jordan (1960)** diffuse transmittance; **Kasten & Young (1989)** airmass. Documented estimated built-in feed. | Hottel, Solar Energy 18 (1976); Liu & Jordan, Solar Energy 4(3) (1960); Kasten & Young, Applied Optics 28 (1989). |
| Operational irradiance feed | **NREL NSRDB PSM v3** (30-min/hourly GHI/DNI/DHI per site, TMY or scenario year), binary per-site series interpolated in true solar time. | M2+ scenario pipeline (B.7.1); the M1 clear-sky model stands in. |
| POA transposition | **Hay-Davies** (B.7.2 picks it over Perez), plus isotropic ground reflection with albedo 0.2 (PVWatts convention). | Hay & Davies (1980). |
| DC derate | PVWatts-style: gamma_pdc = -0.0035/C, T_cell = T_amb + (G_poa/1000) x 30 C. | B.7.2 exact values; NREL PVWatts. |
| System loss stack | Mismatch 2 %, DC wiring 2 %, connections 0.5 %, nameplate 1 %, LID 2 %, availability 1 % (fixed product 0.9179); monthly soiling table (<= 5 % worst month); per-home shading_factor [0, 0.3]. PV-inverter ~4 % applied downstream -> PVWatts-consistent ~14 % total. | PVWatts-consistent default stack (B.7.2), itemized. |
| Soiling | Monthly loss table, Central-Texas base, zone proxy from site lat/lon (West x1.4 capped 5 %, North x0.9, Gulf x0.8) because `PvConfig` carries no zone id (M1 deviation). | B.7.2 form `eta_soiling = 1 - 0.02 * soiling_factor`. |
| Cloud overlay | Markov sky-state chain (clear/partly/broken; per-season dwell times, fitted-order magnitudes only) + within-state AR(1) flicker (sigma <= 30 % in broken, 30 s correlation), multiplier clamped [0.2, 1.05], draws from `PvCloud` substream. State means fitted to stationary average ~1.0 (structural requirement of the 1.05 clamp). Energy neutrality: causal per-clock-hour tracking servo with state-aware anticipation + cross-hour gain loop (exact scheme in `PvArray::dc_power_w` rustdoc). Measured (30-day July run, Austin): mean \|hour error\| 0.62 %, worst hour 6.0 %, cumulative drift 0.26 % (spec: settlement intervals within +/-2 % on average). | B.7.5; transition matrices per zone are M2 (zone-collapsed in M1 — `PvConfig` has no zone id). Fleet cell correlation (`m = 0.6 m_cell + 0.4 m_local`) is M2+ (needs a scenario `cloud_cell` id). |

## Recorded M1 deviations (contract-level)

1. B.7.5 fleet cloud-cell correlation not implemented (`PvConfig` has no
   cell id; only the local process runs).
2. B.7.5 sky-state transition matrix is season-resolved but
   zone-collapsed (same reason).
3. Soiling zone is inferred from lat/lon, not a scenario zone id.
4. Load climate zone enters HVAC scaling only; other end uses are
   zone-independent in M1.
5. Fixed UTC-6 civil clock, no DST.
6. EV plug-in schedule identical on all days (B.6.3 specifies weekday
   commuters; weekend schedule unspecified).
7. Critical-loads split is a constant per-home share of total load
   (B.6.4 allows the constant-share model for M1); the end-use table and
   backup-panel cap are M2+.
