# Data Sources

This file records which dataset informs (or will inform) each load/PV
shape table, and the provenance class of every current table, so the
numbers shipped in the simulator carry their attribution with them.
The planned HTTP API's scenario config exposes `load_shape_version` to
select among these tables.

**Current status: every table below is a synthetic engineering estimate**,
hand-calibrated to publicly known Texas residential magnitudes (RECS
annual totals, ERCOT seasonal peak timing). No ResStock, Pecan Street,
RECS, or NSRDB data is parsed at runtime; parsing external CSVs on the
hot path is deliberately out of scope. The extraction/fitting pipeline
is planned future work.

`load_shape_version` for the current tables: **`synthetic-2026.07`**
(to be replaced by e.g. `resstock-2024.2-tx-v1` once the planned
extraction pipeline lands).

## Load shape tables (`crates/batsim-core/src/load.rs`)

Reference home for all tables: 2400 sqft, 2.8 occupants, CentralAC,
Post2000, TX_Central. Table values are average kW by
{weekday, weekend} x {summer, winter, shoulder} x hour-of-day,
linear-interpolated within the hour.

| End use | Current table provenance | Dataset that WILL inform it |
| --- | --- | --- |
| HVAC (`HVAC_SHAPES`) | Estimated. Summer night-loaded shape peaking 16:00 (ERCOT 4CP coincidence target); winter morning resistance ramp. | **NREL ResStock** (end-use disaggregation, archetype shares, HVAC temperature response). Cross-checked against **RECS** Texas annual kWh quartiles (validation test, not runtime). |
| Water heat (`WATER_SHAPES`) | Estimated. Morning/evening draws on a standby baseline; winter x1.15, summer x0.92 (inlet temperature). | **ResStock** (DHW profiles); RECS totals check. |
| Plug/background (`PLUG_SHAPES`) | Estimated. Smooth background only (fridge cycling, electronics, standby); discrete spikes live in `R_app`. | **ResStock** (plug-load breakdown). |
| Lighting (`LIGHT_SHAPES`) | Estimated. Morning/evening peaks; seasonal rows scale with day length. | **ResStock**; RECS totals check. |
| Pool (`POOL_SHAPES`) | Estimated. Summer 08:00-16:00 window at 1.2 kW, shoulder halved, winter freeze-protection run; per-home +/-30 min offset. | **Pecan Street Dataport** (pool circuit schedules). |
| Appliance signatures (`SIGNATURES`, `ARRIVAL_*`) | Estimated. 12 fixed (power, duration) signatures, 0.3-4.5 kW, 1-90 min; Poisson rate ~8 events/day at reference occupancy, evening peak. | **Pecan Street Dataport** circuit-level data - primary source for spike signatures and ramp distributions. |
| Base residual `R_base` | AR(1), sigma = 60 W, 5-min correlation, 50 W vampire floor. | **Pecan Street** 1-min residual distributions (fitting target). |
| HVAC stochastic `R_hvac` | Duty-cycle model: shape x temperature multiplier / rated power; per-home period +/-10 % and phase +/-5 min (LoadPhase draws). Heat-pump aux strip +3-5 kW below 2 C. Temperature coupling: +10 %/degC above the 30 C summer reference and +8 %/degC below the 8 C winter reference. | **ResStock** duty-cycle statistics. |
| EV session model | Plug-in ~18:00 +/- 2 h daily (per-day substream draw), charge at `home_charge_kw` until `daily_miles x 0.28 kWh/mile`. Currently the same schedule on weekdays and weekends (simplification). | Scenario data (the device catalog's `EvConfig`). |
| Min15 scaled noise | Single normal draw per 15-min block, sigma = 120 W (aggregate of `R_app` + `R_base` at 15-min scale; a shapes-plus-scaled-noise construction). | **Pecan Street** 15-min residual statistics. |
| Critical share | Constant per-home share of the last evaluation, U[0.25, 0.35] (default band; fridge + lights/plugs subset + network equipment). End-use-level critical table and backup-panel cap are planned future work. | Scenario/catalog table (planned future work). |
| Climate/vintage factors | Estimated. Zone (cool, heat): Gulf (1.10, 0.80), Central (1.00, 1.00), North (0.80, 1.45), West (1.05, 1.10). Vintage: pre-1980 x1.25, 1980-2000 x1.00, post-2000 x0.85. | **ResStock** archetype calibration. |

Plausibility anchors used for the current estimates (RECS 2020 Texas
public magnitudes): total 8-16 MWh/home-yr depending on archetype
(measured on the current reference home: see
`load::tests::annual_energy_within_band`), fleet load factor 0.45-0.6
(the mandatory load-factor band; see
`load::tests::fleet_of_200_load_factor_in_band`).

Recorded simplifications of the current tables: climate zone enters only
the HVAC scaling (other end uses are zone-independent); civil time is
fixed UTC-6 with no DST (America/Chicago CDT ~= CST for load shapes);
EV schedule identical all days.

## PV model (`crates/batsim-core/src/pv.rs`)

| Component | Model | Citation / provenance |
| --- | --- | --- |
| Solar position | NOAA solar calculator series (Meeus-based; geom. mean longitude/anomaly, equation of center, apparent longitude, obliquity correction, equation of time). Accuracy <= 0.05 deg for 1950-2050 (PSA/NREL SPA-lite accuracy class). Pure-function; transcendentals via the libm-backed `math` module (cross-platform bit-exact). | NOAA Solar Calculator (Reda & Andreas class), Meeus, *Astronomical Algorithms*. |
| Extraterrestrial irradiance | G_sc = 1367 W/m^2 with eccentricity-correction day-angle series. | Spencer (1971); Iqbal (1983). |
| Clear-sky irradiance | **Hottel (1976)** beam transmittance, model A (23 km visibility standard atmosphere) at fixed 0.2 km altitude; **Liu & Jordan (1960)** diffuse transmittance; **Kasten & Young (1989)** airmass. Documented estimated built-in feed. | Hottel, Solar Energy 18 (1976); Liu & Jordan, Solar Energy 4(3) (1960); Kasten & Young, Applied Optics 28 (1989). |
| Operational irradiance feed | **NREL NSRDB PSM v3** (30-min/hourly GHI/DNI/DHI per site, TMY or scenario year), binary per-site series interpolated in true solar time. | Planned scenario pipeline; the built-in clear-sky model stands in for now. |
| POA transposition | **Hay-Davies** (chosen over Perez), plus isotropic ground reflection with albedo 0.2 (PVWatts convention). | Hay & Davies (1980). |
| DC derate | PVWatts-style: gamma_pdc = -0.0035/C, T_cell = T_amb + (G_poa/1000) x 30 C. | NREL PVWatts values. |
| System loss stack | Mismatch 2 %, DC wiring 2 %, connections 0.5 %, nameplate 1 %, LID 2 %, availability 1 % (fixed product 0.9179); monthly soiling table (<= 5 % worst month); per-home shading_factor [0, 0.3]. PV-inverter ~4 % applied downstream -> PVWatts-consistent ~14 % total. | PVWatts-consistent default stack, itemized. |
| Soiling | Monthly loss table, Central-Texas base, zone proxy from site lat/lon (West x1.4 capped 5 %, North x0.9, Gulf x0.8) because `PvConfig` carries no zone id (a current simplification). | Model form `eta_soiling = 1 - 0.02 * soiling_factor`. |
| Cloud overlay | Markov sky-state chain (clear/partly/broken; per-season dwell times, fitted-order magnitudes only) + within-state AR(1) flicker (sigma <= 30 % in broken, 30 s correlation), multiplier clamped [0.2, 1.05], draws from `PvCloud` substream. State means fitted to stationary average ~1.0 (structural requirement of the 1.05 clamp). Energy neutrality: causal per-clock-hour tracking servo with state-aware anticipation + cross-hour gain loop (exact scheme in `PvArray::dc_power_w` rustdoc). Measured (30-day July run, Austin): mean \|hour error\| 0.62 %, worst hour 6.0 %, cumulative drift 0.26 % (target: settlement intervals within +/-2 % on average). | Transition matrices per zone are planned (currently zone-collapsed - `PvConfig` has no zone id). Fleet cell correlation (`m = 0.6 m_cell + 0.4 m_local`) is planned future work (needs a scenario `cloud_cell` id). |

## Recorded deviations of the current engine

1. Fleet cloud-cell correlation is not implemented (`PvConfig` has no
   cell id; only the local process runs).
2. The sky-state transition matrix is season-resolved but
   zone-collapsed (same reason).
3. Soiling zone is inferred from lat/lon, not a scenario zone id.
4. Load climate zone enters HVAC scaling only; other end uses are
   zone-independent.
5. Fixed UTC-6 civil clock, no DST.
6. EV plug-in schedule identical on all days (weekday commuters are the
   modeled case; the weekend schedule is unspecified).
7. Critical-loads split is a constant per-home share of total load
   (the constant-share model is the sanctioned simplification); the
   end-use table and backup-panel cap are planned future work.
