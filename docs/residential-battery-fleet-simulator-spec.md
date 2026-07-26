Residential Battery Fleet Simulator — Implementation Specification
Codename: batsim
Scope: ERCOT only (explicitly no other ISOs)
Stack: Rust (single binary), OpenAPI 3.1-first REST + streaming API
Audience: AI implementation agents (this document is the complete build brief)
Version: 1.0 — 2026-07-26
Status: Approved for implementation
0. Executive Summary
This document specifies a residential battery fleet simulator that stands up virtual homes
with faithful, physics-based emulations of real OEM storage systems — Tesla Powerwall 2/3,
Enphase IQ Battery 5P/10/10C, SolarEdge Home Battery 400V, and sonnen ecoLinx / Core+ /
Batterie 10 — including their inverters, coupling topologies (AC-coupled vs DC-coupled
hybrid vs microinverter-based), chemistries (LFP vs NMC), and vendor cloud/local API shapes.
An external system (e.g., a dispatch-strategy optimizer) connects over a simple,
self-describing HTTP API, assembles fleets of thousands of virtual homes, accelerates time,
injects ERCOT price/ancillary/outage scenarios, dispatches the fleet, and reads back
telemetry and settlement results — as if the batteries were live in ERCOT.
The four non-negotiable design commitments:
Real behavior, not mocks. SOC dynamics, efficiency curves, thermal derating,
degradation, inverter clipping, grid-forming backup transitions, and vendor-realistic
telemetry noise are simulated per-device (Part B), parameterized by a declarative,
versioned device registry (Part A).
API is the only interface. REST + SSE/WebSocket streaming; the OpenAPI 3.1 document
is generated from code and is the single source of truth; clients for any language are
generated on the fly, never hand-written (Part C).
ERCOT only. 5-minute real-time settlement prices with ORDC scarcity adders, DAM,
RRS/ECRS/Non-Spin ancillary semantics, 4CP awareness, ADER-style aggregate dispatch, and
a seeded synthetic price-scenario generator for stress testing (Part D).
Deterministic and fast. Same seed + same inputs ⇒ bit-identical state hashes;
target 10,000 homes at 1-second ticks running ≥10× realtime on one machine (Parts B, C).
Document Map
Table
Part   Contents
Part A   OEM hardware registry & system topology — device catalog, AC/DC coupling, JSON schemas
Part B   Simulation engine — time engine, battery/inverter/thermal/degradation physics, load & PV generation, outage behavior, telemetry model
Part C   API & Rust architecture — workspace layout, endpoint surface, vendor-API mimicry, concurrency, testing, DevEx
Part D   ERCOT market integration — market model, data sources, synthetic prices, settlement, worked dispatch example
0.1 Feature Set
Priority tiers: P0 = MVP (simulator is useful), P1 = v1 (production-grade),
P2 = v2 (differentiating). Each feature references its spec section.
Simulation Core
Table
#   Feature   Tier   Spec
F1   Virtual clock with 1-s device ticks, run-until/step/speed-multiplier control   P0   B.1, C.3 /v1/sim
F2   Bit-identical determinism (seeded ChaCha stream splitting; snapshot hash CI)   P0   B.1, C.7
F3   Battery SOC model: separate charge/discharge efficiency, piecewise efficiency curves from registry   P0   B.2, A.4
F4   Thevenin equivalent (OCV + R_int) so power limits sag with SOC/temperature/aging   P1   B.2
F5   Chemistry modules: LFP (0 °C charge block, flat OCV) vs NMC (high-SOC calendar penalty)   P0   B.2
F6   Inverter model: load-dependent efficiency, standby draw, clipping, AC/DC path losses   P0   B.3, A.3
F7   Thermal model with Texas ambient feed and temperature derating   P1   B.4
F8   Degradation: calendar + cycle aging, SOH telemetry, warranty throughput accounting (toggleable)   P1   B.5
F9   Load profile synthesis: home archetypes calibrated to ResStock/Pecan Street/RECS, HVAC, EV, pool; critical-loads split   P0   B.6
F10   PV model: NSRDB irradiance → POA → PVWatts-style derate, DC/AC clipping, seeded cloud noise   P0   B.7
F11   Grid outages: planned/stochastic/correlated; vendor-realistic transfer times; islanded power balance with PV curtailment and black-start   P1   B.8
F12   Vendor-realistic telemetry (granularity, quantization, ±1% revenue vs ±5% BMS noise classes)   P1   B.9
Device Registry & System Composition
Table
#   Feature   Tier   Spec
F13   Declarative JSON device catalog (batteries + inverters), build-time embedded, runtime shadow dir, semver + integrity-checked   P0   A.1, A.4
F14   Full 2024–2026 catalog: Tesla PW2/PW3 (+expansion), Enphase 5P/10/10C (+System Controller), SolarEdge Home Battery 400V (+Home Hub), sonnen ecoLinx/Core+/Batterie 10   P0   A.2
F15   HomeSystem composer with per-vendor validation constraints (stack limits, controller-required-for-backup, expansion packs)   P0   A.3
F16   Coupling-aware energy paths (AC double-conversion vs DC single-inversion) with explicit loss points   P0   A.3, B.3
F17   Add-a-device without recompiling core logic (schema-validated registry entries)   P1   A.1
API & Integration
Table
#   Feature   Tier   Spec
F18   OpenAPI 3.1 generated from code (utoipa), served at /openapi.json + Swagger UI; one-command client generation (openapi-generator/Fern/Stainless)   P0   C.1, C.3
F19   Homes/fleets CRUD; fleet manifests (archetype × count × ERCOT load-zone distribution)   P0   C.3
F20   Scenarios: bind time range, price source, weather, outages, seed   P0   C.3
F21   Fleet dispatch: kW setpoints, reserve SOC, operating modes, PV curtailment; jittered per-device execution latency (mimics real cloud APIs); audit log; idempotency   P0   C.3 /v1/dispatch
F22   Telemetry query + live SSE/WebSocket streams with filtering/downsampling   P0   C.3 /v1/telemetry
F23   Snapshots: full serializable state save/restore (bincode+zstd, sha256) for reproducible runs   P1   C.5
F24   Vendor-API mimicry mode: per-home endpoints impersonating Tesla Fleet/Gateway, Enphase Envoy/Enlighten, SolarEdge monitoring, sonnen REST v2 — point existing OEM integrations at the sim unchanged   P2   C.3 /v1/vendor-api, A.2
F25   Prometheus metrics + structured tracing   P1   C.6
ERCOT
Table
#   Feature   Tier   Spec
F26   Price replay: ERCOT MIS reports (RT 5-min SPPs, DAM, AS clearing, ORDC adders) normalized to Parquet; UTC/DST-safe   P0   D.3
F27   PriceSource trait: Replay / Live / Synthetic   P0   D.3
F28   Synthetic scenario generator: seeded regime-switching (normal / negative-solar / ORDC scarcity / Uri-style storm) with correct post-Uri caps   P1   D.4
F29   Settlement & P&L: wholesale/retail arbitrage, AS revenue with performance derates, 4CP savings attribution, retailer margin view   P1   D.5
F30   ADER-style aggregate dispatch channel (MW setpoints, pluggable baselines)   P1   D.2
F31   5-min interval alignment (1-s ticks integrated to settlement intervals; no sub-second frequency physics)   P0   D.7
Explicit Non-Goals
Other ISOs/RTOs (CAISO, PJM, …) — ERCOT only (D.1).
Sub-second frequency-response physics — AS response is modeled as ramp compliance (D.7).
Thermal runaway / safety-event simulation (B.4).
Full electrochemical PDE cell models — Thevenin-level fidelity is the ceiling (B.2).
mTLS / multi-tenant auth — single-tenant local deployment by default (C.4).
0.2 Build Milestones (for AI implementation agents)
Each milestone is independently verifiable via the conformance tests referenced.
Table
Milestone   Deliverable   Exit criteria
M1 — Core engine + registry   batsim-core, batsim-registry; F1–F6, F9, F10, F13–F16   Golden SOC traces pass per device model (B.11); energy-conservation proptests green
M2 — API skeleton   batsim-server, batsim-cli; F18–F22, F31   schemathesis contract tests green; one-command generated Python client drives a 100-home fleet end-to-end
M3 — ERCOT replay + settlement   batsim-ercot; F26, F27, F29, F31   Replay of a historical ERCOT day (incl. a scarcity day) → settlement report matches hand-checkable totals; MUST-VERIFY checklist (D.8) executed
M4 — Realism & resilience   F4, F7, F8, F11, F12, F23, F28   Outage/backup golden tests pass; synthetic price regimes reproducible from seed; snapshot restore = hash-identical
M5 — Scale + vendor mimicry   F17, F24, F25   10k homes × 1-s ticks ≥10× realtime (C.7 load test); a real OEM integration client connects to mimicry endpoints unmodified
Agents iterate per milestone with the cargo check → cargo clippy → cargo test loop defined in C.8. Determinism (F2) is a release gate at every milestone.
Part A — OEM Hardware Registry & System Topology
Status: Draft for implementation
Audience: AI implementation agents (no human engineering review assumed)
Siblings: Part B (physics engine / efficiency & degradation models), Part C (API surface & Rust architecture), Part D (ERCOT market & telemetry integration). This part defines what hardware exists and how it is composed; it does not define time-stepping physics (Part B), endpoint mechanics beyond registry queries (Part C), or market logic (Part D).
Provenance convention (normative). Every numeric or categorical value in the device catalog and in registry JSON entries MUST carry a provenance marker:
spec — value appears in a manufacturer datasheet, warranty document, or installation manual.
estimated — value is inferred, rounded, or taken from secondary sources because the manufacturer does not publish it. Implementations MUST NOT silently promote estimated values to spec.
Where a value is genuinely unknown, omit the field rather than inventing it; the JSON schema marks such fields optional. Defaults for omitted values are defined in §5 and may be overridden only with an explicit assumption note.
1. Registry Design Principles
1.1 Devices are data, not code
Device models (batteries, inverters, system controllers, PV presets) are declarative JSON catalog files under a registry/ directory tree in the simulator repository:
plain
registry/
  catalog.json                 # manifest: version, entry index, content hashes
  batteries/tesla_powerwall_2.json
  batteries/tesla_powerwall_3.json
  batteries/enphase_iq_battery_5p.json
  ...
  inverters/tesla_pw3_integrated_hybrid.json
  inverters/enphase_iq8d_micro.json
  inverters/solaredge_home_hub_hd_wave.json
  controllers/tesla_gateway_2.json
  controllers/enphase_iq_system_controller_2.json
  pv_presets/residential_south_8kw.json
Hardcoding device parameters as Rust structs is forbidden. Rust types exist only as serde deserialization targets for the schema in §4. The only compile-time coupling permitted is a build-time embed: catalog files MAY be embedded into the binary with include_str! / include_dir! (or equivalent) so the release binary is self-contained; the registry MUST also support loading catalog files from a directory path at startup (CLI flag --registry-dir, env SIM_REGISTRY_DIR) to allow new devices to be added without recompilation. When both are present, the external directory shadows the embedded catalog entry-by-entry on (kind, vendor, model_id) key match, and the shadowing MUST be logged.
1.2 Versioning
The registry has a single semantic version registry_version in catalog.json (e.g., "1.3.0"), bumped on any entry add/change/remove.
Each entry carries schema_version (schema in §4) and entry_version (content revision of that entry).
A simulator build reports the loaded registry version (embedded vs external source recorded) at GET /v1/registry/version (see Part C for endpoint conventions).
Entries are immutable once published: corrections require an entry_version bump and a supersedes pointer. Simulations MUST record the registry version and exact entry versions used, in their run manifest (Part B), for reproducibility.
1.3 Queryability
The registry is exposed read-only over the API (full path/query semantics in Part C; normative requirements here):
GET /v1/registry/batteries — list with filters: chemistry, coupling, min_usable_kwh, max_usable_kwh, vendor.
GET /v1/registry/batteries/{model_id} — full entry, including efficiency curves and provenance map.
GET /v1/registry/inverters[...] — analogous.
GET /v1/registry/controllers[...] — analogous.
GET /v1/registry/version — registry and schema versions, entry count, SHA-256 of catalog.json.
Unknown model_id → 404 with error body per Part C error schema. Registry MUST be validated at startup: any entry failing JSON Schema validation or failing cross-reference checks (§4.6) aborts startup with a non-zero exit code and a diagnostic listing every offending entry and field path.
1.4 Units and conventions
Energy: kWh. Power: kW. Temperature: °C. Time durations: seconds. Currency: USD where needed.
Efficiency values are fractions in [0,1] unless explicitly suffixed _pct (percent). Catalog tables below use % for readability; JSON entries store fractions.
All powers are AC-side unless suffixed _dc. Battery DC-side power is derived through the charge/discharge efficiency curves (Part B evaluates them; Part A only stores them).
2. Device Catalog (2024–2026 nameplate data)
Each row value carries spec or estimated. Abbreviations: RTE = round-trip efficiency; Cont. = continuous; DoD = depth of discharge; GWFM = grid-forming.
2.1 Tesla Powerwall 2 (tesla.powerwall_2)
Table
Attribute   Value   Provenance
Nameplate energy   14.0 kWh   spec
Usable energy   13.5 kWh   spec
Continuous discharge power (AC)   5.0 kW   spec
Peak discharge power (AC)   7.0 kW, 10 s   spec
Continuous charge power (AC)   5.0 kW   spec
Chemistry   NMC   spec
Coupling   AC-coupled, integrated inverter (line-interactive; grid-forming in backup)   spec
Round-trip efficiency (AC–AC)   ~90%   spec (Tesla-published figure, conditions not disclosed)
DoD / usable SOC window   100% DoD advertised; effective window 0–100% of 13.5 kWh   spec
Backup / grid-forming   Grid-forming behind Tesla Backup Gateway (Gateway 1/2); islanded microgrid with PV via frequency-shift curtailment   spec
Warranty   10 yr, 70% capacity retention; unlimited cycles when charged from solar; 37.8 MWh aggregate throughput otherwise   spec
Operating temperature   −20 °C to 50 °C   spec
Cooling   Liquid-cooled thermal management   spec
Comms (real world)   Local Gateway LAN API (/api/..., HTTPS, bearer token from password login); Tesla Fleet API / Tesla Energy cloud endpoints   spec
Weight / mounting   114 kg (approx.) floor/wall mount   spec
Notes for simulation: Gateway transfer time on outage ~ tens of ms to sub-second depending on configuration — treat as an estimated transfer-delay parameter (default 100 ms, estimated); exact behavior is firmware-dependent.
2.2 Tesla Powerwall 3 (tesla.powerwall_3)
Table
Attribute   Value   Provenance
Nameplate energy   13.5 kWh   spec
Usable energy   13.5 kWh   spec
Continuous discharge power (AC, on-grid)   11.5 kW   spec
Continuous power (backup/off-grid)   11.5 kW per unit, load-dependent   estimated (backup ratings published as unit configs; continuous on-grid figure is the clean datasheet number)
Continuous charge power (AC)   11.5 kW (battery-side up to 5 kW DC per some docs; treat charge limit as 11.5 kW AC from grid)   estimated
Chemistry   LFP   spec
Coupling   DC-coupled hybrid: integrated inverter with battery DC bus and solar MPPTs   spec
Solar input   Up to 20 kW DC, 6 MPPTs   spec
Solar-to-grid efficiency   97.5% peak claimed (PV→AC)   spec (claim)
Battery RTE (AC–AC)   ~89%   estimated (Tesla publishes ~89% solar-to-grid storage-cycle figure; AC–AC RTE not separately published)
Expansion   DC expansion packs (13.5 kWh each, no additional inverter), up to 3 packs per PW3   spec
Backup / grid-forming   Grid-forming behind Tesla Gateway / Backup Switch; whole-home backup capable   spec
Warranty   10 yr, 70% retention, unlimited cycles (solar-charged use case)   spec
Operating temperature   −20 °C to 50 °C   spec
Cooling   Active thermal management (air/liquid hybrid per teardowns)   estimated
Comms (real world)   Same Tesla local Gateway LAN API and Fleet API family as PW2 (PW3 registers as a device behind the Gateway/Tesla One)   spec
2.3 Enphase IQ Battery family (enphase.iq_battery_5p, enphase.iq_battery_10, enphase.iq_battery_10c)
Table
Attribute   IQ Battery 5P   IQ Battery 10 (2nd gen)   IQ Battery 10C   Provenance
Nameplate energy   5.0 kWh   10.08 kWh   10.0 kWh   spec / spec / spec
Usable energy   4.96 kWh   10.08 kWh (usable ≈ nameplate; LFP window)   10.0 kWh   spec / estimated / estimated
Continuous power (AC)   3.84 kW   3.84 kW   7.08 kW   spec / spec / spec
Peak power (AC)   7.68 kW (3 s)   7.68 kW (3 s)   14.16 kW (3 s)   spec / spec / estimated
Embedded microinverters   6 × IQ8D   12 × IQ8D   12 × IQ8D (newer rev)   spec / spec / estimated
Chemistry   LFP   LFP   LFP   spec
Coupling   AC-coupled (microinverter-based)   AC-coupled   AC-coupled   spec
RTE (AC–AC)   ~90%   ~89%   ~89%   spec (5P) / estimated / estimated
Cooling   Passive (no fan, no moving parts)   Passive   Passive   spec
Backup / grid-forming   Grid-forming via IQ8D micros; requires IQ System Controller 2 or 3 for backup/islanding and grid-forming microgrid; supports generator input through controller   spec
Warranty   15 yr, 6,000 cycles, 60% retention   15 yr, 6,000 cycles   15 yr   spec / spec / estimated
Operating temperature   −20 °C to 50 °C (derating above ~45 °C)   same   same   spec / estimated / estimated
Comms (real world)   Enphase Enlighten cloud API; local Envoy/IQ Gateway API (/ivp/..., /api/v1/production, token auth via Enlighten)   spec
Simulation-relevant note: power scales linearly with microinverter count (0.64 kW per IQ8D continuous, spec-derived: 3.84/6). The registry therefore stores microinverter_count explicitly (§4.2).
2.4 SolarEdge Home Battery 400V (solaredge.home_battery_400v)
Table
Attribute   Value   Provenance
Nameplate energy   10.3 kWh   estimated (9.7 usable published; nameplate inferred)
Usable energy   9.7 kWh   spec
Continuous power (DC-coupled path)   5.0 kW   spec
Peak power   7.5 kW (10 s)   spec
Chemistry   LFP   spec
Coupling   DC-coupled to SolarEdge Home Hub / Energy Hub single-phase inverter; up to 3 batteries per inverter (29.1 kWh)   spec
RTE (DC-coupled)   94.5% claimed (PV→battery→AC path)   spec (claim)
Backup / grid-forming   Backup via Home Hub Backup Interface; inverter is grid-following on-grid, grid-forming in backup mode   spec
Warranty   10 yr, 70% retention; unlimited cycles; throughput warranty ~ (published as cycles-unlimited variant)   spec/estimated
Operating temperature   −10 °C to 50 °C   estimated
Cooling   Passive   estimated
Comms (real world)   SolarEdge monitoring cloud API (/site/{id}/..., API key); local Modbus-TCP on inverter (site-limited)   spec
2.5 Sonnen (sonnen.ecolinx, sonnen.sonnencore_plus, sonnen.sonnenbatterie_10)
Table
Attribute   ecoLinx   sonnenCore+   sonnenBatterie 10   Provenance
Usable energy   10–20 kWh, 2 kWh modular steps   10–20 kWh   5.5–22 kWh, 5.5 kWh steps   spec / spec / spec
Continuous power   8.0 kW   4.8 kW (10 kWh) to 8.6 kW (20 kWh)   8.0 kW (hybrid)   spec / spec / spec
Chemistry   LFP   LFP   LFP   spec
Coupling   AC-coupled (integrated inverter)   AC-coupled (integrated inverter)   Hybrid (DC-coupled PV option) or AC-coupled retrofit   spec
RTE   ~86–90%   ~90%   ~96% (DC path, claimed)   estimated / estimated / spec (claim)
Backup / grid-forming   Grid-forming, whole-home energy automation (integrates with smart breakers/loads)   Grid-forming w/ sonnenProtect (backup add-on)   Grid-forming w/ backup box   spec / spec / spec
Warranty   10 yr / 10,000 cycles   10 yr / 10,000 cycles   10 yr / 10,000 cycles   spec
Operating temperature   5 °C to 40 °C (indoor-oriented)   −5 °C to 45 °C   −5 °C to 45 °C   estimated
Comms (real world)   Sonnen local REST API v2 on the battery LAN (readings: /api/v2/status, /api/v2/configurations; setpoints via authenticated endpoints); cloud portal + sonnen VPP aggregation software (sonnenCommunity / grid services stack) — directly relevant to Part D mimicry   spec
2.6 Vendor API mimicry summary (consumed by Part C/D)
Table
Vendor   API family to mimic   Auth style   Key endpoints (real world)   Notes
Tesla   Local Gateway LAN API + Fleet API   Local: login → bearer token; Fleet: OAuth2   /api/login/Basic, /api/meters/aggregates, /api/system_status/soe, /api/operation   Local API is the primary mimicry target; Fleet API second
Enphase   Enlighten cloud + local Envoy/IQ Gateway   Enlighten token (JWT) for local; OAuth for cloud   /ivp/livedata/status, /api/v1/production, /ivp/ensemble/inventory   Ensemble (battery) inventory endpoints report per-unit serials
SolarEdge   Monitoring cloud API; local Modbus-TCP   API key (cloud)   /site/{id}/power, /site/{id}/storageData, /site/{id}/energy   storageData gives per-battery SOC/power
Sonnen   Local REST API v2   Token header   /api/v2/status, /api/v2/configurations, /api/v2/setpoint/...   Simplest mimicry target; setpoint endpoints map well to fleet dispatch
The simulator's per-device vendor-API adapter layer (Part C) uses the vendor_api block of each registry entry (§4.5) to generate these surfaces; Part A only declares them.
3. System Topology Modeling
3.1 Composition model
A HomeSystem is a directed composition of registry parts plus a small number of free parameters. The simulator composes, at simulation-init time (Part B consumes the resulting resolved topology object):
Table
Component   Cardinality   Source   Notes
Battery units   0..n   BatteryModel registry entries + quantity   Units of the same model share one entry; per-unit serials generated (Part C)
Inverters   0..n   InverterModel entries + quantity   For AC-coupled batteries the inverter is embedded in the BatteryModel; explicit InverterModel entries are required for hybrid (DC-coupled) systems and for PV string inverters
Microinverter count   0..n   derived from battery entry or PV preset   Enphase: microinverter_count per battery; PV microinverter counts come from PV preset
PV array   0..1   pv_presets/ or inline   kw_dc (float), orientation enum (`S   SSW   SW   W   E   SE   SSE   N   FLATor azimuth degrees),tilt_deg, dc_ac_ratio`
Main service panel   1   free params   service_rating_a (default 200 A @ 240 V split-phase), bus rating
Backup sub-panel / critical loads   0..1   free params   critical_loads_kw_peak, list of load circuits; absent ⇒ whole-home backup topology
Generator input   0..1   free params   generator_kw, auto-transfer-switch flag; only valid when system controller supports generator input (e.g., IQ System Controller 2/3)
EV charger   0..n   free params   ev_charger_kw (default 11.5 kW L2), modeled as controllable load with a schedule + override API (Part C); optionally V1G (controllable) or dumb
System controller / gateway   0..n   controller registry entries   Tesla Gateway, IQ System Controller 2/3, SolarEdge Backup Interface, sonnen backup box — required iff backup_capable: true is asserted for the system
Grid meter point   1   implicit   ERCOT ESIID binding point (Part D)
Validation rules enforced at composition time:
If backup_capable: true, exactly one controller entry with provides_grid_forming: true MUST be present, OR every battery in the system MUST have grid_forming_in_backup: true with an integrated transfer mechanism (PW2/PW3 behind Gateway ⇒ Gateway entry required; Enphase ⇒ IQ System Controller 2/3 entry required).
DC-coupled batteries MUST reference a compatible hybrid inverter (compatible_inverter_ids non-empty intersection).
SolarEdge: battery_count ≤ 3 × home_hub_inverter_count (spec).
PW3: expansion_pack_count ≤ 3 per PW3 head unit (spec); expansion packs add energy but not power.
Enphase: battery continuous power = 0.64 kW × total IQ8D count (spec-derived); composition MUST recompute and cross-check against the summed battery entries.
Total backup-path continuous power = min(Σ battery continuous power, Σ inverter backup rating). This computed value, not any single nameplate, is what Part B may dispatch behind the meter.
3.2 AC-coupled energy path (PW2, Enphase IQ Battery, sonnen ecoLinx/Core+)
plain
 PV array (DC) ──► PV inverter / microinverters ──► AC combiner ─┐
                       (loss L1: DC→AC, η_pv_inv)                │
                                                                 ▼
 GRID ◄─────► MAIN PANEL ◄─────────────► HOME LOADS (AC)
                 │  ▲
                 │  │ discharge: battery DC ──► battery inverter ──► AC
                 │  │              (loss L3: DC→AC, η_dis)
                 │  │
                 │  └ charge: grid/PV AC ──► battery inverter ──► battery DC
                 │                 (loss L2: AC→DC, η_chg)
                 │
                 └──► [SYSTEM CONTROLLER / GATEWAY] ──► BACKUP SUB-PANEL ──► CRITICAL LOADS
                                   (islanding + transfer switch)

Storing PV in battery (round trip): PV DC ─[L1]→ AC ─[L2]→ batt DC ─[L3]→ AC  ⇒  DOUBLE CONVERSION
Effective PV-storage round trip ≈ η_pv_inv × η_chg × η_dis   (AC-coupled)
Loss locations: L1 at the PV inverter (present even without storage), L2 on battery charge, L3 on battery discharge. Grid charging incurs only L2+L3. Registry entries store η_chg and η_dis as power-dependent point lists (§4); η_pv_inv belongs to the PV InverterModel.
3.3 DC-coupled hybrid energy path (PW3, SolarEdge Home Battery 400V, sonnenBatterie 10 hybrid)
plain
 PV array (DC) ──► MPPT inputs ──► HYBRID INVERTER DC BUS ◄──── BATTERY (DC)
                                        │    ▲           (loss L2': DC→DC converter, η_dcdc)
                                        │    │                  (bidirectional)
                       discharge: batt DC ┘    │ PV direct: DC ─[L3']→ AC
                                        ▼
                          single DC→AC inversion (loss L3', η_hyb)
                                        │
 GRID ◄───► MAIN PANEL ◄───► HOME LOADS (AC)
                  │
                  └──► [BACKUP INTERFACE / GATEWAY] ──► CRITICAL LOADS

Storing PV in battery (round trip): PV DC ─[L2']→ batt DC ─[L3']→ AC  ⇒  SINGLE INVERSION
Effective PV-storage round trip ≈ η_dcdc × η_hyb              (DC-coupled; PV never touches AC)
Grid charging (AC→batt): AC ─[η_hyb_inv]→ DC bus ─[η_dcdc]→ batt   (still double conversion)
Consequences the implementation MUST honor:
DC-coupled systems have higher PV-storage round-trip efficiency (SolarEdge claims ~94.5%, spec claim) than AC-coupled (~89–90%, spec/estimated), but grid-charging efficiency is comparable or worse because AC→DC→batt-DC is still a double conversion. Part B selects the correct loss path per charge source (pv vs grid) using the topology tag — the registry MUST therefore store both rte_pv_coupled and rte_ac_coupled (or the component curves to derive them) for hybrid entries.
DC-coupled batteries cannot charge from AC without the hybrid inverter being present and online; an inverter fault zeroes battery charge AND PV production. Model as a single-point-of-failure flag consumed by Part B fault injection (if implemented) — otherwise document as ignored.
Battery clip/curtailment: hybrid inverters have a max DC-bus and AC output rating; PV + battery simultaneous discharge is capped at the AC rating (e.g., PW3 11.5 kW AC, spec). Registry stores max_ac_output_kw on the InverterModel; Part B enforces the cap.
AC-coupled batteries can charge while PV exports at full power (parallel paths, no shared inverter bottleneck) up to the main-panel interconnection limit.
3.4 Microgrid / backup behavior
Grid-forming vs grid-following is a per-entry boolean pair: grid_following_on_grid (all catalog devices: true) and grid_forming_in_backup (all catalog devices: true when paired with their controller). The controller entry owns islanding mechanics: transfer time (estimated defaults: Tesla Gateway 100 ms; IQ System Controller ~<1 s; mark estimated), frequency-shift PV curtailment curve (Watt-Hz droop, default 0.5 Hz full-curtail span, estimated), and generator interlock.
PV microinverters (IQ8-series, spec) are themselves grid-forming-capable and participate in the islanded microgrid; in simulation, PV continues producing in backup subject to (a) battery SOC headroom for curtailment signaling and (b) load balance — implemented in Part B, enabled by flags declared here.
4. Registry Data Model (JSON Schema, draft 2020-12)
4.1 Shared definitions ($defs, referenced by all schemas)
JSON
{
  "$id": "https://battery-fleet-sim.local/schemas/common.json",
  "$defs": {
    "provenance": { "enum": ["spec", "estimated"] },
    "annotatedNumber": {
      "type": "object",
      "required": ["value", "provenance"],
      "properties": {
        "value": { "type": "number" },
        "provenance": { "$ref": "#/$defs/provenance" },
        "unit": { "type": "string", "description": "SI-ish unit label, e.g. kWh, kW, degC" },
        "note": { "type": "string" }
      },
      "additionalProperties": false
    },
    "efficiencyCurve": {
      "type": "object",
      "required": ["points", "provenance"],
      "properties": {
        "points": {
          "type": "array",
          "minItems": 2,
          "description": "Monotonically increasing x. Linear interpolation between points; clamp outside range. Evaluated by Part B.",
          "items": {
            "type": "object",
            "required": ["x_kw", "efficiency"],
            "properties": {
              "x_kw": { "type": "number", "minimum": 0 },
              "efficiency": { "type": "number", "minimum": 0, "maximum": 1 }
            },
            "additionalProperties": false
          }
        },
        "provenance": { "$ref": "#/$defs/provenance" }
      },
      "additionalProperties": false
    },
    "chemistry": { "enum": ["LFP", "NMC", "NCA"] },
    "coupling": { "enum": ["ACCoupled", "DCCoupledHybrid", "MicroinverterBased"] },
    "temperatureRange": {
      "type": "object",
      "required": ["min_c", "max_c", "provenance"],
      "properties": {
        "min_c": { "type": "number" },
        "max_c": { "type": "number" },
        "derating_note": { "type": "string" },
        "provenance": { "$ref": "#/$defs/provenance" }
      },
      "additionalProperties": false
    },
    "warranty": {
      "type": "object",
      "properties": {
        "years": { "$ref": "#/$defs/annotatedNumber" },
        "cycles": { "$ref": "#/$defs/annotatedNumber" },
        "throughput_mwh": { "$ref": "#/$defs/annotatedNumber" },
        "capacity_retention_pct": { "$ref": "#/$defs/annotatedNumber" }
      },
      "additionalProperties": false
    },
    "vendorApi": {
      "type": "object",
      "required": ["family", "auth_style", "endpoints", "provenance"],
      "properties": {
        "family": { "enum": ["tesla_local_gateway", "tesla_fleet_api", "enphase_envoy_local", "enphase_enlighten_cloud", "solaredge_monitoring_cloud", "solaredge_modbus_tcp", "sonnen_local_v2", "generic"] },
        "auth_style": { "enum": ["bearer_local_login", "oauth2", "jwt_via_cloud", "api_key", "token_header", "none"] },
        "base_path_hint": { "type": "string" },
        "endpoints": {
          "type": "array",
          "items": {
            "type": "object",
            "required": ["path", "purpose"],
            "properties": {
              "path": { "type": "string" },
              "purpose": { "enum": ["telemetry", "soc", "setpoint_dispatch", "mode_config", "inventory", "auth"] }
            },
            "additionalProperties": false
          }
        },
        "provenance": { "$ref": "#/$defs/provenance" }
      },
      "additionalProperties": false
    }
  }
}
4.2 BatteryModel schema
JSON
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://battery-fleet-sim.local/schemas/battery_model.json",
  "title": "BatteryModel",
  "type": "object",
  "required": [
    "schema_version", "entry_version", "model_id", "vendor", "display_name",
    "chemistry", "coupling", "nameplate_energy_kwh", "usable_energy_kwh",
    "continuous_discharge_power_kw", "continuous_charge_power_kw",
    "soc_window", "charge_efficiency_curve", "discharge_efficiency_curve",
    "grid_forming_in_backup", "warranty", "operating_temperature",
    "ramp_rate", "vendor_api"
  ],
  "properties": {
    "schema_version": { "const": "1.0.0" },
    "entry_version": { "type": "string", "pattern": "^\\d+\\.\\d+\\.\\d+$" },
    "supersedes": { "type": "string", "description": "model_id of entry this replaces" },
    "model_id": { "type": "string", "pattern": "^[a-z0-9_]+\\.[a-z0-9_]+$", "description": "vendor.model" },
    "vendor": { "type": "string" },
    "display_name": { "type": "string" },
    "chemistry": { "$ref": "common.json#/$defs/chemistry" },
    "coupling": { "$ref": "common.json#/$defs/coupling" },
    "nameplate_energy_kwh": { "$ref": "common.json#/$defs/annotatedNumber" },
    "usable_energy_kwh": { "$ref": "common.json#/$defs/annotatedNumber" },
    "continuous_discharge_power_kw": { "$ref": "common.json#/$defs/annotatedNumber" },
    "peak_discharge_power_kw": { "$ref": "common.json#/$defs/annotatedNumber" },
    "peak_duration_s": { "$ref": "common.json#/$defs/annotatedNumber" },
    "continuous_charge_power_kw": { "$ref": "common.json#/$defs/annotatedNumber" },
    "soc_window": {
      "type": "object",
      "required": ["min_soc_frac", "max_soc_frac", "provenance"],
      "properties": {
        "min_soc_frac": { "type": "number", "minimum": 0, "maximum": 1 },
        "max_soc_frac": { "type": "number", "minimum": 0, "maximum": 1 },
        "reserve_floor_frac": { "type": "number", "minimum": 0, "maximum": 1, "description": "user-settable backup reserve; simulator default" },
        "provenance": { "$ref": "common.json#/$defs/provenance" }
      },
      "additionalProperties": false
    },
    "charge_efficiency_curve": { "$ref": "common.json#/$defs/efficiencyCurve" },
    "discharge_efficiency_curve": { "$ref": "common.json#/$defs/efficiencyCurve" },
    "rte_pv_coupled": { "$ref": "common.json#/$defs/annotatedNumber", "description": "PV-source round trip (single-inversion path for DC hybrids)" },
    "rte_ac_coupled": { "$ref": "common.json#/$defs/annotatedNumber", "description": "grid-source round trip (double conversion)" },
    "grid_forming_in_backup": { "type": "boolean" },
    "requires_controller_id": { "type": ["string", "null"], "description": "controller model_id required for backup operation" },
    "integrated_inverter": { "type": "boolean", "description": "true for PW2/PW3/5P/ecoLinx; false for rack batteries needing external hybrid inverter" },
    "microinverter_count": { "type": ["integer", "null"], "minimum": 1 },
    "power_per_microinverter_kw": { "$ref": "common.json#/$defs/annotatedNumber" },
    "expansion": {
      "type": "object",
      "properties": {
        "max_units_per_inverter": { "type": "integer" },
        "expansion_pack_model_id": { "type": ["string", "null"] },
        "packs_add_power": { "type": "boolean" }
      },
      "additionalProperties": false
    },
    "warranty": { "$ref": "common.json#/$defs/warranty" },
    "operating_temperature": { "$ref": "common.json#/$defs/temperatureRange" },
    "cooling": { "enum": ["passive", "active_air", "active_liquid", "unknown"] },
    "ramp_rate": {
      "type": "object",
      "required": ["max_kw_per_s", "provenance"],
      "properties": {
        "max_kw_per_s": { "type": "number", "exclusiveMinimum": 0 },
        "provenance": { "$ref": "common.json#/$defs/provenance" },
        "note": { "type": "string", "description": "rarely published; default estimated from sub-second response capability of Li-ion inverters" }
      },
      "additionalProperties": false
    },
    "self_discharge_frac_per_day": { "$ref": "common.json#/$defs/annotatedNumber" },
    "vendor_api": { "$ref": "common.json#/$defs/vendorApi" }
  },
  "additionalProperties": false
}
4.3 InverterModel schema
JSON
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://battery-fleet-sim.local/schemas/inverter_model.json",
  "title": "InverterModel",
  "type": "object",
  "required": [
    "schema_version", "entry_version", "model_id", "vendor", "display_name",
    "topology", "rated_ac_output_kw", "efficiency_curve",
    "grid_forming_in_backup", "compatible_battery_ids"
  ],
  "properties": {
    "schema_version": { "const": "1.0.0" },
    "entry_version": { "type": "string" },
    "model_id": { "type": "string" },
    "vendor": { "type": "string" },
    "display_name": { "type": "string" },
    "topology": { "enum": ["HybridDCCoupled", "StringPVOnly", "MicroinverterPV", "BatteryIntegrated"] },
    "rated_ac_output_kw": { "$ref": "common.json#/$defs/annotatedNumber" },
    "max_ac_output_kw_backup": { "$ref": "common.json#/$defs/annotatedNumber" },
    "max_pv_dc_input_kw": { "$ref": "common.json#/$defs/annotatedNumber" },
    "mppt_count": { "$ref": "common.json#/$defs/annotatedNumber" },
    "max_pv_voltage_v": { "$ref": "common.json#/$defs/annotatedNumber" },
    "efficiency_curve": { "$ref": "common.json#/$defs/efficiencyCurve", "description": "DC→AC conversion efficiency vs AC output kW" },
    "grid_following_on_grid": { "type": "boolean", "default": true },
    "grid_forming_in_backup": { "type": "boolean" },
    "compatible_battery_ids": { "type": "array", "items": { "type": "string" } },
    "max_batteries": { "type": ["integer", "null"] },
    "vendor_api": { "$ref": "common.json#/$defs/vendorApi" }
  },
  "additionalProperties": false
}
4.4 HomeSystem composition schema
JSON
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "$id": "https://battery-fleet-sim.local/schemas/home_system.json",
  "title": "HomeSystem",
  "type": "object",
  "required": ["schema_version", "system_id", "batteries", "inverters", "main_panel", "backup_capable", "grid_meter"],
  "properties": {
    "schema_version": { "const": "1.0.0" },
    "system_id": { "type": "string", "format": "uuid" },
    "label": { "type": "string" },
    "batteries": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["model_id", "quantity"],
        "properties": {
          "model_id": { "type": "string" },
          "quantity": { "type": "integer", "minimum": 1 },
          "expansion_packs_per_unit": { "type": "integer", "minimum": 0, "default": 0 },
          "initial_soc_frac": { "type": "number", "minimum": 0, "maximum": 1, "default": 0.5 },
          "reserve_frac": { "type": "number", "minimum": 0, "maximum": 1, "default": 0.2 }
        },
        "additionalProperties": false
      }
    },
    "inverters": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["model_id", "quantity"],
        "properties": {
          "model_id": { "type": "string" },
          "quantity": { "type": "integer", "minimum": 1 }
        },
        "additionalProperties": false
      }
    },
    "controllers": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["model_id", "quantity"],
        "properties": {
          "model_id": { "type": "string" },
          "quantity": { "type": "integer", "minimum": 1 }
        },
        "additionalProperties": false
      }
    },
    "pv": {
      "type": ["object", "null"],
      "required": ["kw_dc", "orientation"],
      "properties": {
        "kw_dc": { "type": "number", "exclusiveMinimum": 0 },
        "orientation": { "anyOf": [ { "enum": ["N","NE","E","SE","S","SSW","SW","WSW","W","NW","FLAT"] }, { "type": "integer", "minimum": 0, "maximum": 359, "description": "azimuth degrees" } ] },
        "tilt_deg": { "type": "number", "minimum": 0, "maximum": 90, "default": 25 },
        "dc_ac_ratio": { "type": "number", "exclusiveMinimum": 0, "default": 1.2 },
        "pv_inverter_model_id": { "type": ["string", "null"], "description": "null iff PV lands on a hybrid inverter's MPPTs" }
      },
      "additionalProperties": false
    },
    "main_panel": {
      "type": "object",
      "required": ["service_rating_a"],
      "properties": {
        "service_rating_a": { "type": "number", "default": 200, "description": "240 V split-phase assumed" },
        "interconnection_limit_kw": { "type": ["number", "null"], "description": "export cap if utility-imposed; null = none" }
      },
      "additionalProperties": false
    },
    "backup_capable": { "type": "boolean" },
    "backup_panel": {
      "type": ["object", "null"],
      "properties": {
        "critical_loads_peak_kw": { "type": "number", "default": 5.0 },
        "whole_home": { "type": "boolean", "default": false }
      },
      "additionalProperties": false
    },
    "generator": {
      "type": ["object", "null"],
      "properties": {
        "rated_kw": { "type": "number" },
        "auto_start": { "type": "boolean", "default": true }
      },
      "additionalProperties": false
    },
    "ev_chargers": {
      "type": "array",
      "items": {
        "type": "object",
        "required": ["rated_kw"],
        "properties": {
          "rated_kw": { "type": "number", "default": 11.5 },
          "controllable": { "type": "boolean", "default": true, "description": "V1G controllable load; dispatch interface in Part C" },
          "on_backup_panel": { "type": "boolean", "default": false }
        },
        "additionalProperties": false
      }
    },
    "grid_meter": {
      "type": "object",
      "required": ["esiid"],
      "properties": {
        "esiid": { "type": "string", "description": "ERCOT ESI ID binding; consumed by Part D" },
        "tdsp": { "type": "string", "description": "Oncor | CenterPoint | AEP-Central | AEP-North | TNMP" }
      },
      "additionalProperties": false
    }
  },
  "additionalProperties": false
}
4.5 Vendor-API mimicry metadata
The vendor_api block (§4.1) is the only Part-A input to the adapter layer. Part C MUST generate, for each instantiated device, endpoints mirroring the declared family with the declared auth_style; SOC/telemetry responses MUST be computed from Part-B state at request time. Fields family/endpoints are spec-grade (they describe real vendor surfaces); simulated auth tokens are synthetic and MUST be marked as such in generated responses' metadata header (X-Sim-Vendor-Api: true).
4.6 Cross-reference validation (startup)
Every HomeSystem.batteries[].model_id and inverters[].model_id MUST resolve to a registry entry of matching kind.
requires_controller_id MUST resolve to a controller entry present in the system when backup_capable: true.
compatible_battery_ids ∩ system batteries ≠ ∅ for every hybrid inverter.
catalog.json SHA-256 MUST match the concatenated entry hashes (tamper detection).
Every annotatedNumber.provenance MUST be present — schema-required, not optional.
4.7 Filled example — Tesla Powerwall 3 (BatteryModel)
JSON
{
  "schema_version": "1.0.0",
  "entry_version": "1.0.0",
  "model_id": "tesla.powerwall_3",
  "vendor": "Tesla",
  "display_name": "Tesla Powerwall 3",
  "chemistry": "LFP",
  "coupling": "DCCoupledHybrid",
  "nameplate_energy_kwh": { "value": 13.5, "provenance": "spec", "unit": "kWh" },
  "usable_energy_kwh": { "value": 13.5, "provenance": "spec", "unit": "kWh" },
  "continuous_discharge_power_kw": { "value": 11.5, "provenance": "spec", "unit": "kW", "note": "on-grid continuous AC" },
  "peak_discharge_power_kw": { "value": 11.5, "provenance": "estimated", "unit": "kW", "note": "backup peak ratings config-dependent; not cleanly published" },
  "peak_duration_s": { "value": 10, "provenance": "estimated", "unit": "s" },
  "continuous_charge_power_kw": { "value": 11.5, "provenance": "estimated", "unit": "kW", "note": "grid charge AC-side; PV DC charge limited by MPPT/battery DC rating" },
  "soc_window": { "min_soc_frac": 0.0, "max_soc_frac": 1.0, "reserve_floor_frac": 0.2, "provenance": "spec" },
  "charge_efficiency_curve": {
    "points": [
      { "x_kw": 0.5, "efficiency": 0.90 },
      { "x_kw": 2.0, "efficiency": 0.945 },
      { "x_kw": 5.0, "efficiency": 0.95 },
      { "x_kw": 11.5, "efficiency": 0.93 }
    ],
    "provenance": "estimated"
  },
  "discharge_efficiency_curve": {
    "points": [
      { "x_kw": 0.5, "efficiency": 0.90 },
      { "x_kw": 2.0, "efficiency": 0.945 },
      { "x_kw": 5.0, "efficiency": 0.95 },
      { "x_kw": 11.5, "efficiency": 0.93 }
    ],
    "provenance": "estimated"
  },
  "rte_pv_coupled": { "value": 0.945, "provenance": "estimated", "unit": "frac", "note": "single-inversion DC path; derived from component claims" },
  "rte_ac_coupled": { "value": 0.89, "provenance": "estimated", "unit": "frac", "note": "Tesla-published ~89% solar-to-grid storage-cycle figure used as AC RTE proxy" },
  "grid_forming_in_backup": true,
  "requires_controller_id": "tesla.gateway_2",
  "integrated_inverter": true,
  "microinverter_count": null,
  "power_per_microinverter_kw": null,
  "expansion": { "max_units_per_inverter": 4, "expansion_pack_model_id": "tesla.pw3_expansion_pack", "packs_add_power": false },
  "warranty": {
    "years": { "value": 10, "provenance": "spec", "unit": "yr" },
    "cycles": { "value": 999999, "provenance": "spec", "unit": "cycles", "note": "unlimited when solar-charged; sentinel value" },
    "capacity_retention_pct": { "value": 70, "provenance": "spec", "unit": "pct" }
  },
  "operating_temperature": { "min_c": -20, "max_c": 50, "provenance": "spec" },
  "cooling": "active_liquid",
  "ramp_rate": { "max_kw_per_s": 11.5, "provenance": "estimated", "note": "full-swing in ~1 s assumed for Li-ion inverter; not published" },
  "self_discharge_frac_per_day": { "value": 0.002, "provenance": "estimated", "unit": "frac/day", "note": "includes idle/standby draw; not published" },
  "vendor_api": {
    "family": "tesla_local_gateway",
    "auth_style": "bearer_local_login",
    "base_path_hint": "/api",
    "endpoints": [
      { "path": "/api/login/Basic", "purpose": "auth" },
      { "path": "/api/system_status/soe", "purpose": "soc" },
      { "path": "/api/meters/aggregates", "purpose": "telemetry" },
      { "path": "/api/operation", "purpose": "mode_config" }
    ],
    "provenance": "spec"
  }
}
4.8 Filled example — Enphase IQ Battery 5P (BatteryModel)
JSON
{
  "schema_version": "1.0.0",
  "entry_version": "1.0.0",
  "model_id": "enphase.iq_battery_5p",
  "vendor": "Enphase",
  "display_name": "Enphase IQ Battery 5P",
  "chemistry": "LFP",
  "coupling": "MicroinverterBased",
  "nameplate_energy_kwh": { "value": 5.0, "provenance": "spec", "unit": "kWh" },
  "usable_energy_kwh": { "value": 4.96, "provenance": "spec", "unit": "kWh" },
  "continuous_discharge_power_kw": { "value": 3.84, "provenance": "spec", "unit": "kW", "note": "6 × IQ8D @ 0.64 kW" },
  "peak_discharge_power_kw": { "value": 7.68, "provenance": "spec", "unit": "kW" },
  "peak_duration_s": { "value": 3, "provenance": "spec", "unit": "s" },
  "continuous_charge_power_kw": { "value": 3.84, "provenance": "spec", "unit": "kW" },
  "soc_window": { "min_soc_frac": 0.0, "max_soc_frac": 1.0, "reserve_floor_frac": 0.1, "provenance": "spec", "note": "LFP full window advertised; reserve floor is a user setting" },
  "charge_efficiency_curve": {
    "points": [
      { "x_kw": 0.3, "efficiency": 0.88 },
      { "x_kw": 1.0, "efficiency": 0.92 },
      { "x_kw": 1.92, "efficiency": 0.93 },
      { "x_kw": 3.84, "efficiency": 0.915 }
    ],
    "provenance": "estimated"
  },
  "discharge_efficiency_curve": {
    "points": [
      { "x_kw": 0.3, "efficiency": 0.88 },
      { "x_kw": 1.0, "efficiency": 0.92 },
      { "x_kw": 1.92, "efficiency": 0.93 },
      { "x_kw": 3.84, "efficiency": 0.915 }
    ],
    "provenance": "estimated"
  },
  "rte_pv_coupled": { "value": 0.90, "provenance": "spec", "unit": "frac", "note": "AC-coupled round trip, Enphase-published ~90%" },
  "rte_ac_coupled": { "value": 0.90, "provenance": "spec", "unit": "frac" },
  "grid_forming_in_backup": true,
  "requires_controller_id": "enphase.iq_system_controller_2",
  "integrated_inverter": true,
  "microinverter_count": 6,
  "power_per_microinverter_kw": { "value": 0.64, "provenance": "spec", "unit": "kW" },
  "expansion": { "max_units_per_inverter": null, "expansion_pack_model_id": null, "packs_add_power": true },
  "warranty": {
    "years": { "value": 15, "provenance": "spec", "unit": "yr" },
    "cycles": { "value": 6000, "provenance": "spec", "unit": "cycles" },
    "capacity_retention_pct": { "value": 60, "provenance": "spec", "unit": "pct" }
  },
  "operating_temperature": { "min_c": -20, "max_c": 50, "derating_note": "power derating above ~45 C", "provenance": "spec" },
  "cooling": "passive",
  "ramp_rate": { "max_kw_per_s": 3.84, "provenance": "estimated", "note": "sub-second full-swing assumed; not published" },
  "self_discharge_frac_per_day": { "value": 0.002, "provenance": "estimated", "unit": "frac/day" },
  "vendor_api": {
    "family": "enphase_envoy_local",
    "auth_style": "jwt_via_cloud",
    "base_path_hint": "/",
    "endpoints": [
      { "path": "/ivp/ensemble/inventory", "purpose": "inventory" },
      { "path": "/ivp/ensemble/soc", "purpose": "soc" },
      { "path": "/api/v1/production", "purpose": "telemetry" },
      { "path": "/ivp/livedata/status", "purpose": "telemetry" }
    ],
    "provenance": "spec"
  }
}
5. Open Questions & Default Assumptions
An implementing agent MUST resolve each item below, either by adopting the stated default (and recording it in the run manifest) or by raising it per the project's clarification process.
Efficiency curve granularity (default: as given). Manufacturer-published RTE is a single number; the point-list curves in §4.7–4.8 are estimated syntheses. Default: use them as-is with linear interpolation and endpoint clamping (Part B). If Part B requires convexity/consistency guarantees (charge η × discharge η ≤ claimed RTE), it may renormalize the curves and MUST log the adjustment.
Peak-power sustain dynamics. peak_duration_s is declared, but the thermal/SoC-dependent envelope beyond it is not modeled in Part A. Default: Part B enforces a hard timer (peak allowed for peak_duration_s within any 60 s window, then clamp to continuous). Flagged estimated behavior.
Ramp rates are not published by any catalog vendor. Default: full rated swing in 1 s (max_kw_per_s = continuous rating, estimated). ERCOT-scale aggregation (Part D) is insensitive to sub-second ramp at residential unit scale.
Idle/standby power draw (inverter tare losses, gateway draw ~5–20 W, estimated) is not in the catalog tables. Default: fold into self_discharge_frac_per_day (0.2%/day, estimated); a later registry revision SHOULD split this into standby_power_w.
Temperature derating curves (power vs ambient T) are not published beyond operating ranges. Default: flat within range, hard cutoff outside; Part B MAY apply LFP cold-charge inhibition below 0 °C (estimated heuristic, charge power × 0.5 between −10 °C and 0 °C, zero below −10 °C) — flag it as an assumption in the run manifest.
Backup transfer time defaults: Tesla Gateway 100 ms, IQ System Controller 2/3 1000 ms, SolarEdge Backup Interface 500 ms (all estimated). Loads are not modeled during the transfer gap (simulator treats transfer as instantaneous but records the delay in telemetry for realism).
Generator input is declared topologically but generator dispatch/fuel physics are out of scope for Part A/B v1. Default: generator, if present, is an always-available backup source capped at rated_kw, activated only by explicit API command (Part C), not auto-start logic.
EV charger is load-only (V1G). V2H/V2G bidirectional EV behavior is explicitly out of scope; the schema has no field for it. If ERCOT DR programs require EV discharge (Part D), a schema revision is required.
Firmware-version effects (real vendors change dispatch behavior via OTA updates) are not modeled. Default: registry entries represent 2024–2026 shipping firmware behavior; entry_version bumps capture datasheet changes only.
Multiple hybrid inverters per home (e.g., 2× PW3): default assumes independent AC outputs summed at the panel with no inter-unit coordination latency. If Part D requires plant-level controller emulation (Tesla "site" behavior), that controller belongs in Part C's site aggregation layer, not in the registry.
catalog.json hash algorithm is SHA-256 over UTF-8 bytes of each entry file, concatenated in lexicographic path order, then hashed again. Default is normative unless Part C overrides with a documented alternative.
Sonnen sonnenBatterie 10 DC-coupled option: the same model_id covers AC-retrofit and hybrid installs. Default: registry carries two entries (sonnen.sonnenbatterie_10_ac, sonnen.sonnenbatterie_10_hybrid) rather than a mode flag, to keep the coupling enum single-valued per entry. Confirm with Part C before freeze.
Warranty/cycle enforcement: registry stores warranty numbers; whether the simulator enforces warranty-aware dispatch limits (throughput caps) is a Part B/Part D policy decision. Default: telemetry-only (track cumulative throughput and cycle count against warranty values; never restrict dispatch).
End of Part A. Physics evaluation of the curves and windows declared here → Part B. Endpoint mechanics, Rust module layout, OpenAPI definitions → Part C. ERCOT market signals, aggregation, ESIID-level telemetry → Part D.
Part B — Simulation Engine (Physics & Behavior)
Document role. This part specifies the discrete-time simulation engine that advances every
home, battery, inverter, load, and PV array in the fleet. It defines the virtual clock, update
ordering, device physics models (battery, inverter, thermal, degradation), stochastic generators
(load, PV, telemetry noise), grid/outage behavior, and the numerical/performance contract for
the Rust implementation (Part C). Hardware parameters referenced here — nameplate energies,
power limits, efficiency curves, chemistries, coupling topologies, cooling types — are data
from the Part A registry; this part consumes that data, it does not redefine it. Market
price signals, dispatch objectives, and ERCOT settlement semantics are defined in Part D;
this part defines only the mechanical interfaces by which a price/dispatch signal enters the
per-tick update. API surface and crate layout are defined in Part C.
Normative language. The key words MUST, MUST NOT, SHOULD, SHOULD NOT, and MAY are to be
interpreted as in RFC 2119.
B.0 Conventions, Units, and Notation
All internal quantities MUST be SI: power in watts (W), energy in watt-hours (Wh), voltage in
volts (V), current in amperes (A), temperature in degrees Celsius (°C), time in seconds (s,
u64 virtual epoch). Conversion to kW/kWh happens only at the API/telemetry boundary (Part C).
Sign convention (per device, unless stated otherwise): positive = exporting/discharging
toward the home/grid; negative = importing/charging from the home/grid. This holds for
battery DC power P_batt, inverter AC power P_inv, grid exchange P_grid, and metering.
dt denotes the device-level timestep in seconds (default 1; §B.1).
SOC is a dimensionless fraction in [0, 1] of current available (degraded) capacity
Q_avail(t), not of nameplate. Nameplate-normalized values appear only in telemetry fields
explicitly labeled *_nameplate.
All randomness MUST flow through the seeded RNG subsystem (§B.1.4). No other entropy source
(SystemTime, HashMap iteration order, thread scheduling) may influence simulation outputs.
B.1 Time Engine
B.1.1 Virtual clock
The engine advances a virtual clock t_sim (seconds since simulation epoch, u64),
decoupled from wall time. Wall time MUST NOT be read anywhere in engine code; where wall-time
stamping is required (e.g., telemetry emission logs), the value MUST be derived from t_sim.
The simulation epoch t_epoch is a configurable UTC instant (ISO-8601 in config, converted once
to seconds). All scenario content (prices, weather, outages, dispatch schedules) is indexed
against virtual time. Consequence: replaying a scenario that starts 2025-08-15T00:00:00Z
produces identical results whether executed today or next year.
B.1.2 Tick structure and timestep
The engine is tick-based. The fundamental device-level timestep dt is configurable;
the default MUST be dt = 1 s. Allowed range: 1 s ≤ dt ≤ 60 s; sub-second ticks are not
supported (grid-transition dynamics below 1 s are handled by scripted sequences, §B.8.3).
Each tick executes the per-tick update pipeline (§B.1.5) for every simulated entity in a
fixed, deterministic order.
Settlement aggregation: device-level states are integrated and aggregated into ERCOT
5-minute settlement intervals (300 s). The aggregation boundary MUST align to wall-clock
5-minute marks (t_sim % 300 == 0 relative to the epoch, epoch MUST be 5-minute-aligned).
Aggregates (energy counters, average power, interval-end SOC) are emitted at interval close
and are the quantities Part D settles on. Aggregation MUST be derived from per-tick values,
never from a second integration path (single source of truth: the tick loop).
B.1.3 Time acceleration modes
The engine MUST support three execution modes, switchable only when the engine is paused:
realtime — one tick per dt of wall time (i.e., 1×). Used for live demo / hardware-
in-the-loop pacing. Implementation: a pacing loop that sleeps dt_wall − t_compute; if
t_compute > dt_wall, the engine MUST log a RealtimeOverrun event and continue (no catch-up
bursts; pacing skew is recorded, not silently absorbed).
fast_forward { rate: N } — fixed-step fast-forward at up to N × realtime. The engine
runs ticks back-to-back and throttles to at most N·dt of virtual time per wall second.
N = ∞ (unbounded) means run ticks as fast as compute allows; this is the default for batch
simulation. In unbounded mode there is no pacing code on the hot path.
run_until { t_target } — event-driven advance: the engine executes ticks until
t_sim ≥ t_target (or until a stop condition: scenario end, fatal error, API stop command).
This is the primary mode for scenario replay and is how Part D's "settle the next 5-minute
interval" request is implemented: run_until(next_interval_boundary).
All three modes execute identical per-tick code. Acceleration changes pacing only, never
numerics. An advance API call (Part C) MUST produce results bit-identical to the equivalent
number of unbounded fast_forward ticks.
B.1.4 Determinism and seeded RNG
Determinism requirement (hard invariant): given (a) the same scenario input bundle, (b) the
same master seed, (c) the same engine build, and (d) the same timestep configuration, a run MUST
produce bit-identical outputs: identical telemetry streams, identical energy counters, identical
settlement aggregates, identical event logs (excluding wall-clock metadata).
Implementation requirements:
RNG: ChaCha-based CSPRNG in counter mode — use the rand_chacha crate (ChaCha8Rng is
sufficient; ChaCha20Rng acceptable). One master seed (64-bit or 256-bit, config-provided;
if absent in a non-deterministic debug run, the engine MUST generate one and print it).
Stream splitting: independent RNG substreams are derived per entity and per tick so that
parallel execution order cannot change results. Required scheme:
plain
stream_key(entity_id, purpose, tick) = ChaChaRng::seed_from_u64(
    hash64(master_seed, entity_id, purpose_tag, tick))
where hash64 is a fixed non-cryptographic mixing function (e.g., xxh3_64 over the
concatenated little-endian fields, or the SeedableRng stream/word API — pick one, fix it in
code, document the choice). Each (entity, purpose, tick) substream is consumed in a fixed
sequence. purpose_tag is an enum: LoadNoise, PvCloud, TelemetryNoise, DispatchJitter,
OutageTrigger, etc.
Per-tick stream construction cost is acceptable (ChaCha8 seeding ≈ tens of ns); if
profiling shows it hot, an equivalent optimization is a persistent per-entity RNG whose state
is advanced in a deterministic order — but then parallel scheduling MUST NOT change per-entity
draw counts. The stream-splitting scheme above is preferred because it is order-invariant by
construction.
No hidden nondeterminism: iteration order over entities MUST be a sorted index or
insertion-ordered Vec, never HashMap iteration. Parallel stepping (§B.10.4) MUST partition
entities deterministically. Floating-point code MUST NOT use fused operations that vary by
platform unless the build pins them (see §B.10.1); do not enable -ffast-math-equivalent
flags.
A determinism_check test MUST exist: run a seeded 24 h scenario twice (single-threaded and
multi-threaded) and assert byte-equality of the serialized telemetry archives.
B.1.5 Per-tick update pipeline (mandatory ordering)
Each tick t → t+dt executes, in this exact order, per home (homes are independent and may
be parallelized; devices within a home are sequential):
Table
Stage   Name   Description
1   load   Evaluate home load power P_load(t) (§B.6): baseline schedule + stochastic draws for this tick.
2   pv   Evaluate PV DC/AC generation P_pv(t) (§B.7): irradiance interpolation, temperature derate, clipping, cloud noise.
3   price_signal   Inject current grid price / ancillary signal for interval containing t (Part D supplies the time series; engine only indexes it). Also evaluate frequency/voltage event state (§B.8.4).
4   dispatch   Compute the battery power setpoint P_set(t) from the active control mode (self-consumption, TOU arbitrage, reserve-hold, manual, market dispatch from Part D). Includes dispatch latency/jitter model and setpoint clamping to device limits.
5   battery   Integrate battery physics for dt (§B.2): efficiency application, SOC ODE, Thevenin voltage, power-limit sag, thermal coupling, degradation bookkeeping. Produces realized DC power P_batt_realized.
6   inverter   Convert DC↔AC through the inverter model (§B.3): efficiency curve, clipping, standby draw, coupling-path losses (Part A topology), AC/DC microinverter path for Enphase. Produces P_inv_ac.
7   metering   Compute power balance at each meter point (main service, backup sub-panel, PV meter, battery meter): P_grid = P_load − P_pv_ac − P_inv_ac (with sign convention of §B.0). Integrate energy counters (Wh) with revenue-grade quantization deferred to stage 8. Update islanded balance if grid absent (§B.8.3).
8   telemetry   Emit device telemetry (§B.9): apply measurement noise/quantization per device accuracy class, sample-rate decimation, event detection (SOC thresholds, trips, transitions). Telemetry MUST be derived from stage 1–7 results of this tick only.
Ordering rationale (non-negotiable): exogenous flows (load, PV) are established first; the
control signal is computed against fresh exogenous state; physics integrate against the setpoint;
metering closes the balance; telemetry observes the closed balance. Any deviation from this order
changes results and breaks determinism across engine versions.
Setpoint lag: the dispatch stage computes P_set from stage 1–3 values of the same
tick. If a controller requires last-tick metering (closed-loop behavior), it MUST read stage 7
results of tick t−dt, introducing an explicit one-tick loop delay — this models real
controller sample latency and MUST be a named parameter control_loop_delay_ticks (default 1).
B.1.6 Sub-tick integration and large dt
With default dt = 1 s, all models in this part use explicit Euler integration; error analysis
in §B.10.2. For dt > 1 s, the battery and thermal models MUST internally sub-step at ≤ 5 s to
preserve accuracy near power-limit sags and thermal time constants; the sub-step count is
ceil(dt/5) and deterministic. No model may use implicit or adaptive-step solvers
(determinism risk); fixed-step explicit only.
B.2 Battery Electrochemical Model
Fidelity target: pragmatic system-level model, not a porous-electrode PDE (no
Newman/Doyle–Fuller–Newman model, no electrolyte dynamics). The model MUST reproduce, to
telemetry-observable accuracy: energy throughput within ±1 % of nameplate-consistent values,
power-limit sag at SOC/temperature extremes, per-chemistry qualitative differences, and
degradation over multi-year accelerated runs. Cell-level transients < 1 s are out of scope.
B.2.1 State variables
Per battery unit (a "unit" is one registry device from Part A; a home may contain several,
operating as parallel units sharing a controller — see Part A topology section):
Q_avail — current available capacity [Wh] (starts at usable_energy_wh from Part A, decays
per §B.5). This is the denominator of SOC.
soc ∈ [0,1] — state of charge relative to Q_avail.
T_cell — cell temperature [°C] (§B.4).
R_int — internal resistance [Ω] (grows per §B.5).
E_cum_chg, E_cum_dis — cumulative charge/discharge throughput [Wh] (warranty accounting,
§B.5.4).
Controller state: ramp integrator, startup/shutdown timers, min on/off timers (§B.2.6).
B.2.2 SOC ODE with separated losses
Sign convention of §B.0: P_dc > 0 discharging, P_dc < 0 charging, where P_dc is DC-side
power at the battery terminals (before inverter for DC-coupled; the inverter stage has already
back-converted for AC-coupled topologies — see §B.3.4).
Per tick:
plain
if P_dc >= 0:                                  # discharge
    dE_pack = P_dc * dt / 3600                 # Wh removed from pack terminals
    soc(t+dt) = soc(t) - dE_pack / Q_avail
    E_cum_dis += dE_pack
else:                                          # charge
    P_store = |P_dc| * eta_conv_chg(P, T)      # conversion loss at pack
    dE_store = P_store * dt / 3600 * eta_coul  # coulombic loss
    soc(t+dt) = soc(t) + dE_store / Q_avail
    E_cum_chg += |P_dc| * dt / 3600
where:
eta_coul — coulombic (charge) efficiency, dimensionless. Defaults: LFP 0.99, NMC 0.98.
Part A may override per device.
eta_conv_chg(P, T) / eta_conv_dis(P, T) — power-dependent conversion efficiency from the
piecewise curve in Part A (§B.2.3), evaluated at |P_dc| / P_rated and T_cell. Discharge
losses are applied implicitly: the realized AC output is P_dc · η_inv, and the pack-internal
dissipation P_dc · (1 − η_conv_dis) / η_conv_dis (I²R form, §B.2.4) both enter the thermal
model and reduce effective deliverable power.
The round-trip efficiency of the full AC path MUST reproduce the Part A round-trip spec within
±0.5 pp when exercised by the standard test profile: charge at 0.5 C for 2 h, rest 10 min,
discharge at 0.5 C to cutoff. This is a mandatory conformance test (tests/rte_conformance.rs).
B.2.3 Power-dependent efficiency (piecewise curves)
Part A supplies, per device, piecewise-linear efficiency curves sampled at fractional power
breakpoints. Canonical form:
plain
eta_curve: [(p_frac, eta_chg, eta_dis), ...]   # p_frac ∈ [0,1], ascending, min 3 points
Evaluation: linear interpolation between breakpoints; clamp (do not extrapolate) outside
[p_min, p_max]; below p_frac < 0.02 efficiency falls steeply — curve MUST include a
(0.0, 0.0, 0.0) anchor so near-idle conversion is loss-dominated (this is what makes standby
behavior emerge naturally in combination with inverter self-consumption, §B.3.2).
Typical shapes the Part A data encodes (for calibration reference):
Table
p_frac   0.05   0.10   0.25   0.50   0.75   1.00
η (typical)   0.86   0.92   0.955   0.965   0.96   0.945
Temperature dependence: multiply interpolated eta by 1 − k_T · max(0, T_ref − T_cell) for
cold derate, k_T = 0.002 /°C, T_ref = 25 °C. Hot-side efficiency change is neglected
(resistance growth handles it via §B.2.4).
B.2.4 Thevenin equivalent (OCV + internal resistance) — optional but default-on
To make power limits sag at SOC extremes and temperature, the engine MUST implement a
single-state Thevenin model per unit:
plain
V_oc(soc, T_cell)          # open-circuit voltage, from Part A per-chemistry table
R_int(soc, T_cell, SOH_R)  # internal resistance, Part A base value × modifiers
I = (V_oc − sqrt(V_oc² − 4·R_int·P_req)) / (2·R_int)   # discharge, P_req ≥ 0
V_term = V_oc − I·R_int                                # terminal voltage
For charge use the conjugate solution I = (−V_oc + sqrt(V_oc² + 4·R_int·P_req))/(2·R_int)
with P_req < 0 and V_term = V_oc + I·R_int. If the discriminant < 0, the requested power
is infeasible: clamp I to V_oc / (2·R_int) (maximum-power point of the Thevenin source) and
emit a PowerLimited telemetry flag.
Voltage-curve data (Part A provides the table; shapes specified here for validation):
LFP (Powerwall 3, IQ Battery 5P/10, SolarEdge Home Battery): extremely flat mid-range —
V_oc varies ≤ 3 % over soc ∈ [0.15, 0.90]; distinct knees below 10 % and above 95 %. A
17-point SOC table (0, 5, 10, …, 100 %) with monotone cubic interpolation (Fritsch–Carlson,
to avoid overshoot on the flat region) is REQUIRED; linear interpolation produces visible
artifacts at the knees and is not acceptable for LFP.
NMC (Powerwall 2, Sonnen ecoLinx/core+): near-linear slope of ≈ 15–20 % voltage swing over
the mid-range; linear interpolation acceptable, monotone cubic preferred for code sharing.
Resistance modifiers:
plain
R_int = R_base · (1 + k_soc_low · max(0, 0.15 − soc)/0.15)      # low-SOC rise
              · (1 + k_soc_hi  · max(0, soc − 0.95)/0.05)       # high-SOC rise (small)
              · (1 + k_T_r · max(0, T_ref − T_cell)/10)         # cold rise, k_T_r ≈ 0.06/10°C... 
              · (1 + R_growth)                                   # aging, §B.5.3
with k_soc_low = 1.5, k_soc_hi = 0.3, k_T_r = 0.06 per 10 °C below T_ref (i.e., +6 %
per 10 °C of cold), hot-side +0.5 %/°C above 35 °C. These defaults MAY be overridden per
device in Part A. Effect: at 5 % SOC and −5 °C, deliverable continuous power sags to roughly
40–60 % of nameplate — matching observed residential ESS behavior.
Operating-mode flag: thevenin_enabled: bool (default true). When false, power limits
are taken directly from the Part A static limit table (still SOC/temperature-indexed) and no
voltage is computed. Both modes share the SOC ODE of §B.2.2.
B.2.5 Chemistry-specific behavior
The chemistry tag from Part A (Lfp | Nmc) selects behavior modules:
Table
Behavior   LFP   NMC
Voltage curve   flat, cubic-interpolated   sloped
Low-temperature charging   prohibited below 0 °C cell temp: charge power forced to 0 when T_cell < 0 °C; linear recovery 0→full between 0 °C and 10 °C. (Real LFP BMS behavior — lithium plating prevention.)   permitted with derate: charge C-rate limit scales linearly from 0.1 C at −10 °C to full at 10 °C
Discharge at low temp   power derate via R_int cold rise only   same, plus hard cutoff at −20 °C
Calendar aging   weak SOC dependence   strong SOC dependence above 80 % SOC (§B.5.2)
Cycle aging rate   slower (~1.6–2× longer cycle life than NMC at same DOD)   reference rate
High-SOC storage   no special penalty   holding SOC > 90 % raises calendar fade (this is what makes backup-reserve-100 % settings costly for NMC systems, and MUST be visible in multi-month runs)
B.2.6 Operating window, reserves, and dynamic limits
Usable vs nameplate: Part A gives nameplate_energy_wh and usable_energy_wh.
Q_avail initializes to usable_energy_wh. The engine MUST NEVER operate outside
[0, usable]; the hidden nameplate headroom is not modeled as accessible.
Reserve SOC (reserve_soc, a.k.a. backup reserve): dispatch stage clamps discharge so
that projected soc(t+dt) ≥ reserve_soc whenever the grid is present. During an outage
(§B.8), the reserve MAY be released down to outage_min_soc (default 0.05, per Part A
min_soc) — configurable via scenario flag release_reserve_in_outage (default true,
matching Tesla's "reserve preserved until outage" behavior).
Hard window: [soc_min, soc_max] from Part A (e.g., [0.0, 1.0] of usable; some vendors
cap charge at 100 % usable but curtail regen-like fast charge above 97 % — model as
charge_taper_above_soc optional field: above it, charge power limit tapers linearly to
0.05·P_rated at soc_max).
Continuous vs peak power: Part A gives p_continuous and p_peak with peak_duration_s
(e.g., 10 s for motor starts). The engine tracks a thermal-like exponential accumulator for
peak allowance:
plain
if |P| > p_continuous: peak_budget -= (|P| − p_continuous)·dt
else:                   peak_budget += (p_peak − p_continuous)·dt·0.25   # recovery
clamp peak_budget to [0, (p_peak − p_continuous)·peak_duration_s]
Discharge/clamp to p_continuous when budget exhausted.
B.2.7 Ramp rates, startup/shutdown latency, min on/off times
Per device from Part A (defaults in parentheses):
ramp_up_w_s, ramp_down_w_s (default: 100 % rated / 1 s for LFP hybrid systems; 20 %/s for
older AC-coupled). Applied to the setpoint before physics: P_cmd slews toward P_set at
the ramp limit.
startup_latency_s (0.5–3 s; default 1 s): delay from nonzero setpoint to power flow when in
standby. During latency the device draws standby power (§B.3.2).
shutdown_latency_s (default 1 s): symmetric.
min_on_time_s, min_off_time_s (defaults 60 s / 60 s for hybrid inverters; 0 for
battery-integrated AC systems like Powerwall 2/3 which tolerate rapid cycling): once the unit
starts discharging/charging it MUST NOT reverse direction or stop before min_on_time_s;
once stopped it MUST NOT restart before min_off_time_s. The dispatch stage (§B.1.5 stage 4)
enforces this by holding the previous command; a DispatchSuppressed(min_on_off) event is
emitted. Rationale: prevents unrealistic chattering under jittery price signals and matches
hybrid-inverter anti-short-cycle firmware.
All timers are integer tick counters (no floats) to preserve determinism.
B.3 Inverter Model
Scope: the inverter model covers all power conversion stages between battery DC bus, PV, and
the home AC panel, following the topology classification from Part A (AcCoupled, DcCoupled,
DcCoupledHybrid, AcMicro for Enphase's per-battery microinverters).
B.3.1 Efficiency vs. % load
Per device, Part A supplies a piecewise-linear inverter efficiency curve eta_inv(p_frac)
where p_frac = |P_ac| / P_inv_rated (bidirectional; a single curve is used unless Part A
provides separate charge/discharge curves). Requirements on the curve data:
MUST include anchor (0.0, 0.0) and a point at p_frac = 0.05 with realistically poor
efficiency (0.85–0.92): efficiency below 5 % load is bad and the model MUST show it —
this dominates overnight vampire losses in self-consumption mode.
Peak efficiency in the 0.2–0.5 load band, 0.96–0.975 (CEC-weighted values per Part A; e.g.,
Powerwall 2 ≈ 0.97, hybrid string inverters ≈ 0.975 peak).
Slight rolloff at 1.0 load (I²R in magnetics), typically 1–1.5 pp below peak.
Evaluation: linear interpolation, clamped. The conversion equation per tick:
plain
P_ac = P_dc · eta_inv(|P_dc|/P_rated)          # discharge (DC→AC)
P_dc_draw = |P_ac_req| / eta_inv(|P_ac_req|/P_rated)   # charge (AC→DC)
B.3.2 Standby / self-consumption draw
Each inverter/controller draws standby power whenever energized, even at zero throughput:
P_standby from Part A, 20–50 W typical (Powerwall ≈ 30 W; Enphase per-IQ-Battery ≈ 3–5 W
each plus IQ System Controller ≈ 20 W; SolarEdge Home Hub ≈ 25 W; Sonnen ≈ 25 W). Standby draw
is always taken from the AC side (i.e., it increases net grid import or decreases export) and
MUST appear in metering as part of the battery system's net energy balance. During islanded
operation it is supplied by the battery (parasitic load on the island). A deep-sleep mode
(standby_sleep_w, ≤ 5 W, entered after sleep_after_s idle, default vendors: none — flag
exists for future devices) MAY be modeled if Part A provides the fields.
B.3.3 Clipping
Inverter AC output is hard-clamped to P_inv_rated per tick (after efficiency application).
Excess is reported via clipped_energy_wh counters (separate for PV clipping, §B.7.4, and
battery discharge clipping).
For DC-coupled hybrids with a shared inverter (SolarEdge Home Hub, Powerwall 3), the combined
P_pv_dc→ac + P_batt_dc→ac competes for inverter capacity; priority order MUST be
configurable (pv_priority default true: PV first, battery curtailed second — matches
hybrid firmware).
B.3.4 Coupling-path losses (topology from Part A)
AC-coupled (Powerwall 2, Enphase, Sonnen): battery charge path is AC→DC through the
battery's own inverter (η_inv applied); PV reaches the battery through a second conversion
(PV inverter AC → battery inverter DC): total path η_pv_inv · η_batt_inv — the double
conversion MUST be modeled as two explicit stages so telemetry shows each meter point.
DC-coupled (SolarEdge Home Battery via Home Hub, Powerwall 3): PV→battery path is
DC→DC with a single converter efficiency η_dcdc (Part A, ≈ 0.97–0.985); only one
inversion to AC. This is the efficiency advantage Part A's topology data encodes — the engine
MUST route energy accordingly and MUST NOT silently use a single generic path.
Enphase AC microinverter-per-battery: each IQ Battery 5P is an independent AC-coupled
unit; N units in a home are N parallel Thevenin sources with per-unit inverter curves and
per-unit standby draws, coordinated by a home-level controller that splits setpoints
pro-rata by SOC and available power (default split: proportional to remaining headroom).
B.3.5 Grid-following vs. grid-forming
Grid-following (GFL): default on-grid behavior. The inverter synchronizes to the grid
phasor; it cannot produce AC without a grid reference. On grid loss it ceases output
immediately (§B.8.3).
Grid-forming (GFM): required for backup. Per Part A capability flags:
Tesla: Backup Gateway 2 / Powerwall 3 internal transfer — GFM, whole-home or partial.
Enphase: IQ System Controller 2/3 — GFM microgrid with PV microinverter frequency-watt
curtailment control.
SolarEdge: Backup Interface + Home Hub — GFM.
Sonnen: ecoLinx with sonnenProtect / transfer unit — GFM; core+ configurations without a
transfer device are GFL-only and MUST NOT provide backup (engine MUST reject backup-capable
scenario config for such homes at validation time).
GFM mode models: setpoint tracking within the island (P_batt = P_load_island − P_pv_island,
limited by device ratings), a virtual impedance droop if multiple GFM sources share
(P_share ∝ rating), and the frequency-watt curtailment curve of §B.8.3 for PV management.
Phase/pll dynamics are out of scope (sub-cycle).
B.3.6 Anti-islanding and transfer behavior
On grid-loss detection (engine sets grid_present = false, §B.8.1):
GFL-only devices trip to zero output within trip_time_s (Part A; range 0.02–2 s; default
0.1 s for GFL PV inverters per UL 1741/IEEE 1547).
Backup-capable systems execute a transfer sequence with vendor-specific transfer time
(Part A transfer_time_s): Tesla Gateway ≈ 0.1–1 s (model default 0.3 s), Enphase IQ System
Controller ≈ 0.05–0.2 s (default 0.1 s), SolarEdge Backup Interface ≈ 0.5–2 s (default 1 s),
Sonnen ≈ 1–2 s (default 1.5 s). During the transfer window, backed-up loads see zero power
(a "blink"); non-backed loads are dropped permanently for the outage.
With dt = 1 s, sub-second transfers are modeled as: the transition tick carries fractional
energy proportional to (1 − transfer_time_s/dt). The transfer itself MUST be an explicit
state machine (OnGrid → Transferring{ticks_remaining} → Islanded, and reverse
Islanded → ReconnectWait → OnGrid with a reconnect delay reconnect_s, default 300 s,
per IEEE 1547 reconnection practice) — not an instantaneous flag flip.
Grid restoration: reconnect after reconnect_s of stable grid, then resync (GFM → GFL), then
resume normal dispatch. PV GFL inverters additionally wait their own reconnect delay before
producing (default 300 s, matching real UL 1741 behavior — this detail matters for outage
energy accounting in Part D resilience metrics).
B.4 Thermal Model
B.4.1 Model structure
Single-node lumped model REQUIRED; two-node optional (thermal_model: Lumped | TwoNode,
default Lumped).
Lumped (single node):
plain
C_th · dT_cell/dt = Q_gen − (T_cell − T_amb_effective) / R_th
C_th — pack thermal capacitance [J/°C] (Part A; typical 40–80 kJ/°C for a 5–13.5 kWh pack,
giving time constants of hours).
R_th — thermal resistance pack→ambient [°C/W] (Part A).
Q_gen = I²·R_int + |P_throughput|·(1 − eta_stage) — all conversion and ohmic losses from
§B.2.2–B.2.4 become heat (energy conservation MUST hold: electrical losses = heat).
T_amb_effective — the ambient the pack actually sees: outdoor air temperature for
exterior-mounted units (Powerwall, typical), garage/interior temperature for indoor units
(Enphase often garage-mounted): T_indoor = 0.7·T_amb + 0.3·T_setpoint_hvac as a fixed
mixing model (location flag mount: Outdoor | Garage | Indoor from scenario config; default
per Part A vendor-typical).
TwoNode (optional): adds a casing node: cell↔case resistance R_cc, case↔ambient R_ca,
separate capacitances. Required only for devices where Part A marks
thermal_two_node_recommended = true; otherwise keep the hot path simple.
Integration: explicit Euler at dt is stable for these time constants (hours); no sub-stepping
needed beyond §B.1.6.
B.4.2 Ambient feed (Texas climate)
Ambient temperature comes from the scenario weather time series: TMY3/NSRDB-derived hourly
dry-bulb temperatures for the home's Texas climate zone (§B.6.1 climate zones), interpolated
cubically (Catmull-Rom) to tick resolution — linear interpolation of hourly data produces
derivative steps that excite the thermal model unrealistically. An optional seeded diurnal
stochastic component (±0.5 °C, 15-min correlation time) MAY be added (weather_noise: bool,
default false for determinism simplicity).
B.4.3 Cooling types
Part A cooling: Active | Passive | None:
Active (Tesla Powerwall liquid loop, SolarEdge, Sonnen forced air on some models): when
T_cell > T_cool_on (default 30 °C), effective R_th is multiplied by cooling_r_factor
(≈ 0.4) and the cooling system draws P_cool (Part A; 50–150 W) from the AC side, added to
standby accounting. Hysteresis band 2 °C to avoid chattering.
Passive (Enphase IQ Battery — explicitly fanless/passive): R_th fixed, no parasitic
draw; consequence: cell temperature swings with ambient and sustained high power self-limits
via §B.4.4 derating. This MUST be modeled faithfully — it is a real behavioral differentiator
in Texas summers.
None: treated as passive with worse R_th.
B.4.4 Thermal derating
Continuous power limits are multiplied by a derate factor d_T(T_cell), piecewise linear
(Part A may override knots):
plain
d_T = 0.0                     T_cell < −20
      linear 0.5→1.0          −20 ≤ T_cell < 0
      1.0                     0 ≤ T_cell ≤ 40
      linear 1.0→0.6          40 < T_cell ≤ 55
      0.6·linear→0.0          55 < T_cell ≤ 65
      0.0 (trip)              T_cell > 65      # overtemp trip, state machine event
LFP charge prohibition below 0 °C (§B.2.5) applies in addition (charge limit 0, discharge
derated per curve). An overtemp trip latches until T_cell < 50 °C; a ThermalTrip /
ThermalRecovery event pair is emitted.
B.4.5 Thermal runaway — OUT OF SCOPE
Thermal runaway, venting, propagation, and fire dynamics are explicitly out of scope. The
engine treats the overtemp trip of §B.4.4 as the terminal thermal event. No abuse model
(internal short, crush, overcharge-into-runaway) shall be implemented; overcharge is prevented
by the operating window enforcement of §B.2.6. This limitation MUST be stated in generated
telemetry metadata ("thermal_runaway_modeled": false).
B.5 Degradation Model
Toggleable (degradation_enabled: bool, default true) and time-accelerable by construction:
because degradation integrates against virtual time and energy throughput, a 10-year run in
fast_forward unbounded mode ages the fleet 10 years with zero extra machinery. All aging
state MUST be serializable (scenario save/resume).
B.5.1 Structure
Total capacity fade is the sum of calendar and cycle contributions, modeled as capacity loss
fractions L_cal(t), L_cyc(throughput) in [0, 1):
plain
Q_avail(t) = Q_usable_0 · (1 − L_cal(t) − L_cyc(t))
SOH_capacity(t) = Q_avail(t) / Q_usable_0        # telemetry, §B.9
Resistance growth is tracked separately:
plain
R_growth(t) = 0.5·(L_cal + L_cyc) · k_R         # k_R = 2.0: resistance grows ~2× capacity fade
(i.e., at 20 % capacity fade, R_int has grown ≈ 20 %… R_growth = 0.2). k_R MAY be
per-chemistry in Part A.
B.5.2 Calendar aging
Semi-empirical Arrhenius + SOC-stress form, integrated per tick:
plain
dL_cal/dt = k_cal · z_soc(soc, chem) · exp(−Ea / (R·(T_cell_K))) · t^(α−1) · α
Practical implementation uses the incremental square-root-of-time form (aging is
path-dependent, so integrate the rate, don't evaluate a closed form at total time):
plain
rate_cal = k_cal_ref · z_soc · exp( Ea/R · (1/T_ref_K − 1/T_cell_K) )      # fraction/s
dL_cal   = rate_cal · dt / (2 · sqrt(max(t_age_s, 1) / t_ref_s))           # √t law
with:
T_ref = 25 °C, Ea/R = 24 500 / 2 effective (Ea ≈ 24.5 kJ/mol·…, i.e., Ea ≈ 24 500 K·R
→ use Ea_div_R = 24 500/8.314 ≈ 2947 K; a single constant ARRHENIUS_K = 2947.0 MUST be
defined once in code).
z_soc(chem): LFP: 1.0 + 0.3·max(0, soc − 0.9)/0.1 (weak SOC dependence); NMC:
1.0 + 1.5·max(0, soc − 0.8)/0.2 (strong above 80 % — the behavior that penalizes
high backup reserves for Powerwall 2 / Sonnen fleets).
k_cal_ref calibrated so that L_cal at (soc = 0.5, 25 °C, 10 y) equals: LFP ≈ 0.10,
NMC ≈ 0.16. These calibration anchors MUST be encoded as unit tests, not just constants.
B.5.3 Cycle aging
Rainflow counting is NOT required at this fidelity level. Instead use equivalent full
cycles (EFC) with a DOD-weighted Wöhler-style accumulation:
Per tick, accumulate fractional throughput:
plain
dEFC = (|dE_pack| / 2·Q_avail)               # 1 EFC = one full charge+discharge of Q_avail
Apply cycle fade via a DOD-sensitive rate. Track a rolling estimate of cycle depth dod_avg
(exponential moving average over EFC, time constant 20 EFCs) and use:
plain
dL_cyc = dEFC · k_cyc(chem) · (dod_avg)^β        # β = 1.1 (superlinear in DOD)
Calibration anchors (20 % fade): LFP ≈ 4 000 EFC at DOD 0.8, 25 °C; NMC ≈ 2 000 EFC at DOD
0.8, 25 °C. Temperature coupling: multiply dL_cyc by the same Arrhenius factor as calendar
aging but with ARRHENIUS_K_CYC = 1 500 K (weaker T dependence for cycling).
B.5.4 Throughput and warranty accounting
Per unit, the engine maintains E_cum_chg, E_cum_dis (§B.2.1) and computes
throughput_mwh = (E_cum_chg + E_cum_dis)/2. Part A gives warranty terms per device
(energy-throughput cap and/or cycle cap and years, e.g., Tesla Powerwall: 10 y, unlimited
cycles with solar, 37.8 MWh aggregate on some PW2 SKUs; Enphase: 10 y / 4 000 cycles for 5P;
SolarEdge/Sonnen: 10 y with throughput caps). The engine MUST:
expose warranty_throughput_used_pct, warranty_cycles_used_pct, warranty_years_used_pct
in telemetry;
emit WarrantyThreshold{crossed: 70|90|100 %} events;
NOT enforce warranty in physics (degradation continues; warranty is a commercial overlay —
Part D may price it).
B.5.5 SOH in telemetry
soh_pct = 100·SOH_capacity is a first-class telemetry field (§B.9), reported per unit with
0.1 % resolution, plus soh_resistance_pct = 100·(1 + R_growth). SOH affects behavior only
through Q_avail (smaller denominator → faster SOC slew) and R_int (more sag) — this closes
the feedback loop between aging and dispatchability that fleet-aggregation studies (Part D)
require.
B.6 Load Profile Generation
B.6.1 Home archetypes and climate zones
Each home is parameterized by an archetype:
plain
HomeArchetype {
  sqft: u32,                        # 800..6000
  hvac: HvacType,                   # CentralAC | HeatPump | WindowUnits | None(rare in TX)
  water_heat: Resistance | HeatPump | Gas,
  occupancy: u8,                    # persons
  pool: bool,
  ev: Option<EvConfig>,             # {battery_kwh, daily_miles, home_charge_kw, plug_in_schedule}
  climate_zone: TxClimateZone,      # see below
  vintage: Pre1980 | Y1980_2000 | Post2000,
  backup_panel: BackupConfig,       # WholeHome | CriticalLoads{critical_load_kw_share}
}
Texas climate zones (for both weather and load calibration): TX_GulfCoast (Houston —
hot-humid), TX_Central (Austin/San Antonio/Dallas — hot, mixed), TX_North (Panhandle —
colder winters), TX_West (El Paso/Midland — hot-dry). Each zone maps to a weather station
set for the temperature/irradiance feeds (§B.4.2, §B.7.1). ERCOT weather zones map
approximately: GulfCoast→Coast, Central→South Central/North Central, North→North, West→West —
the exact ERCOT weather-zone mapping belongs to Part D; Part B only needs the climate zone.
B.6.2 Data sources and calibration
Load synthesis is calibrated against, in priority order:
NREL ResStock (Texas buildstock, end-use load profiles at 15-min) — primary source for
archetypal daily/seasonal shapes by end use (HVAC, water heat, appliances, lighting, plug
loads, pool, EV). The build pipeline SHOULD pre-extract ResStock Texas AMY/TMY end-use
profiles into an internal binary asset (assets/load_shapes/{zone}/{archetype_key}.bin);
the runtime MUST NOT parse ResStock CSVs on the hot path.
Pecan Street Dataport (Austin, 1-min/15-min whole-home and circuit-level) — primary
source for high-resolution variability statistics: 1-min residual distributions,
ramp distributions, and appliance-spike signatures. Used to fit the stochastic layer
(§B.6.3), not for raw replay.
EIA RECS (Texas region) — annual/monthly consumption cross-checks: generated homes'
annual kWh MUST land within RECS distribution quartiles for their archetype (validation
test, not runtime).
Attribution requirement: the spec deliverable MUST document which dataset version informed each
shape table (a DATA_SOURCES.md in the assets crate), since Part C's config exposes
load_shape_version.
B.6.3 Synthesis algorithm
Per home, per tick (stage 1 of §B.1.5):
plain
P_load(t) = Σ_enduses [ S_e(dow, hour, season, zone) · scale_e(archetype) ]   # archetypal shape
          + R_hvac(t)                                                          # HVAC stochastic (below)
          + R_app(t)                                                           # appliance spikes
          + R_base(t)                                                          # 1-min residual noise
          + P_ev(t)                                                            # EV charging session model
Archetypal shapes S_e: 15-min resolution lookup by (day-type ∈ {weekday, weekend},
hour-of-day, month, zone), per end use, interpolated linearly within 15 min. Scaled by
archetype parameters: HVAC ∝ sqft·(climate factor)·(vintage efficiency factor); water heat ∝
occupancy; plug/lights ∝ sqft^0.7·occupancy^0.5 (sublinear exponents fitted to RECS/ResStock).
HVAC stochastic layer R_hvac: thermostatically driven loads are the dominant Texas
end use and are temperature-coupled. Model: hysteresis cycling around setpoint with
duty cycle dc(T_amb) from the ResStock shape table, plus a seeded ±10 % cycle-length jitter
and ±5 min phase offset per home (drawn once at scenario init from stream tag
LoadPhase) so the fleet does not cycle in lockstep. Heat-pump vs resistance vs AC capacity
and COP come from archetype; defrost and auxiliary-heat behavior for heat pumps in
TX_North winter MUST be included as a distinct mode (aux strips: step +3–5 kW below
balance point ≈ 2 °C).
Appliance spikes R_app: marked point process — event arrivals Poisson with rate
λ(hour, occupancy); each event draws an appliance type (water heater element, dryer, range,
washer, dishwasher, pool pump schedule, microwave, etc.) with fixed (power, duration)
signature from a small table fitted to Pecan Street circuit data. Durations 1–90 min, powers
0.3–5 kW. All draws from the LoadNoise stream.
Base residual R_base: AR(1) noise on the 1-min residual, σ ≈ 60 W, correlation time
5 min, fitted to Pecan Street residuals; clipped so P_load ≥ 0.05 kW (vampire floor).
EV P_ev: session model — plug-in at schedule draw (evening ~18:00±2 h for weekday
commuters), charge at home_charge_kw (3.3–11.5 kW) until daily_miles × kwh_per_mile
(0.28) delivered or departure. V2X is out of scope (state it; EV is a load only).
Resolution: the generator MUST produce 1-min-resolution native load with sub-minute
downsampling to tick resolution by holding the 1-min value constant within the minute (with the
appliance/HVAC layers providing intra-minute on/off transitions at tick resolution). 15-min
resolution mode (load_resolution: Min15) MUST be available for fast fleet screening runs; it
disables the intra-minute stochastic layers and uses shape-table values plus scaled noise.
Validation targets (mandatory tests): fleet-average load factor 0.45–0.6; summer afternoon
peak coincidence with ERCOT 4CP hours (Part D); per-home annual kWh within RECS quartiles;
1-min ramp distribution within 20 % of Pecan Street reference distribution (KS-test).
B.6.4 Critical loads vs. whole home (backup split)
Per Part A topology: WholeHome backup (Tesla Gateway whole-home, Enphase with full
interconnection) — all loads islanded. CriticalLoads: a fixed subset of end uses islands
(refrigerator, selected lights/plugs, one HVAC zone or none, network equipment); the rest is
dropped at transfer. Model: critical_share per end use (Part A/scenario table; default
critical ≈ 25–35 % of average load, capped at the backup sub-panel rating, e.g., 60 A). During
islanded operation the engine computes P_load_island = Σ critical and P_load_dropped
separately; both appear in telemetry. HVAC on critical panels: allowed only if
hvac_on_backup: bool (default false — realistic; large compressor inrush exceeds most
single-unit ESS capability; if true, an inrush model applies: 3× running power for 0.5 s,
served from p_peak budget §B.2.6).
B.7 PV Model
B.7.1 Irradiance source
Per-home irradiance from NREL NSRDB (PSM v3, 30-min or hourly GHI/DNI/DHI for the home's
lat/lon, TMY or a specific year from the scenario bundle). Preprocessing: the build pipeline
converts NSRDB to per-site binary series; runtime interpolates to tick resolution:
GHI/DNI/DHI interpolation: cubic for ambient temperature; linear for irradiance between
samples, with an optional cloud-variability overlay (§B.7.5) supplying intra-sample
structure. Time is solar-position-aware: interpolation MUST be done in true solar time to
avoid smearing sunrise/sunset.
B.7.2 Plane-of-array and DC power ("PVWatts-style single-diode-lite")
Full single-diode five-parameter modeling is explicitly not required. Required pipeline
(per home array):
POA irradiance: Perez or Hay-Davies transposition (pick Hay-Davies — simpler,
adequate at this fidelity) from GHI/DNI/DHI using array tilt and azimuth from the
scenario (Part A does not fix array geometry; it is home-scenario data). Solar position via a
standard PSA/NREL SPA-lite algorithm (accuracy ≤ 0.05°; must be pure-function deterministic).
DC power (PVWatts-style derate):
plain
P_dc = P_stc · (G_poa / 1000) · [1 + gamma_pdc · (T_cell_pv − 25)] · η_system
T_cell_pv = T_amb + (G_poa/1000)·ΔT_noct          # ΔT_noct ≈ 30°C, open rack 25
η_system = η_inv_pv · η_mismatch · η_wiring · η_soiling · η_availability
with gamma_pdc = −0.0035/°C (mono-Si default), η_soiling = 1 − 0.02·soiling_factor
(monthly soiling table per zone, West TX dust higher: up to 5 %), other derates from a
PVWatts-consistent default stack (system losses ≈ 14 %). For DC-coupled topologies the PV
inverter stage is the shared hybrid inverter (§B.3.4) and η_inv_pv is applied there, not
here.
3. DC/AC ratio and clipping: array P_stc vs inverter P_ac_rated from scenario config
(default ratio 1.2). Clipping at the inverter per §B.3.3; clipped energy counted.
B.7.3 Orientation and tilt
Multiple sub-arrays per home MUST be supported (e.g., east + west roofs): each sub-array has
its own (tilt, azimuth, P_stc); POA computed per sub-array; sum before inverter. Default when
scenario omits geometry: single array, tilt = latitude, azimuth = 180° (south), with a seeded
±20° azimuth jitter per home for fleet realism.
B.7.4 Per-home profiles and shading
Fixed shading derate per home (shading_factor ∈ [0, 0.3], default 0) applied to POA.
Snow/soiling step events: out of scope beyond the monthly soiling table (Texas: snow rare;
if desired, scenario weather already encodes low GHI).
Per-home PV profiles are therefore: NSRDB site series → sub-array POA → derates → inverter →
AC power at tick resolution, plus optional §B.7.5 noise.
B.7.5 Cloud-variability noise (optional, seeded)
At dt = 1 s, interpolated 30-min irradiance is unrealistically smooth. Optional overlay
(pv_cloud_noise: bool, default true for device-level studies, false for pure settlement
runs):
Draw a clearness multiplier m(t) ∈ [0.2, 1.05] from a seeded stochastic process: Markov
sky-state chain (clear/partly/broken, transition matrix per zone and season, fitted-order
magnitudes only) + within-state AR(1) flicker (σ up to 30 % of GHI in broken state,
correlation time 30 s).
All draws from the PvCloud RNG stream (§B.1.4). Fleet spatial correlation: homes within the
same cloud_cell (≈ 10 km grid over ERCOT territory) share a common component:
m_home = w·m_cell + (1−w)·m_local, w = 0.6. The cell-level process is its own stream
keyed by cell id — this preserves determinism under home-count changes for shared cells.
The overlay MUST be energy-neutral over each hour on average (normalize multiplicatively
against the interpolated value's hourly mean, clamped) so settlement-interval energies match
NSRDB within ±2 % regardless of tick-level flicker. This is the key requirement making
1-s realism compatible with Part D settlement fidelity.
B.8 Grid & Outage Behavior
B.8.1 Grid state model
Per home, grid_present: bool (default true), plus grid quality state:
plain
GridState {
  present: bool,
  voltage_pu: f64,        # nominal 1.0; sag/swell events 0.85..1.1
  frequency_hz: f64,      # nominal 60.0; events 59.0..61.0
}
Outages are scheduled via the scenario API (Part C endpoint surface; semantics here):
Planned outage: scenario entry {t_start, t_end, home_selector} — deterministic, no RNG.
Unplanned/stochastic outage: scenario supplies a hazard rate (outages per home-year, per
region) and a duration distribution (lognormal; Texas-default median 2 h, σ 1.2 — calibrated
to SAIDI/SAIFI magnitudes; exact parameters are scenario data). Draws use the OutageTrigger
RNG stream at tick granularity (hazard per tick = λ·dt).
Weather-driven correlated outages: scenario MAY declare a regional outage event affecting
a fraction of homes in a zone (e.g., Uri-style winter storm: multi-day, correlated). Such
events are planned-type entries with a selector, not per-home hazard draws, to keep fleet
correlation explicit.
B.8.2 Behavior on grid loss
Sequence per home (ties together §B.3.5–B.3.6):
grid_present cleared at tick t_out. GFL devices trip within trip_time_s.
Backup-capable system enters Transferring state for transfer_time_s (vendor table,
§B.3.6): backed-up loads unserved during transfer (energy-not-served counted).
Islanded state: GFM inverter(s) form the island microgrid (§B.8.3). Non-backed loads
dropped for the duration.
Grid restored: ReconnectWait (reconnect_s, default 300 s) → resync → OnGrid. GFL PV
waits its own reconnect delay before resuming production.
B.8.3 Islanded microgrid power balance
Per tick in islanded state, the home-level balance MUST close:
plain
P_batt_ac + P_pv_ac + P_unserved = P_load_island + P_curtail + P_standby_losses
Rules, in priority order:
Serve load: P_batt_set = P_load_island − P_pv_ac, bounded by device limits
(Thevenin-sagged, thermally derated).
PV continues only if grid-forming is present and PV inverters are
microgrid-compatible (per Part A: Enphase IQ PV microinverters — yes, frequency-watt
controlled; SolarEdge with Home Hub — yes; string GFL PV without compatible control — trips
and stays off for the outage). If PV is GFL-only and non-compatible, P_pv_ac = 0 while
islanded.
Battery full → curtail PV: when soc ≥ soc_max (or charge power limit < PV surplus),
the GFM source raises island frequency per a droop curve: f = 60 + k_f·(surplus_frac),
k_f such that PV reaches zero output at 62 Hz (frequency-watt per IEEE 1547/UL 1741 SA
default curve: start 60.2 Hz, full curtail 62.0 Hz). PV output follows the frequency-watt
curve with a 1 s response lag. P_curtail accounts the curtailed energy. This feedback is
REQUIRED — without it, islanded multi-day scenarios overestimate served load badly.
Battery empty → load shed: when the battery hits outage_min_soc and PV cannot cover
load, the system sheds: P_unserved = deficit; a BackupExhausted event fires; the GFM
source shuts down (island goes dark) until PV production alone can restart it
("black-start from PV", supported per Part A flag pv_blackstart — Enphase and Powerwall 3
support sunlight restart; older PW2 does not) or grid returns.
Unserved energy E_unserved is accumulated per outage event and exported as the
resilience metric Part D consumes.
B.8.4 Frequency / voltage event response hooks
To support ERCOT ancillary simulation (Part D: RegUp/RegDown, RRS, ECRS dispatch), the engine
exposes per-tick grid frequency/voltage inputs (frequency_hz, voltage_pu in GridState,
default 60.0/1.0; scenario or Part D drives deviations):
Frequency-watt response (on-grid): IEEE 1547 default curve — above 60.036 Hz (default
deadband), discharge… rather, export is curtailed and charging encouraged proportionally;
below 59.964 Hz the inverse. Droop slope configurable (default 5 %). Response time: one tick
plus control_loop_delay_ticks.
Volt-var / volt-watt: optional (grid_support_functions: bool, default false); when
enabled, reactive power is modeled only as an apparent-power headroom reduction
(S_rated fixed; |Q| = k·(|V_pu − 1|) up to curve; P_avail = sqrt(S² − Q²)). Full
reactive-power flow is out of scope (no distribution feeder model).
Ancillary dispatch response: Part D delivers fleet-level dispatch signals (MW setpoints
per interval or per 4-s regulation tick); Part B's responsibility is only: (a) accept a
per-home power bias P_bias(t) at stage 4 with configurable latency (default 2 s +
DispatchJitter draw, uniform 0–3 s, modeling ADER telemetry/command latency), and
(b) report realized response in telemetry so Part D can score performance and settle.
Regulation-signal following at 4-s granularity runs naturally on the 1-s tick engine.
B.9 Telemetry & Metering Model
B.9.1 Metering architecture
Per home, meter points (aligned with Part A topology diagrams): MAIN (service entrance,
bidirectional), PV_AC, BATT_AC, BACKUP_PANEL (if present), GEN (future). Each meter
integrates per-tick power into Wh counters (import_wh, export_wh for MAIN; wh for
others) as exact f64 accumulators internally; reported values are quantized per §B.9.3.
Counters are monotonic per run and serializable.
Per battery unit, the BMS telemetry set: soc_pct, soh_pct, p_batt_w, v_term,
i_pack_a, t_cell_c, state (enum below), energy_charged_wh, energy_discharged_wh,
warranty_* (§B.5.4), limit flags (PowerLimited, ThermalDerated, ChargeInhibitedCold).
Device state enum (shared vocabulary, serialized as strings):
plain
Standby | Charging | Discharging | Idle (zero setpoint, contactors closed)
| Transferring | Islanded | Tripped(reason) | Faulted(reason) | Off
B.9.2 Vendor-realistic granularity and rates
Table
Data   Resolution / rate (default)
Device power (W)   per-tick internally; reported 1 s (Tesla-class), 5 s (SolarEdge-class), 15 s (Enphase-class), 60 s (Sonnen cloud-class) — per Part A telemetry_rate_s
Energy counters (Wh)   1 Wh quantization, reported per telemetry sample
SOC (%)   0.1 % reported resolution; update per telemetry sample
SOH (%)   0.1 %, updated daily (vendors do not stream SOH continuously)
Temperature (°C)   0.5 °C quantization
Voltage / frequency   0.1 V / 0.01 Hz, only on devices that expose them (per Part A exposes_grid_phasor)
Fleet rollups   5-min settlement aggregates (§B.1.2)
Telemetry emission MUST be lossless-then-decimated: the engine evaluates true values each
tick, applies noise/quantization at emission time, and decimates to the device rate by
sampling (not averaging) the noisy series — matching how real gateways report. Raw per-tick
truth is available on a separate debug_truth stream (never noise-applied) for validation.
B.9.3 Measurement noise and accuracy classes
Per device, Part A assigns an accuracy class:
Revenue-grade (main meter, PV production meter): ±1 % of reading (class 1.0): reported
P̂ = P·(1 + ε) + q, ε ~ N(0, σ) with σ = 0.005 (i.e., 99 % within ±1.5 %), q uniform
quantization noise to the reported resolution. Energy counters accumulate quantized
reported power so that counter-vs-power integration is self-consistent, as on real meters.
BMS-estimate class (SOC, internal energy counters on battery units): ±5 % worst case —
model SOC reported value as soc_true·(1+ε_soc) + drift, σ_soc = 0.02 random walk
component plus a slow bias drift ∈ [−0.03, +0.03] drawn per unit per run (real BMS SOC
estimators carry persistent bias until a full-charge recalibration). Recalibration event:
when a unit reaches soc ≥ 0.995 (full charge) or ≤ 0.02 (empty), the bias resets toward
0 (draw new bias ≤ 0.5 %). This reproduces the well-known "SOC jumps after full charge"
behavior and matters for dispatch algorithms that trust reported SOC.
Temperature/voltage: ±0.5 °C / ±0.5 % Gaussian, quantized.
All noise draws come from the TelemetryNoise stream (§B.1.4). Noise is applied only to
emitted telemetry, never fed back into physics — except where a control loop explicitly reads
telemetry (controller_uses_reported_soc: bool, default false; set true to study
estimator-driven dispatch errors).
B.10 Numerical & Performance Notes (Rust)
B.10.1 Numeric types
Recommendation: f64 everywhere in engine physics; no fixed-point.
Justification: (a) all magnitudes (W, Wh, s, °C) are far from f64 precision limits — a
10-year run accumulates ≈ 10¹⁰ Wh counters at 1 Wh quantization, well within the 2⁵³ (≈ 9·10¹⁵)
exact-integer range; (b) determinism risk from f64 is platform variance, not precision — and
is controlled by: no -C target-cpu=native with FMA contraction differences on the hot path
(pin codegen flags in Cargo.toml profile; either disable FMA contraction or mandate one
target), no transcendentals in per-tick code except through a vetted libm-consistent path
(std's sin/exp are fine on a fixed toolchain; pin the toolchain via rust-toolchain.toml
— Part C owns toolchain pinning, this part owns the requirement); (c) fixed-point would
complicate the efficiency-curve and Thevenin algebra for zero measured benefit. If a future
embedded target requires fixed-point, isolate it behind the quantity newtypes below; do not
pre-optimize.
Use newtypes (struct Watts(f64), struct WattHours(f64), struct Soc(f64)) with checked
conversions at API boundaries to eliminate unit-confusion defects. no_std compatibility is
not required (the engine may use std, allocation at init, and threads).
B.10.2 Integration error budget
Explicit Euler at dt = 1 s: SOC error per tick is O(dt²·d²soc/dt²); with max C-rate 1 C the
dominant error source is piecewise-efficiency interpolation, ≈ 0.1–0.3 % per charge cycle —
acceptable against the ±1 % RTE conformance target (§B.2.2). Thermal: time constants ≥ 1000 s
→ Euler error negligible. The one stiff pair is Thevenin clamping at the SOC knee (LFP):
handled by the clamp + monotone-cubic table, not by smaller steps. The §B.1.6 sub-stepping
rule covers dt > 1 s.
B.10.3 Memory & allocation discipline
No per-tick heap allocation on the hot path. All per-entity state lives in flat,
preallocated struct-of-vectors arenas (Homes, BatteryUnits, Inverters, indexed by
u32); telemetry emission writes into preallocated ring buffers per output stream.
Rationale: allocation jitter dominates cache behavior at 10⁴–10⁵ entities; arena layout
gives contiguous iteration for the parallel stepper.
Scenario init MAY allocate freely. A debug_assert!-guarded allocation counter in tests
MUST show zero allocations across a 1-h simulation after init.
RNG stream construction per (entity, tick) is stack-only (§B.1.4).
B.10.4 Parallelism
Homes are independent between ticks (fleet-level couplings — price signals, cloud cells,
dispatch commands — are computed at tick start and broadcast as read-only slices). Step homes
with rayon: par_chunks_mut over the home arena with a fixed chunk size
(max(1, n_homes / (4·n_threads)), computed once) so partitioning is deterministic regardless
of thread scheduling. Per-tick barrier via rayon's implicit join; no cross-home locks.
Reduction (fleet aggregates) via deterministic tree reduce over the fixed chunk order —
f64 addition is not associative, so par_iter().sum() (nondeterministic partitioning under
work stealing) MUST NOT be used for outputs; reduce chunk-locally then combine in index order.
B.10.5 Compute budget (10 000 homes × 1 s ticks, faster than realtime)
Per home-tick work: load shape eval + noise (~60 flops + 1 RNG stream), PV chain incl. solar
position (~200 flops; SPA-lite trig amortized per minute via cache), battery Thevenin +
efficiency interp (~80 flops + 1 sqrt), inverter curve (~30), thermal/degradation (~50),
metering/telemetry (~40 + 1–2 RNG draws). Total ≈ 500 flops + ~4 RNG substreams
(ChaCha8 ≈ 30 ns each seeded, or ≈ 1–2 ns per 32 bytes amortized with persistent streams) +
~200 B state touched.
Single core: ≈ 1–3 µs per home-tick (dominated by cache traffic and RNG seeding, not flops).
10⁴ homes → 10–30 ms per tick single-core.
8–16 cores with rayon: 1–4 ms per tick → 250–1000× realtime on one workstation-class
machine. Requirement: ≥ 100× realtime for 10 000 homes at dt = 1 s MUST be demonstrated
by the benches/fleet_10k.rs criterion benchmark on the CI reference machine; the 10×
safety margin absorbs telemetry I/O (which MUST be async/batched — Part C) and Part D
settlement hooks.
A 1-year, 1 000-home degradation study (3.15·10⁷ ticks) completes in ~1–4 core-hours —
practical for overnight fleet-aging analyses.
B.10.6 Serialization & replay
Engine state (all arenas + RNG-relevant counters + event logs) MUST serialize via serde to a
versioned snapshot format (Part C owns transport). Resume-from-snapshot MUST be bit-identical
to uninterrupted runs — tested by tests/snapshot_replay.rs running a scenario with a
save/resume at its midpoint. RNG stream-splitting (§B.1.4) makes this trivial: streams are
stateless functions of (seed, entity, tick), so no RNG state needs serializing at all beyond
the master seed.
B.11 Conformance Test Checklist (binding)
rte_conformance — round-trip efficiency within ±0.5 pp of Part A spec per device (§B.2.2).
determinism_check — seeded 24 h run bit-identical, single- vs multi-threaded (§B.1.4).
pipeline_order — a probe device asserting stage ordering invariants (§B.1.5).
lfp_cold_charge_block — LFP charge power = 0 below 0 °C cell temp; NMC derated only (§B.2.5).
thevenin_sag — deliverable power at 5 % SOC / −5 °C within 40–60 % of nameplate for LFP reference device (§B.2.4).
standby_vampire — overnight self-consumption loss matches Part A standby spec ±10 % (§B.3.2).
transfer_sequence — outage transfer state machine timing per vendor defaults (§B.3.6, B.8.2).
island_curtailment — islanded PV curtailment activates when battery full; island energy balance closes per tick (§B.8.3).
degradation_anchors — calendar/cycle calibration anchors per chemistry (§B.5.2–B.5.3).
load_validation — RECS quartile fit, load factor, Pecan Street ramp KS-test (§B.6.3).
pv_energy_neutrality — cloud-noise overlay matches NSRDB interval energy within ±2 % (§B.7.5).
telemetry_accuracy — noise/quantization classes applied at emission only; truth stream untouched (§B.9.3).
snapshot_replay — midpoint save/resume bit-identical (§B.10.6).
fleet_10k_benchmark — ≥ 100× realtime at 10 000 homes (§B.10.5).
End of Part B.
Part C — API & Rust Architecture
Scope: this document specifies the Rust workspace, crate boundaries, HTTP API surface, auth/tenancy model, concurrency/state design, observability, testing strategy, and developer experience for the residential battery fleet simulator (batsim). It references Part A (OEM device registry), Part B (physics/time engine), and Part D (ERCOT market semantics) without duplicating them. Everything here is written for AI-agent implementers: exact crate names, exact endpoints, exact commands, exact schema sketches.
C.1 Design Goals
Single binary. cargo install --path crates/batsim-server (or the Docker image) produces one statically-linked executable, batsim. No external services are required for the default run: no database, no message bus, no cache. Optional persistence (C.5.4) is embedded.
Sub-millisecond in-process stepping. The simulation core (batsim-core) is a pure, synchronous, allocation-frugal Rust library. Stepping one home one tick is plain function calls over &mut Home state — target ≤ 1 µs/home/tick amortized, and ≤ 1 ms for a full 10k-home tick on one core with rayon parallelism well under that. The HTTP layer never sits in the hot path of a tick; it enqueues intents and reads results.
REST + streaming API is the ONLY interface. There is no required SDK, FFI, IPC, or in-process embedding contract. Every capability — fleet composition, time control, dispatch, telemetry, vendor-API mimicry — is reachable over HTTP. batsim-cli and batsim-client are thin shells over the same OpenAPI document and exist only to dogfood it.
OpenAPI 3.1 generated from code — single source of truth. The spec is generated at build/run time by utoipa from handler and schema annotations. It is served live at GET /openapi.json and rendered by Swagger UI at GET /docs. Clients for any language are generated on the fly; nothing is handwritten twice. Example generation commands (run against a live server or a dumped openapi.json):
bash
# openapi-generator-cli — e.g. a Python client
npx @openapitools/openapi-generator-cli generate \
  -i http://localhost:8080/openapi.json \
  -g python -o clients/python --additional-properties=packageName=batsim_client

# Fern
fern init --openapi http://localhost:8080/openapi.json && fern generate

# Stainless
stainless generate --openapi http://localhost:8080/openapi.json --languages python,typescript
Deterministic. Given the same scenario seed, registry version, ERCOT replay data, and command log, any two runs produce byte-identical snapshots (C.7.4). All randomness flows from rand_chacha::ChaCha8Rng streams seeded per scenario and per home; wall-clock time never enters the simulation core — only the virtual clock of Part B.
Runs in Docker. Multi-stage build to a gcr.io/distroless/cc-debian12 runtime (C.8.2); single container, single exposed port, config mounted as a file.
Config via file + env. figment-style layering: batsim.toml ← BATSIM_* env vars ← CLI flags (CLI wins). Env override syntax: BATSIM_SERVER__PORT=9090 (double underscore = nesting). Every config key is documented in the OpenAPI-adjacent GET /v1/system/config (redacted) and in --print-config.
Scriptable by another AI agent. No interactive prompts anywhere in the server. Every mutating endpoint is idempotent or explicitly versioned (C.3.10). Errors are RFC 9457 application/problem+json with machine-readable type URNs. The CLI mirrors the API 1:1 so an agent can choose either. Exit codes: 0 success, 2 usage/config error, 3 API error (with problem JSON on stderr).
C.2 Rust Workspace Layout
C.2.1 Workspace
plain
batsim/
├── Cargo.toml                 # [workspace] + [workspace.dependencies] (single version source)
├── rust-toolchain.toml
├── crates/
│   ├── batsim-core/           # simulation engine: homes, devices, tick loop, snapshots
│   ├── batsim-registry/       # Part A catalog loading + validation
│   ├── batsim-ercot/          # Part D adapters: price/AS replay + synthetic sources
│   ├── batsim-server/         # axum app: routes, SSE/WS, auth, OpenAPI; also the binary
│   ├── batsim-client/         # thin generated+hand-tuned Rust client (dogfooding)
│   └── batsim-cli/            # clap admin CLI for humans (uses batsim-client)
├── api/                       # vendored copy of generated openapi.json (CI-checked freshness)
├── config/batsim.toml         # default config
├── deploy/Dockerfile
├── deploy/docker-compose.yml
└── tests/                     # workspace-level integration tests (golden, determinism, load)
C.2.2 Crate responsibilities and dependency direction
Dependency direction is strictly acyclic: core ← registry, core ← ercot, {core, registry, ercot} ← server, server ← client (only via HTTP, no code dep — batsim-client depends on generated types, not on batsim-server), client ← cli.
Table
Crate   Kind   Contents   Hard rules
batsim-core   lib, sync, no async, no tokio   Home, Battery, Inverter, PvArray, LoadProfile, TickContext, SimWorld, command queue, snapshot (de)serialization, per-home RNG streams. Physics per Part B.   No tokio, no axum, no network, no std::time::SystemTime/Instant in simulation paths. Instant allowed only behind a #[cfg(feature = "bench")] timing shim. All time is SimTime from Part B.
batsim-registry   lib, sync   Embedded catalog via include_str!("../catalog/*.json") (default) or directory loading (BATSIM_REGISTRY_DIR) for user catalogs; schemars JSON-Schema validation of catalog files at load; typed BatteryModel, InverterModel (Part A schema).   Catalog is immutable after load; loading happens once at startup. Validation errors enumerate every broken entry, not just the first.
batsim-ercot   lib, sync (+ small async file/http fetch behind a feature)   PriceSource trait { Replay { date }, Synthetic { profile, seed } }, AS program models, dispatch-signal generator — thin adapter over Part D semantics. Returns plain data structures; no I/O inside tick loop.   Replay data is loaded and indexed before start; the tick loop reads from memory.
batsim-server   lib + bin (batsim)   axum router, utoipa OpenAPI assembly, SSE/WebSocket streaming, auth middleware, command fan-out executor, snapshot store, metrics, config, main().   A thin shell: handlers validate → translate to batsim-core calls → serialize. No physics, no ERCOT math in handlers.
batsim-client   lib   Types + path functions generated from openapi.json (via progenitor or openapi-generator rust output, committed) plus a thin ergonomic wrapper. Exists to prove the spec is sufficient; CI fails if the client drifts from the spec.   May not import any other workspace crate.
batsim-cli   bin (batsimctl)   clap subcommands mirroring API: homes create/list/get/delete, fleets apply, sim start/pause/step/run-until, dispatch charge ..., telemetry tail, snapshot save/restore, docs open.   Output modes: --json (default, machine-first) and --table.
Why this split. The single most valuable property of the system is deterministic, fast, testable simulation. That is only achievable if the core is a pure synchronous library with zero async coloring: unit tests call world.step() directly, golden tests run without a server, and the determinism test is a plain #[test]. The server is a replaceable shell; if axum were swapped for another framework, no line of batsim-core changes. Registry and ERCOT are separate because they have independent release cadences (new device models, new ERCOT products) and independent validation logic.
C.2.3 Toolchain, MSRV, key dependencies
rust-toolchain.toml:
toml
[toolchain]
channel = "1.83.0"          # pinned exact; bump deliberately, recorded in CHANGELOG
components = ["clippy", "rustfmt", "rust-src"]
targets = ["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu"]
MSRV = pinned stable (1.83.0). CI also builds on stable and beta as non-blocking canaries.
Key dependencies (versions pinned in [workspace.dependencies]):
Table
Crate   Role
axum = "0.8"   HTTP framework; Router, extractors, middleware.
tokio = { version = "1", features = ["rt-multi-thread","macros","signal","time"] }   Async runtime; server only.
utoipa = "5" + utoipa-swagger-ui = { version = "9", features = ["axum"] }   OpenAPI 3.1 generation from #[utoipa::path] + ToSchema derives; Swagger UI at /docs.
serde, serde_json   Wire + snapshot JSON.
schemars = "0.8"   JSON Schema for Part A registry files (validation of user-supplied catalogs).
thiserror = "2"   Library error types; anyhow allowed only in main()/CLI.
tracing, tracing-subscriber = { features = ["json","env-filter"] }   Structured logs + spans.
metrics, metrics-exporter-prometheus   Prometheus metrics at /v1/system/metrics.
time = "0.3" (with serde, formatting)   All timestamps. (chrono acceptable but time preferred; pick one, workspace-wide.)
rand_chacha = "0.3", rand = "0.8"   Deterministic RNG streams.
rayon = "1"   Parallel per-tick home stepping in batsim-core.
redb = "2"   Recommended optional persistence (snapshots, audit log). Single-file embedded KV, ACID, zero config, pure Rust. Preferred over sqlx (no need for relational queries; a Postgres dependency would violate the single-binary goal) and over sled (unmaintained-ish, beta status; redb has stable ACID semantics and predictable write latency). Persistence is behind --features persistence and off by default.
tokio-stream = { features = ["sync"] }   BroadcastStream → SSE event source.
axum-extra = { features = ["typed-header"] }   Optional WebSocket upgrade path.
clap = { version = "4", features = ["derive"] }   CLI + server flags.
figment = { features = ["toml","env"] }   Layered config.
proptest, insta (dev)   Property tests, golden/snapshot tests.
schemathesis (CI, external tool)   Contract fuzzing against /openapi.json.
C.3 API Surface
Base: all routes under /v1. JSON everywhere (application/json; charset=utf-8). Every request/response type is a #[derive(ToSchema)] struct registered in the utoipa OpenApi tree — if it is not in /openapi.json, it does not ship.
C.3.1 Common conventions
Identifiers. Server-assigned, prefixed ULIDs: home_01J…, flt_01J…, scn_01J…, cmd_01J…, snap_01J…. ULIDs sort by creation time, which makes audit logs and pagination trivial.
Timestamps. RFC 3339 / ISO 8601 UTC strings (2025-03-14T18:30:00Z). Simulation time fields are named sim_* (e.g. sim_time) to distinguish them from wall-clock created_at.
Pagination (all list endpoints): cursor-based.
plain
GET /v1/homes?limit=100&cursor=eyJ…&fleet_id=flt_01J…
200 OK
{
  "data": [ { …home… } ],
  "page": { "next_cursor": "eyJsYXN0IjoiaG9tZV8wMUo…", "has_more": true }
}
limit ∈ [1, 1000], default 100. Cursors are opaque, base64url-encoded (sort_key, id); stable under insertion.
Filtering. Query params only, allow-listed per endpoint (never free-form). Multi-value = repeated param (?mode=backup-only&mode=grid-services). Unknown params → 400.
Error model — RFC 9457 application/problem+json:
JSON
{
  "type": "https://batsim.dev/problems/dispatch-window-violation",
  "title": "Dispatch window violation",
  "status": 422,
  "detail": "home_01J… cannot discharge 5.0 kW: available 3.2 kW at SOC 24%",
  "instance": "/v1/dispatch",
  "code": "DISPATCH_WINDOW_VIOLATION",
  "trace_id": "4b9f1c…",
  "invalid_targets": [{ "id": "home_01J…", "reason": "INSUFFICIENT_POWER", "available_kw": 3.2 }]
}
Fixed problem-type registry: validation-error (400), unauthorized (401), not-found (404), conflict (409, incl. idempotency/state conflicts), unprocessable (422, physics/rule violations), sim-not-running / sim-running (409 on illegal time-control transitions), internal (500). code is a SCREAMING_SNAKE enum stable across releases.
Idempotency (dispatch + all POSTs that mutate simulation state). Client sends Idempotency-Key: <uuid> header. Server stores (key, request_hash) → response in the audit store (TTL configurable, default 24h). Replay with same key + same body → original response, header Idempotent-Replay: true. Same key + different body → 409 idempotency-key-reuse. Additionally every dispatch command carries a client-supplied command_id (ULID/uuid); duplicates by command_id are deduplicated before enqueue, so a retried command can never execute twice on a device.
C.3.2 /v1/registry/* — device catalog (Part A)
Table
Method   Path   Description
GET   /v1/registry/batteries   List battery models. Filters: vendor, min_capacity_kwh, max_capacity_kwh, chemistry.
GET   /v1/registry/batteries/{model_id}   One battery model, full Part A schema.
GET   /v1/registry/inverters   List inverter models. Filters: vendor, min_power_kw.
GET   /v1/registry/inverters/{model_id}   One inverter model.
GET   /v1/registry/version   Catalog version/hash used in determinism guarantees.
GET /v1/registry/batteries/tesla-powerwall-3 →
JSON
{
  "model_id": "tesla-powerwall-3",
  "vendor": "tesla",
  "nominal_capacity_kwh": 13.5,
  "usable_capacity_kwh": 13.5,
  "max_charge_kw": 5.0,
  "max_discharge_kw": 11.5,
  "round_trip_efficiency": 0.90,
  "chemistry": "LFP",
  "soc_window": { "min": 0.0, "max": 1.0 },
  "degradation": { "cycle_fade_per_cycle": 0.00005, "calendar_fade_per_year": 0.01 }
}
C.3.3 /v1/homes — simulated home CRUD
Table
Method   Path   Description
POST   /v1/homes   Create home (idempotent).
GET   /v1/homes   List. Filters: fleet_id, mode, load_zone, battery_model.
GET   /v1/homes/{id}   Get full home state (config + current sim state + SOC).
PATCH   /v1/homes/{id}   Update mutable config (mode, reserve SOC). Returns 409 sim-running unless sim is paused — config changes are applied at tick boundary.
DELETE   /v1/homes/{id}   Remove home. 409 if sim running.
POST /v1/homes:
JSON
{
  "fleet_id": "flt_01J…",
  "battery": { "model_id": "tesla-powerwall-3", "count": 2 },
  "inverter": { "model_id": "tesla-pw3-inverter" },
  "pv": { "peak_kw": 8.4, "azimuth_deg": 178, "tilt_deg": 27 },
  "load": { "archetype": "sfh_family_ev", "annual_kwh": 14200 },
  "location": { "ercot_load_zone": "LZ_NORTH", "climate_zone": "3A" },
  "initial_soc": 0.5
}
→ 201 Created, Location: /v1/homes/home_01J…:
JSON
{
  "id": "home_01J…",
  "config": { …echo of request, validated & defaulted… },
  "state": { "soc": 0.5, "mode": "self-consumption", "battery_power_kw": 0.0, "pv_power_kw": 0.0, "load_power_kw": 0.0, "grid_power_kw": 0.0 },
  "created_at": "2025-03-14T18:30:00Z"
}
C.3.4 /v1/fleets — bulk composition
Table
Method   Path   Description
POST   /v1/fleets   Create fleet from manifest; expands to N homes deterministically.
GET   /v1/fleets / /v1/fleets/{id}   List / get (includes expansion stats).
POST   /v1/fleets/{id}:expand   Add more homes from a delta manifest.
POST   /v1/fleets/{id}:dispatch   Convenience: dispatch to entire fleet (alias for /v1/dispatch with target.fleet_id).
DELETE   /v1/fleets/{id}   Delete fleet and its homes (409 if sim running).
Manifest:
JSON
{
  "name": "north-texas-10k",
  "seed": 42,
  "archetypes": [
    { "weight": 0.55, "template": { "battery": { "model_id": "tesla-powerwall-3", "count": 1 },
        "pv": { "peak_kw": { "uniform": [5.0, 12.0] } }, "load": { "archetype": "sfh_family" } } },
    { "weight": 0.30, "template": { "battery": { "model_id": "enphase-iq-5p", "count": 2 },
        "pv": { "peak_kw": { "uniform": [6.0, 15.0] } }, "load": { "archetype": "sfh_empty_nester" } } },
    { "weight": 0.15, "template": { "battery": { "model_id": "sonnen-eco-10", "count": 1 },
        "pv": { "peak_kw": { "uniform": [4.0, 9.0] } }, "load": { "archetype": "townhome" } } }
  ],
  "geo": { "ercot_load_zones": { "LZ_NORTH": 0.6, "LZ_HOUSTON": 0.25, "LZ_WEST": 0.15 } },
  "count": 10000
}
Expansion is a pure function of (manifest, seed): archetype assignment uses a seeded weighted sampler; per-home continuous params draw from the home's own RNG stream (keyed by home index), so re-applying the same manifest yields identical homes — the response includes "expansion_hash": "sha256:…" for verification.
C.3.5 /v1/scenarios — bind a simulation run
A scenario binds time + data sources + seed to the current fleet. Exactly one scenario may be active at a time.
Table
Method   Path   Description
POST   /v1/scenarios   Create scenario.
GET   /v1/scenarios / /v1/scenarios/{id}   List / get.
POST   /v1/scenarios/{id}:activate   Load into engine (requires sim stopped). Validates replay data availability, preloads ERCOT series.
POST   /v1/scenarios/{id}:deactivate   Unload.
JSON
{
  "name": "feb-2021-uri-replay",
  "time": { "start": "2021-02-14T00:00:00Z", "end": "2021-02-20T00:00:00Z", "tick_seconds": 1 },
  "prices": { "source": "replay", "replay": { "date_range": ["2021-02-14", "2021-02-19"], "market": "RTM", "settlement_point": "HB_NORTH" } },
  "ancillary": { "programs": ["RRS", "ECRS"], "dispatch_model": "part_d_default" },
  "outages": [{ "start": "2021-02-15T07:00:00Z", "end": "2021-02-17T12:00:00Z", "scope": { "load_zones": ["LZ_NORTH"] }, "probability": 0.4 }],
  "weather": { "source": "replay", "dataset": "nsrdb", "years": [2021] },
  "seed": 1337
}
prices.source may also be "synthetic" with a shape spec ({ "profile": "summer_peak", "volatility": 0.3, "seed": 7 }) implemented by batsim-ercot per Part D.
C.3.6 /v1/sim — virtual time control
State machine: stopped → running ⇄ paused → stopped. All transitions are explicit endpoints (no implicit start on scenario activate).
Table
Method   Path   Description
POST   /v1/sim:start   Begin ticking at configured speed.
POST   /v1/sim:pause   Freeze at current tick boundary (drains command queue first).
POST   /v1/sim:resume   Resume.
POST   /v1/sim:stop   Halt and return to stopped (state retained for snapshot).
POST   /v1/sim:step   Advance N ticks synchronously while paused: { "ticks": 3600 } → { "sim_time": "…", "ticks_executed": 3600, "wall_ms": 412 }. Body ≤ 86 400 ticks (one sim-day) unless ?allow_large=true.
POST   /v1/sim:run-until   { "until": "2021-02-16T00:00:00Z" } — synchronous; returns when reached.
PUT   /v1/sim:speed   { "multiplier": 10.0 } — sim-seconds per wall-second; 0 = "as fast as possible" (bounded by CPU).
GET   /v1/sim:status   { "state": "running", "sim_time": "…", "tick": 51234, "speed": 10.0, "achieved_speed": 9.98, "lag_ticks": 3, "queued_commands": 12 }.
POST   /v1/sim:snapshots   Capture full state → snap_01J….
GET   /v1/sim:snapshots / /v1/sim:snapshots/{id}   List / download (binary; see C.5.4).
POST   /v1/sim:snapshots/{id}:restore   Restore. Requires sim stopped or paused; replaces world state atomically. Response includes snapshot_hash so a caller can verify restore fidelity.
C.3.7 /v1/dispatch — control commands
Table
Method   Path   Description
POST   /v1/dispatch   Send command to a device/home/fleet subset. Idempotent.
GET   /v1/dispatch/commands   Audit log. Filters: target, status, since, command_type.
GET   /v1/dispatch/commands/{command_id}   Command status + per-target execution detail.
DELETE   /v1/dispatch/commands/{command_id}   Cancel commands still in queued state.
Request:
JSON
{
  "command_id": "cmd_client_supplied_01J…",
  "target": { "fleet_id": "flt_01J…", "filter": { "mode": ["grid-services"], "soc_gt": 0.4 }, "sample_pct": 50 },
  "action": { "type": "discharge_to", "kw": 5.0, "duration_s": 3600 },
  "execution": { "latency_ms": { "uniform": [300, 2500] }, "timeout_s": 30, "ramp": "immediate" }
}
Actions: charge_to {kw}, discharge_to {kw}, set_reserve_soc {soc}, set_mode {mode: self-consumption|backup-only|time-of-use|grid-services}, curtail_pv {pct}, clear_override. Action enums are closed (OpenAPI oneOf with discriminator type).
Fan-out semantics (mimics real cloud APIs): the server expands the target set, assigns each home an execution delay drawn from execution.latency_ms using the home's RNG stream (deterministic per (command_id, home_id)), and enqueues (execute_at_tick, action) into the per-home command queue. Commands are applied at tick boundaries only (C.5.2). A home that cannot fully comply applies the physics-limited result and reports it. Response:
JSON
{
  "command_id": "cmd_client_supplied_01J…",
  "accepted": true,
  "targets": 5000,
  "status": "queued",
  "status_url": "/v1/dispatch/commands/cmd_client_supplied_01J…"
}
Per-target execution detail (queryable after completion): { "home_id": "…", "status": "applied|partial|rejected|timeout", "requested_kw": 5.0, "applied_kw": 3.2, "executed_at_sim_time": "…", "latency_ms": 1187 }. Aggregate status rollup: queued → in_flight → completed | completed_with_errors.
Audit log: append-only, retained per config (audit.retention_days, default 30); every entry has request hash, idempotency key, requesting principal, and full target expansion hash.
C.3.8 /v1/telemetry — history + streaming
Table
Method   Path   Description
GET   /v1/telemetry/homes/{id}/series   Historical timeseries for one home.
GET   /v1/telemetry/fleets/{id}/series   Aggregated fleet timeseries.
GET   /v1/telemetry/stream   SSE live stream.
GET   /v1/telemetry/ws   WebSocket equivalent (same message schema; negotiated via subprotocol batsim.v1+json).
Series query params: fields=battery_power_kw,soc,grid_power_kw,pv_power_kw,load_power_kw,price_rtm (allow-listed), from, to (sim time), resolution=1s|1m|5m|15m|1h, agg=sum|mean|p95 (fleet only, per-bucket). Response is columnar for compactness:
JSON
{
  "home_id": "home_01J…",
  "resolution": "1m",
  "fields": ["soc", "battery_power_kw"],
  "t": ["2021-02-15T07:00:00Z", "2021-02-15T07:01:00Z"],
  "v": [[0.52, -4.98], [0.515, -4.97]]
}
Downsampling: server-side bucket aggregation (mean for power, last for SOC); requesting finer resolution than the retention tier → 422 with detail naming the finest available tier. Retention tiers (config): raw tick data 24 sim-hours, 1m rollups 90 sim-days, 1h rollups forever (bounded ring buffer in memory; persisted when persistence enabled).
SSE stream:
plain
GET /v1/telemetry/stream?fleet_id=flt_01J…&fields=aggregate&downsample=1s
Accept: text/event-stream

event: tick
id: 51234
data: {"sim_time":"2021-02-15T07:00:01Z","tick":51234,"fleet":{"battery_power_kw":-4120.5,"pv_power_kw":0.0,"load_power_kw":9802.2,"grid_power_kw":13922.7,"soc_mean":0.48},"price_rtm":9001.11}

event: dispatch
id: 51236
data: {"command_id":"cmd_01J…","targets_applied":4988,"targets_rejected":12}
fields=aggregate returns fleet rollup; fields=raw (small fleets only, ≤ 500 homes, else 422) streams per-home vectors. Filters: fleet_id, home_ids (repeated, ≤ 500), fields, downsample. Backpressure: each SSE consumer has a bounded channel (default 1024 events); on overflow the server emits event: gap with { "missed_ticks": [a, b] } and continues — consumers must treat the stream as lossy-under-overload and reconcile via the series endpoint. Last-Event-ID resume is supported for tick events (id = tick number).
C.3.9 /v1/vendor-api/* — OEM cloud mimicry (optional mode)
Enabled per-run with --features vendor-mimicry and config vendor_api.enabled = true. When on, the server additionally mounts per-vendor path prefixes (outside /v1, matching each vendor's real URL shape) so that an unmodified OEM integration pointed at the simulator's base URL works against simulated homes. Each home exposes a vendor persona chosen from its battery model's vendor. Persona state is a pure projection of batsim-core home state — there is no second simulation.
Persona mapping (Part A vendor field → mounted API):
Table
Vendor   Mounted prefix   Imitates
tesla   /api/1/…   Tesla Fleet API (energy endpoints)
enphase   /api/v2/… (cloud-style) and /ivp/…, /api/v1/production (Envoy local)   Enphase Enlighten + Envoy local API
solaredge   /site/{id}/…   SolarEdge monitoring API
sonnen   /api/v2/…   sonnenBatterie local API v2
Because prefixes collide for enphase/sonnen (/api/v2), homes of different vendors get vendor-scoped base URLs: the server exposes virtual host routing (--vendor-hosts tesla.local,enphase.local,…) or, simpler, per-vendor listener ports (BATSIM_VENDOR_API__TESLA_PORT=18081, etc.). Port-per-vendor is the reference implementation — deterministic and trivially scriptable.
Auth mimicry: any bearer token is accepted; POST /oauth/token (Tesla shape) returns a fixed valid token. This keeps real integrations' auth flows working without secrets.
Tesla example. GET /api/1/products:
JSON
{
  "response": [
    {
      "energy_site_id": 8812345001,
      "resource_type": "battery",
      "site_name": "Home home_01JHQ…",
      "gateway_id": "GW-home_01JHQ…",
      "energy_left": 7020.0,
      "total_pack_energy": 13500,
      "percentage_charged": 52,
      "battery_type": "ac_powerwall",
      "battery_power": -4980,
      "grid_status": "Active",
      "backup_capable": true,
      "components": { "solar": true, "solar_type": "pv_panel", "battery": true, "grid": true, "backup": true, "gateway": "teg" }
    }
  ],
  "count": 1
}
GET /api/1/energy_sites/{site_id}/live_status:
JSON
{
  "response": {
    "solar_power": 3120,
    "energy_left": 7020.0,
    "total_pack_energy": 13500,
    "percentage_charged": 52,
    "battery_power": -4980,
    "load_power": 2460,
    "grid_status": "Active",
    "grid_services_active": true,
    "grid_power": 4320,
    "island_status": "on_grid",
    "timestamp": "2021-02-15T07:00:01Z"
  }
}
Field semantics: battery_power sign convention follows the vendor's (negative = charging, per Tesla docs); timestamp is sim time, not wall time — this is what makes replay scenarios work with unmodified integrations. Control endpoints are also mimicked and map onto the dispatch pipeline (so they appear in the audit log): POST /api/1/energy_sites/{id}/operation ({ "default_real_mode": "backup" } → set_mode backup-only), POST /api/1/energy_sites/{id}/backup (→ set_reserve_soc), POST /api/1/energy_sites/{id}/grid_import_export. Vendor error shapes are imitated too (Tesla returns { "error": "…", "error_description": "…" } with HTTP 4xx, not problem+json) — the mimicry layer translates from the canonical problem model.
Enphase Envoy local example: GET /ivp/livedata/status returns meters/pv/storage blocks derived from the same home state; sonnen: GET /api/v2/status with Consumption_W, Production_W, Pac_total_W, RSOC, USOC, GridFeedIn_W. SolarEdge: GET /site/{id}/currentPowerFlow with PV/LOAD/GRID/STORAGE node JSON. All are generated projections; spec tables for every field live in batsim-server/src/vendor_api/<vendor>.rs with one golden test each (C.7.1).
Non-goals: only read paths + the listed control paths are imitated; account/fleet management endpoints of real vendors are stubbed with static valid responses. Firmware update, pairing, and tariff-editor flows are out of scope and return vendor-shaped 501.
C.3.10 /v1/market/* — pass-through to Part D
Table
Method   Path   Description
GET   `/v1/market/prices?settlement_point=HB_NORTH&from=…&to=…&market=RTM   DAM`   5-min settlement price series (Part D).
GET   /v1/market/ancillary/programs   AS products active in scenario.
GET   `/v1/market/ancillary/awards?home_id   fleet_id=…`   AS obligations per home/fleet.
GET   /v1/market/dispatch-signals?fleet_id=…   Current/scheduled ERCOT-style dispatch instructions the fleet is following.
Handlers delegate to batsim-ercot; no market logic in the server. Responses use Part D's schemas verbatim (registered in the same OpenAPI doc).
C.3.11 /v1/system
Table
Method   Path   Description
GET   /v1/system/health   { "status": "ok", "sim_state": "running", "uptime_s": 812, "version": "0.4.2", "git_sha": "…" }. Kubernetes-style; 503 until registry+replay preload completes.
GET   /v1/system/metrics   Prometheus text exposition (text/plain; version=0.0.4).
GET   /v1/system/version   { "version", "git_sha", "build_time", "registry_version", "openapi_version": "3.1.0" }.
GET   /v1/system/config   Effective config, secrets redacted.
GET   /openapi.json   The OpenAPI 3.1 document (also GET /openapi.yaml).
GET   /docs   Swagger UI (utoipa-swagger-ui).
C.4 Auth & Tenancy
Default: single-tenant, no auth. Server binds 127.0.0.1:8080; every request is treated as principal local. This is the right default for a simulator on a developer/agent machine and keeps integration tests trivial.
Optional API key. auth.api_keys = ["key1", "key2"] in config or BATSIM_AUTH__API_KEYS=k1,k2. When non-empty, all /v1/* routes require Authorization: Bearer <key> or X-Api-Key: <key>; failures get problem type unauthorized. Keys are compared in constant time (subtle crate); only SHA-256 fingerprints appear in logs/config dumps.
Principals & audit. The API key's configured name (or local) is stamped into the dispatch audit log. There is no user management, no RBAC beyond an optional auth.read_only_keys list (may only call GET + /v1/telemetry/stream).
Vendor mimicry endpoints accept any token (C.3.9) regardless of API-key config, since their purpose is receiving unmodified third-party integrations. Binding them to separate ports keeps this exposure explicit.
mTLS: out of scope. If needed, terminate TLS at a reverse proxy (Caddy/nginx) in front; the server speaks plain HTTP/1.1+HTTP/2 cleartext only.
Rate limiting: intentionally absent. Local-first tool; a local client can already DoS via /v1/sim:step. A hook exists (tower::limit::RateLimitLayer, behind auth.rate_limit = { rps = … }) for shared deployments but is off by default and not a deliverable.
CORS: permissive (*) by default for local browser tooling; tighten via server.cors_origins.
C.5 Concurrency & State Model
C.5.1 Sharding
The world is sharded into R regions (R = available_parallelism(), min 4), each Region { homes: Vec<Home>, queue: CommandQueue } living behind one tokio::sync::Mutex. Homes are assigned by hash(home_id) % R at creation and never migrate (so RNG streams and telemetry buffers stay put). A single engine task (one dedicated OS thread via tokio::task::spawn_blocking, or a plain std::thread) owns the tick schedule and drives regions through rayon scoped threads for the parallel part of each tick.
Rationale over actor-per-home: 10k homes × mailbox actors adds scheduler noise and destroys cache locality; sharded-Vec + rayon gives deterministic order and sub-millisecond ticks. The Mutex is held only for (a) the parallel step (uncontended — only the engine ever takes it during a tick) and (b) brief API reads (GET /v1/homes/{id} takes a read snapshot of one home via tokio::task::block_in_place-free try_lock with retry on next tick boundary).
C.5.2 Tick interleaving — determinism contract
Each tick executes this fixed sequence (matching Part B's tick semantics):
Drain command queues. For each region, in home-index order, dequeue all commands with execute_at_tick <= now, apply mode/setpoint changes. (Commands enqueued by HTTP between ticks always carry execute_at_tick >= current_tick + ceil(latency / tick_len), so HTTP can never mutate mid-tick.)
Inputs. Read ERCOT price/AS/dispatch values for the tick (preloaded arrays, indexed by tick number — no RNG, no I/O), weather, outage schedule.
Step homes. rayon par-iter over regions/homes: pure physics update per Part B. Homes may not read each other (grid-level coupling, if any, is computed from the previous tick's aggregate — explicitly documented in Part B).
Aggregate. Fleet rollups, telemetry ring-buffer append, SSE broadcast (non-blocking try_send; lagging consumers get gap events).
Bookkeeping. Metrics, audit-log flush for commands applied this tick, snapshot-on-schedule if configured.
Because steps 1–3 are a pure function of (state, inputs, tick index) and the HTTP layer can only append to queues, the API can perturb a run only through recorded commands — replaying the audit log from a snapshot reproduces the run exactly.
C.5.3 Ordering guarantees
Commands to the same home execute in execute_at_tick, then enqueue order.
A POST /v1/dispatch response returning accepted happens-before the command's application (queue ack, not device ack — device ack is the per-target execution record).
SSE tick events for tick N are emitted after all tick-N state is committed; a client reading /v1/homes/{id} after receiving tick N sees state ≥ N.
Snapshot capture is a stop-the-world pause (engine quiesces at a tick boundary, serializes, resumes) — snapshots are always tick-aligned, never torn.
C.5.4 Snapshot format & persistence
Two representations, same logical content (SimSnapshot { scenario, tick, sim_time, homes: Vec<HomeState>, rng_states, telemetry_cursors, pending_commands }):
Binary (recommended, default): bincode 2 (with serde compat), zstd-compressed (zstd level 3), file suffix .batsim.snap. ~10–20× smaller and faster than JSON. Header: magic BATSNAP1, format version, sha256 of payload (this hash is the snapshot_hash used by the determinism test).
JSON (interchange/debug): serde_json, .json suffix. Byte-stable via sorted-map serialization; used when a human/agent must inspect or hand-edit state.
Without the persistence feature, snapshots live in an in-memory store (LRU, default 8 snapshots) and can be downloaded via the API. With persistence, snapshots + audit log + command idempotency records go into a single redb file (data/batsim.redb); redb's single-writer model fits the engine's single-writer reality, and its copy-on-write commit means a crash mid-snapshot never corrupts a previous one.
C.5.5 Memory budget
Per home (measured targets, asserted in a #[test] with std::mem::size_of + allocation counting in benches):
Table
Component   Bytes (approx.)
Config (ids, enum refs into registry, PV/load params)   400
Physics state (SOC, temps, degradation counters, mode, setpoints)   200
RNG stream state (ChaCha8)   96
Telemetry ring buffer (24h × 1s × 6 f32 channels)   ~2.1 MB if per-home raw retention kept in memory
Raw per-home tick retention dominates; therefore raw ticks are kept per home only for the last hour (3600 × 6 × 4 B ≈ 86 KB) and older data lives as fleet-level rollups + per-home 1m rollups (90 days × 1440/day × 24 B ≈ 3 MB/day-of-fleet… bounded by ring caps). Budget: ≤ 100 KB/home steady-state, i.e. ≤ ~1.1 GB for a 10 000-home fleet including overhead — comfortably within a 2 GB container. A GET /v1/system/stats endpoint exposes live RSS/heap accounting so regressions are observable.
C.6 Observability
Tracing (tracing + tracing-subscriber, JSON on stdout). Span hierarchy: scenario (fields: scenario_id, seed) → tick (tick, sim_time, homes) → region_step (region, n_homes, elapsed_us). HTTP: tower_http::trace::TraceLayer with request_id (ULID, echoed as X-Request-Id, also in problem JSON trace_id). Levels: INFO request summaries + scenario lifecycle; DEBUG per-tick (off by default); ERROR with full problem detail. RUST_LOG/config logging.filter uses EnvFilter syntax.
Prometheus metrics (/v1/system/metrics):
plain
batsim_homes_total{fleet}                       gauge
batsim_ticks_total{scenario}                    counter
batsim_tick_duration_seconds                    histogram (buckets 1e-6..1)
batsim_achieved_speed_ratio                     gauge   (achieved vs configured multiplier)
batsim_tick_lag_ticks                           gauge   (schedule drift)
batsim_commands_applied_total{action,result}    counter
batsim_commands_latency_ms                      histogram (per-device execution latency)
batsim_sse_subscribers                          gauge
batsim_sse_dropped_events_total                 counter
batsim_http_requests_total{route,status}        counter
batsim_memory_bytes                             gauge
Alert-relevant invariant: batsim_tick_lag_ticks must return to 0 after bursts; persistent >0 at multiplier ≤ target means the machine can't sustain the requested speed.
Structured logs: every dispatch command emits one INFO JSON line at acceptance and one at completion rollup (fields: command_id, targets, applied, rejected, elapsed_sim_s). No PII exists in the system; logs may include full request bodies only at DEBUG.
C.7 Testing Strategy
C.7.1 Golden-file physics tests (per device model)
For every model in the Part A registry (Tesla PW3, Enphase IQ 5P, SolarEdge Home Battery, Sonnen eco 10, …): a fixed 48-hour scripted scenario (defined PV curve, load curve, ambient temp, two dispatch commands) is stepped in a plain #[test] against batsim-core. Expected SOC/power traces live in tests/golden/<model>.snap.json (insta). Assertion: per-tick SOC within 1e-4 absolute of golden, cumulative energy within 1e-6. Goldens are regenerated only via INSTA_UPDATE=always cargo test -p batsim-core --test golden + PR review; CI runs --check.
Each vendor-API mimicry module also has one golden test: fixed home state → exact expected vendor JSON (field order irrelevant, compared structurally).
C.7.2 Property tests (proptest)
Energy conservation: for all random device models × random power sequences, |stored_energy_delta - (charge_in*η_c - discharge_out/η_d)| ≤ ε, and cumulative round-trip efficiency stays within the model's RTE bounds ± tolerance.
SOC window: SOC ∈ [soc_window.min, soc_window.max] for all inputs, including adversarial (charge while full, discharge while empty, 100% PV curtailment toggling).
Setpoint clamping: |battery_power| ≤ min(model limit × count, inverter limit) always.
Command idempotency: applying the same command twice (same command_id) equals applying once.
Serialization round-trip: snapshot → bincode → restore is the identity on state hash.
C.7.3 API contract tests
Schemathesis (CI job): schemathesis run http://localhost:8080/openapi.json --checks all --stateful=links --hypothesis-max-examples=500 against a seeded server with a 200-home fixture fleet. Any 5xx or schema-nonconformant response fails the build.
Handwritten route tests (axum::Router::oneshot) for every endpoint's happy path + documented error paths (each problem type from C.3.1 exercised at least once).
Spec freshness check: CI regenerates api/openapi.json from the binary (batsim --dump-openapi > api/openapi.json) and git diff --exit-code — the committed spec can never drift from the code. batsim-client compilation against the fresh spec doubles as a codegen smoke test.
C.7.4 Determinism test
tests/determinism.rs: build 1000-home fleet from fixed manifest, activate fixed replay scenario, apply a recorded command script at fixed ticks, run 86 400 ticks (1 sim-day), snapshot, sha256. Run the whole thing twice in the same process and once more in a fresh process: all three snapshot_hash values must be equal. Also run at speed multiplier 10× vs step-driven: hashes must match (proves wall-clock scheduling does not leak into state).
C.7.5 Load/performance test
tests/load.rs (nightly, #[ignore] in PR CI): 10 000 homes, 1-second ticks, scenario length 24 sim-hours, dispatch bursts of 5000-target commands every 10 sim-minutes. Target: sustained ≥ 10× realtime (one sim-day in ≤ 2.4 wall-hours) on a 4-core CI runner, with tick_duration p99 < 100 ms, lag_ticks == 0 at 10×, RSS < 2 GB, and SSE streaming to 10 concurrent subscribers with zero gap events. Results are uploaded as CI artifacts with a trend comment.
C.8 Build / Run / DevEx
C.8.1 Cargo commands
bash
cargo build --workspace                     # build all
cargo run -p batsim-server -- --config config/batsim.toml
cargo run -p batsim-server -- --dump-openapi > api/openapi.json
cargo test --workspace                      # unit + integration (excluding #[ignore]d load test)
cargo test -p batsim-core --test golden     # physics goldens
cargo test --workspace -- --ignored         # load/perf (nightly)
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo doc --workspace --no-deps --open
Server flags (all also config/env): --config <path>, --port, --data-dir, --registry-dir, --seed, --print-config, --dump-openapi, --features vendor-mimicry,persistence at build time.
Quick start:
bash
batsim --config config/batsim.toml &
curl -s localhost:8080/v1/system/health
curl -s -X POST localhost:8080/v1/fleets -H 'content-type: application/json' -d @examples/fleet-10k.json
curl -s -X POST localhost:8080/v1/scenarios -d @examples/uri-replay.json
curl -s -X POST localhost:8080/v1/scenarios/scn_01J…:activate
curl -s -X POST localhost:8080/v1/sim:start
curl -N 'localhost:8080/v1/telemetry/stream?fleet_id=flt_01J…&fields=aggregate'
C.8.2 Dockerfile (deploy/Dockerfile, multi-stage → distroless)
dockerfile
# ---- build ----
FROM rust:1.83-bookworm AS build
WORKDIR /src
RUN apt-get update && apt-get install -y clang mold && rm -rf /var/lib/apt/lists/*
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./
COPY crates crates
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release -p batsim-server --features vendor-mimicry,persistence \
 && cp target/release/batsim /batsim

# ---- runtime ----
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /batsim /usr/local/bin/batsim
COPY config/batsim.toml /etc/batsim/batsim.toml
EXPOSE 8080
VOLUME ["/data"]
USER nonroot
ENTRYPOINT ["batsim", "--config", "/etc/batsim/batsim.toml", "--data-dir", "/data"]
(debian-slim is the accepted fallback if a debugger/shell in-image is needed: swap the runtime stage for debian:bookworm-slim.)
C.8.3 docker-compose (sim + generated-client demo)
yaml
services:
  batsim:
    build: { context: .., dockerfile: deploy/Dockerfile }
    ports: ["8080:8080", "18081:18081"]   # API + Tesla mimicry
    environment: { BATSIM_LOGGING__FILTER: "info,batsim=debug" }
    volumes: ["batsim-data:/data"]
    healthcheck: { test: ["CMD", "/usr/local/bin/batsim", "--health-check"], interval: 5s }
  client-demo:
    build: { context: ../examples/client-demo }   # python:3.12-slim + openapi-generator output
    depends_on: { batsim: { condition: service_healthy } }
    command: ["python", "demo.py"]   # generates client from http://batsim:8080/openapi.json, then runs a scenario
volumes: { batsim-data: {} }
C.8.4 just tasks (justfile; make acceptable alternative)
plain
just check        # fmt --check + clippy -D warnings + cargo check --workspace
just test         # cargo test --workspace
just golden       # INSTA_UPDATE=always cargo test -p batsim-core --test golden
just spec         # regenerate api/openapi.json, diff against HEAD
just contract     # run server fixture + schemathesis
just load         # cargo test --release --test load -- --ignored --nocapture
just docker       # docker build -f deploy/Dockerfile -t batsim:dev .
just compose      # docker compose -f deploy/docker-compose.yml up --build
just ci           # check + test + spec + contract
C.8.5 CI matrix (GitHub Actions)
Table
Job   Matrix   Steps
lint   stable   fmt, clippy -D warnings, cargo doc (warnings deny)
test   {ubuntu-latest × stable 1.83.0} × {default, --all-features}   workspace tests
test-canary   ubuntu × {stable, beta}   non-blocking
spec   stable   --dump-openapi diff + build batsim-client + schemathesis
determinism   stable   cargo test --test determinism --release
docker   —   build image, run container, hit /v1/system/health, /openapi.json
load (nightly)   4-core runner   load test, publish trend
audit (weekly)   —   cargo audit, cargo deny check
C.8.6 How an AI agent should iterate
Make the change in the narrowest crate (physics → batsim-core; endpoint → batsim-server; catalog → batsim-registry JSON).
cargo check -p <crate> until clean; then cargo clippy -p <crate> --all-targets -- -D warnings.
cargo test -p <crate>; add/adjust goldens only via just golden and review the diff.
If the API changed: just spec and confirm the diff to api/openapi.json is intentional; fix batsim-client if codegen breaks.
just ci before considering the task done. Never merge with a failing determinism or spec-freshness check — those two jobs are the system's core promises (reproducibility, spec-as-truth).
C.9 Non-goals (explicit)
Multi-tenant SaaS concerns: orgs, billing, per-tenant quotas.
mTLS / TLS termination (proxy's job).
A stateful relational model; all queries are pre-defined endpoints, not ad-hoc SQL.
Browser UI beyond Swagger UI (a dashboard may consume the API later, but is not this spec).
Vendor endpoints beyond the read + control surface enumerated in C.3.9.
Part D — ERCOT Market Integration
Status: Normative engineering specification.
Crate: batsim-ercot (per Part C, §Architecture). All market-facing logic specified here lives in batsim-ercot; device physics lives in Part B (batsim-engine); device registry in Part A; HTTP/streaming API surface in Part C (batsim-api).
Scope rule (hard): This simulator integrates ERCOT only. No CAISO, PJM, MISO, NYISO, ISO-NE, SPP, or any other market model may be introduced, abstracted-for, or stubbed "for future use." Where other Parts describe market-agnostic seams, this Part fills exactly one plug: ERCOT.
Cross-references used below:
Part A — OEM device registry: per-device power/energy/SOC limits, chemistry, warranty cycle limits.
Part B — physics/time engine: 1-second device ticks, interval integration hooks, clock control (real-time / accelerated / replay).
Part C — API & Rust architecture: crate layout, /v1/dispatch endpoint, telemetry streams, scenario API that binds a PriceSource to a simulation run.
D.1 ERCOT Market Model Scope (as of 2025–2026)
ERCOT is an energy-only, nodal market. There is no capacity market; resource adequacy revenue is delivered almost entirely through scarcity pricing and ancillary services. This is the single most important fact for the simulator's economics: a residential battery fleet in ERCOT earns money by being available and responsive during a small number of very high-priced hours, plus a steadier ancillary-services layer, plus retailer-side 4CP savings. The simulator MUST model all three.
Verifiability note: all ERCOT market facts in this section are stated as of the 2025–2026 protocol environment. ERCOT protocols change via NPRR (Nodal Protocol Revision Request) process; the implementer MUST verify every load-bearing number (caps, response times, product names, report IDs) against the current ERCOT Nodal Protocols and MIS documentation at build time. See §D.8.
D.1.1 Real-Time Market (RTM)
Dispatch: ERCOT runs Security-Constrained Economic Dispatch (SCED) every 5 minutes, producing Locational Marginal Prices (LMPs) at resource nodes and electrical buses.
Settlement: Settlement Point Prices (SPPs) are published at three location granularities: resource nodes, load zones, and trading hubs (e.g., Hub North, Hub West, HB_NORTH, LZ_HOUSTON, etc.). Residential load settles against its load zone SPP. A behind-the-meter residential fleet, when not registered as a market resource, is economically exposed to its load-zone SPP (as avoided cost / retail pass-through); when registered (e.g., as an aggregate resource, §D.2), it may be settled at a defined settlement point. The simulator MUST support location as a typed enum: Location::Hub(TradingHub) | Location::LoadZone(LoadZone) | Location::Node(String). ERCOT has four competitive load zones: LZ_WEST, LZ_NORTH, LZ_SOUTH, LZ_HOUSTON (verify names against current MIS settlement-point list).
Settlement interval cadence: Historically RTM SPPs were settled on 15-minute intervals even though SCED runs every 5 minutes. ERCOT's RTC+B (Real-Time Co-Optimization plus Batteries) program went live December 5, 2025, which changes real-time market structure (co-optimization of energy and AS in real time, battery state-of-charge modeling) and associated data publications; several legacy MIS reports were discontinued or replaced at that cutover (see §D.3, §D.8). Uncertainty flag: whether post-RTC+B settlement is at 5-minute granularity must be verified against the current protocols. The simulator's settlement engine MUST treat the settlement interval length as configuration (settlement_interval_secs, default 900, allow 300), not a constant.
Scarcity pricing / ORDC: ERCOT applies an Operating Reserve Demand Curve (ORDC) price adder system-wide when operating reserves fall below a threshold (nominally ~7,000 MW; the adder rises non-linearly as reserves decline toward ~3,000 MW and below, where prices are driven toward the system-wide cap). A Reliability Deployment Price Adder (RDPA) also applies when ERCOT takes out-of-market reliability actions. Under normal conditions both adders are $0. This mechanism is the economic heart of the simulator: a handful of scarcity hours per year — where SPPs jump from ~$30/MWh to $1,000–$5,000/MWh within a few SCED intervals — can dominate annual fleet revenue. The simulator MUST ingest or synthesize adder timeseries separately from the base LMP, because (a) adders are published separately in ERCOT data products, and (b) the synthetic generator (§D.4) composes base price + adder regimes.
Offer caps (post-Winter-Storm-Uri): The PUCT lowered the High System-Wide Offer Cap (HCAP) from $9,000/MWh to $5,000/MWh, effective January 1, 2022 (approved December 2021). The Low System-Wide Offer Cap (LCAP) is $2,000/MWh, applied under the Emergency Pricing Program after sustained time at the high cap (reported as: after 12 hours at HCAP within a rolling 24-hour period — verify exact trigger language in the current protocols). Historical replays of February 2021 (Winter Storm Uri) therefore show prices at the then-effective $9,000/MWh cap sustained for days; synthetic Uri-style scenarios must be parameterized by the cap in force for the scenario's simulated era (§D.4).
D.1.2 Day-Ahead Market (DAM)
Hourly DAM SPPs at nodes/zones/hubs, plus DAM ancillary-service clearing prices for capacity.
DAM is a financial (and, for AS, physical-award) market cleared once daily for the next operating day. The retailer's optimizer typically uses DAM results to build a day-ahead dispatch plan; the simulator MUST expose DAM SPPs and DAM AS clearing prices as first-class signals (§D.3, §D.6).
D.1.3 Ancillary Services (AS)
ERCOT's market-procured ancillary services, and their realistic accessibility to aggregated residential DER:
Table
Product   Response requirement   Duration requirement   Procured   Realistic for aggregated residential batteries?
Regulation Up / Reg Down   Sub-second AGC following, continuous   Continuous   DAM (+ RTM co-optimization post-RTC+B)   Typically out of reach. Requires AGC-grade telemetry and regulation qualification; not realistic for thousands of behind-meter homes. Model as not available in the simulator's AS module.
Responsive Reserve Service (RRS)   Fast response (seconds) to frequency events / deployment   Sustained per product spec   DAM   Accessible via aggregation for qualified load/storage resources (incl. controllable load resources). Historically the flagship residential-DR AS product. Model as available.
Non-Spinning Reserve (Non-Spin)   10-minute response   Up to 4-hour sustain requirement (verify)   DAM   Accessible in principle to aggregations of load/resources; long duration makes it marginal for 2–4 h residential batteries but model as available with duration-based derate.
ECRS (ERCOT Contingency Reserve Service)   10-minute response, sustained 2 consecutive hours; introduced June 10, 2023 — first new ERCOT AS in over a decade   2 h   DAM   Primary modern AS opportunity for storage aggregations. ECRS has been one of the most lucrative products for storage since launch (though clearing prices compressed sharply through 2024–2025 as BESS capacity saturated the market — the simulator must NOT hard-code 2023-era price levels). Model as available.
Notes for implementers:
Duration-based eligibility is real: e.g., reporting on ERCOT rules indicates a 1-hour battery can sell at most ~50% of rated power into ECRS while ≥2-hour resources can sell full rated power (verify current duration-derate rules). The simulator's AS module MUST apply a configurable duration derate: awardable_mw = min(fleet_headroom_mw, f(usable_energy_kwh / discharge_kw, product)).
AS awards are hourly MW quantities at hourly clearing prices ($/MW) from the DAM. Real-time AS co-optimization post-RTC+B changes real-time AS pricing mechanics — verify; the simulator MAY model AS revenue purely from DAM awards (recommended default) and treat any RT AS repricing as out of scope v1.
A further product, DRRS (Dispatchable Reliability Reserve Service), has been under ERCOT development (stemming from Texas HB 1500); track and note in §D.8 as "emerging — do not implement until rules are final."
D.1.4 4CP (Four Coincident Peak) Transmission Cost Allocation
ERCOT allocates a large share of transmission cost (via the 4CP methodology) to load-serving entities based on their coincident peak demand during the ERCOT system peak 15-minute interval in each of June, July, August, and September of the prior year.
Consequence: a retailer's transmission-cost exposure is driven by what its customers' meters draw during four specific 15-minute intervals per year. A residential battery fleet that discharges during candidate peak intervals directly reduces the retailer's 4CP tag — often worth tens of dollars per kW-year at the meter level (market-rate numbers vary by TDSP and year; treat as scenario parameter, not constant).
The simulator MUST provide: (a) a 4CP watch mode that flags intervals as 4CP candidates (system load within configurable % of season-to-date peak, June–Sept), (b) per-interval fleet net-load reduction vs. baseline, and (c) 4CP savings attribution in settlement output (§D.5). A candidate interval is confirmed retroactively when the season's actual peaks are known — the settlement engine MUST support retro-confirmation marking.
D.1.5 Retail context
The user persona is a retail electric provider (REP) with a residential battery fleet testing dispatch strategies. The simulator is not a bidding engine and not a QSE interface; it is a strategy evaluation harness: given price/signal streams and a dispatch strategy (external optimizer), it computes fleet physical response (Parts A/B), market exposure, and settlement P&L (§D.5). Wholesale market registration of the fleet (as ADER/aggregate, §D.2) is modeled only at the level needed to test the strategy: dispatch-in → physical response → telemetry-out → settlement.
D.2 ADER / Aggregation Semantics
ERCOT's Aggregate Distributed Energy Resource (ADER) pilot (project launched ~2022; verify current pilot status and any permanent ADER framework) allows distributed resources to participate in ERCOT markets as an aggregate resource via a Qualified Scheduling Entity (QSE), with:
Aggregate-level registration: the fleet appears to ERCOT as one resource with a defined settlement point / zone mapping and an aggregate capability (MW up/down, MWh).
Aggregate dispatch instructions: the aggregate receives MW setpoints (energy dispatch, AS deployment instructions) from ERCOT/SCED — the aggregate's operator (here, the retailer's fleet platform) is responsible for disaggregating that setpoint across thousands of homes and demonstrating aggregate delivery via telemetry.
Baseline methodologies: load-side participation requires a baseline (counterfactual load) to measure delivered MW. ERCOT pilot work has explored baseline approaches; the details are pilot-specific and evolving — do not hard-code. The simulator MUST implement baseline as a pluggable component with at least: LastNDaysAverage{n, exclusion_rules} and MeteredBeforeAfter (in-event vs. pre-event meter), and must record the chosen baseline method in settlement output so P&L is auditable against it.
Model-level contract in the simulator
The simulator represents the fleet to the outside world as an aggregate resource with a dispatch instruction channel. Concretely:
plain
                  ┌─────────────────────────── simulator ───────────────────────────┐
 external         │  batism-ercot                Part B engine          Part A      │
 optimizer  ───►  │  AggregateResource ──►  fleet dispatch disaggregation ──► homes │
 (strategy)  ◄──  │  (dispatch-in MW)  ◄──  interval telemetry (MW, SOC)  ◄──       │
                  └─────────────────────────────────────────────────────────────────┘
Dispatch instruction (in): aggregate MW setpoint with optional per-interval shape, or per-device schedule (Part C /v1/dispatch). batsim-ercot translates aggregate setpoints into device-level allocations respecting Part A limits and Part B physics; allocation strategy is pluggable (ProRata, SocWeighted, MarginalWarrantyCost — the last using Part A warranty cycle cost).
Baseline (state): batsim-ercot maintains a per-interval baseline net load for the fleet using the configured baseline methodology over simulated meter history.
Telemetry (out): interval-average aggregate delivered MW, baseline MW, delta (delivered), SOC distribution, unavailable-capacity MW. Delivered MW = baseline − metered, per convention of the configured baseline module.
AS representation: AS awards are inputs (from replay/synthetic AS price + award scenario, §D.3/§D.4); the simulator does NOT simulate ERCOT's AS clearing. When an AS deployment event occurs (per scenario script or replayed event log), the fleet receives an aggregate deployment instruction through the same channel.
Out of scope for v1: QSE webservice emulation, ERCOT outbound telemetry protocols (ICCP etc.), registration workflows. The dispatch channel is an internal API, not an ERCOT wire protocol.
D.3 Price & Signal Data Sources
D.3.1 ERCOT public data (MIS / data products)
ERCOT publishes market data through its MIS (Market Information System, mis.ercot.com) and the newer data-product portal (data.ercot.com / api.ercot.com). Formats are predominantly CSV (often zipped) and XML; some reports are near-real-time, many settlement-quality reports are published with delay (historically 48 hours for settlement-point-level detail, 60 days for offer-level disclosure reports). All report IDs, endpoints, columns, cadences, and delays below MUST be verified against current ERCOT documentation at build time (§D.8) — ERCOT changes these, and the RTC+B cutover (Dec 5, 2025) discontinued/replaced several reports.
Known/expected report products to ingest for replay (IDs as catalogued pre-RTC+B; verify):
Table
Signal   Data product (name / ID)   Cadence   Delay   Format
Real-time SPPs (nodes/zones/hubs)   "Settlement Point Prices at Resource Nodes, Hubs and Load Zones" — NP6-905-CD   15-min (SCED-derived)   near-RT   CSV/ZIP
DAM SPPs   "DAM Settlement Point Prices" — NP4-190-CD   hourly, daily   next day   CSV/ZIP
DAM AS clearing prices for capacity   "DAM Clearing Prices for Capacity" — NP4-188-CD   hourly, daily   next day   CSV/ZIP
ORDC / reliability adders & reserves by SCED interval   "Real-Time ORDC and Reliability Deployment Price Adders..." — NP6-323-CD (publication stopped ~Dec 5, 2025 — identify replacement)   5-min   near-RT   CSV/ZIP
Historical RTM zone/hub prices   "Historical RTM Load Zone and Hub Prices" — NP6-785-ER   hourly/15-min   48-h   CSV/ZIP
Historical DAM zone/hub prices   "Historical DAM Load Zone and Hub Prices" — NP4-180-ER   hourly   48-h   CSV/ZIP
System-wide actuals (load, fuel mix, etc.)   "System-Wide Actuals" — NP6-235-CD   5/15-min   near-RT   CSV/ZIP
Actual system load by forecast zone   NP6-346-CD   hourly   near-RT   CSV
Fuel mix   (fuel-mix report; verify ID — needed for §D.5 emissions)   15-min   near-RT   CSV
RTM / DAM price corrections   NP4-197-M / NP4-196-M   as needed   later   CSV
Also note: api.ercot.com provides a REST API for many of these products (the gridstatus project's endpoint list is a useful cross-check); API access requires registration and an API key, with rate limits. The public MIS site allows anonymous bulk/zip downloads. Both paths are legitimate ingestion routes; the adapter must support both.
Settlement-final vs real-time: replay for settlement-grade P&L SHOULD use the settlement/historical (48-h-delayed, corrected) series, not the indicative real-time series; the adapter MUST record provenance (source_series: RealTimeIndicative | SettlementFinal | Corrected) per row.
D.3.2 PriceSource trait (in batsim-ercot)
rust
/// One market-signal sample, already normalized.
pub struct PriceSample {
    pub ts: DateTime<Utc>,               // interval START, UTC
    pub interval_secs: u32,              // 300 | 900 | 3600
    pub location: Location,              // Hub | LoadZone | Node (D.1.1)
    pub lmp_usd_per_mwh: f64,            // base energy price
    pub ordc_adder_usd_per_mwh: f64,     // scarcity adder component (0 if n/a)
    pub rdpa_adder_usd_per_mwh: f64,     // reliability adder component (0 if n/a)
    pub provenance: Provenance,          // RealTimeIndicative | SettlementFinal | Synthetic
}
pub struct AsPrice { pub ts: DateTime<Utc>, pub product: AsProduct, pub mcpc_usd_per_mw: f64 }
pub struct SystemSignal { pub ts: DateTime<Utc>, pub system_load_mw: f64,
                          pub reserves_mw: Option<f64>, pub fuel_mix: Option<FuelMix> }

pub trait PriceSource: Send + Sync {
    /// DAM hourly SPPs for [start,end). Dam = day-ahead.
    fn dam_spps(&self, loc: &Location, r: TimeRange) -> Result<Vec<PriceSample>>;
    /// Real-time SPPs at native cadence of the source.
    fn rt_spps(&self, loc: &Location, r: TimeRange) -> Result<Vec<PriceSample>>;
    /// DAM AS clearing prices for capacity (hourly, per product).
    fn as_prices(&self, r: TimeRange) -> Result<Vec<AsPrice>>;
    /// System load / reserves / fuel mix (drives 4CP watch & emissions).
    fn system_signals(&self, r: TimeRange) -> Result<Vec<SystemSignal>>;
    /// Streaming view for live mode (Part C stream binding). Default: unimplemented.
    fn subscribe_rt(&self, _loc: &Location) -> Result<BoxStream<'static, PriceSample>> {
        Err(Error::Unsupported("source is not live"))
    }
}
Three mandated implementations:
Replay(archive_root) — reads normalized Parquet archives (§D.3.3). This is the primary mode for strategy backtesting; MUST be deterministic and MUST respect the engine's simulated clock (Part B) rather than wall-clock time.
Live(ErcotAdapter) — polls api.ercot.com / MIS with registered credentials; normalizes on the fly; emits into the stream. Credentials via env/config, never in scenario files.
Synthetic(ScenarioGenerator) — §D.4. Fully deterministic given seed + parameters.
The Part C scenario API binds exactly one PriceSource per simulation run (scenario document field market.price_source). Binding MUST be explicit — no implicit default source.
D.3.3 Ingestion pipeline
Recommended pipeline (lives in a batsim-ercot-ingest binary or subcommand; library code in batsim-ercot):
plain
ERCOT MIS/API (CSV/XML/ZIP) ──► parse/normalize ──► canonical Arrow tables
        ──► Parquet, partitioned:  <signal>/date=YYYY-MM-DD/location=<loc>.parquet
        ──► manifest.json (schema version, source report ID, ingest ts, provenance)
Normative requirements:
Columnar store: Parquet. Read path uses polars (pragmatic choice: fastest developer velocity in Rust for this workload; arrow2-based, lazy scans, predicate pushdown). arrow2 directly is acceptable only if polars' dependency weight is prohibitive; do not introduce both arbitrarily — default: polars.
Partitioning: date (+ location where applicable). Queries in replay mode are overwhelmingly "one day to one month at one location" — the partition layout must make that a single-file or few-file read.
Timestamps: normalize everything to UTC interval-start with explicit interval_secs. ERCOT publishes in CPT (Central Prevailing Time) and uses hour-ending conventions and a 25-hour DST day in fall — the ingest layer MUST handle DST transitions explicitly (duplicate interval in fall-back MUST be preserved and disambiguated; do not silently dedupe). This is a classic replay-corruption bug; add a round-trip test on a DST boundary day.
Schema versioning: every Parquet file carries a schema_version; the replay reader MUST refuse unknown versions with a clear error (fail loud, never silently mis-map columns when ERCOT changes a report).
Corrections: when a price-correction report (NP4-197-M/NP4-196-M style) exists, ingest applies corrections and bumps provenance; raw values are retained in a _raw column for auditability.
D.4 Synthetic Price Scenario Generator
Purpose: stress-test dispatch strategies on conditions beyond recorded history — deeper scarcity, higher solar penetration, lower reserve margins, Uri-class winter events. Implementation: SyntheticPriceGenerator in batsim-ercot, implementing PriceSource.
D.4.1 Model: regime-switching composition
The generator composes, per interval: price = base_dow_shape(season, hour) + renewable_dip + regime_component + noise, floored/capped by scenario caps.
Regimes (Markov-switching, seeded):
Table
Regime   Description   Typical price behavior
Normal   Standard conditions   $15–60/MWh, day-night shape, mild shoulder peaks
SolarNegative   High-solar midday hours   Prices ≤ $0 (down to floor, e.g., −$20 to −$50/MWh) midday; duck-curve evening ramp steepens
ScarcityOrdc   Reserves tight; ORDC adder active   Adder per ORDC-like curve: adder = f(reserves_mw) with reserves drawn below threshold; prices $200–$2,000+, spiky at 5-min scale
WinterStorm   Uri-2021-style extended emergency   Sustained at/near scenario cap for hours–days, then normalization
D.4.2 Parameters (scenario document)
yaml
market:
  price_source:
    kind: synthetic
    seed: 42                      # REQUIRED. Same seed + params => identical series.
    season: summer                # winter | summer | shoulder
    solar_penetration: 0.35       # fraction of midday energy; drives SolarNegative freq/depth
    reserve_margin: 0.10          # lower => more ScarcityOrdc transitions
    regime_matrix: [...]          # optional override of Markov transition probs
    caps: { hcap: 5000, lcap: 2000, emergency_hours_at_hcap: 12 }   # post-Uri defaults; verify
    ordc: { threshold_mw: 7000, steepness: ..., voll: 5000 }        # simplified ORDC params
    event_overlay:                # optional scripted events
      - { kind: winter_storm, start: "2026-02-14T00:00:00-06:00", duration_h: 96, cap: 9000 }
Normative requirements:
Determinism: single seeded RNG (use rand_chacha::ChaCha8Rng or StdRng::seed_from_u64); no wall-clock, no thread-local RNG, no HashMap iteration-order dependence (use BTreeMap in generation paths). Two runs with identical seed+params MUST produce bit-identical price series; CI test required.
Cap correctness: default hcap = 5000, lcap = 2000 (post-Uri regime). Uri-historical replay/overlay uses cap = 9000 — that was the cap in force Feb 2021. Do not mix: a scenario set in 2026 must not silently allow $9,000 prices unless the user explicitly overrides caps.
Emergency pricing: when configured, after emergency_hours_at_hcap cumulative hours at HCAP in a rolling window, drop the effective cap to LCAP. Flag: exact PUCT trigger language must be verified; keep the rule parameterized.
ORDC shape: simplified monotone-increasing-in-scarcity curve is acceptable; this is a stress tool, not a price-forecasting model. Document in output metadata that series are synthetic.
AS prices in synthetic mode: generate per-product hourly MCPCs correlated with regime (ECRS/RRS elevated during ScarcityOrdc/WinterStorm), with configurable saturation discount (AS prices fall as assumed competing storage GW rises — parameter as_saturation_gw).
D.5 Settlement & Revenue Simulation
batsim-ercot::settlement computes, per simulation run, both per-home and fleet-level P&L. All money in USD; all energy in kWh/MWh with explicit units in types (rust_decimal or f64 chosen once in Part C — respect that choice; recommend rust_decimal for money, f64 for physics).
D.5.1 Components
Energy arbitrage — wholesale granularity (fleet-as-resource view):
For each settlement interval i: energy_revenue_i = SPP_i($/MWh) × net_export_mwh_i where net_export_mwh_i is fleet interval energy from Part B integration (§D.7). Charging intervals produce negative revenue (cost). Location = configured settlement point.
Energy arbitrage — retail granularity (retailer view):
Fleet discharge offsets retail purchases: avoided_cost_i = retail_rate_i($/kWh) × discharged_kwh_i minus charging cost at retail rate. The retail rate structure is scenario input (flat, TOU, or wholesale-pass-through like "Griddy-style": retail_rate_i = SPP_i × multiplier + adders). The simulator MUST support both views simultaneously; the retailer margin view (below) reconciles them.
AS revenue (aggregate level):
as_revenue = Σ_hours Σ_products awarded_mw × mcpc($/MW) × performance_factor.
awarded_mw comes from the scenario's AS-award script (bounded by duration-derated capability, §D.1.3).
performance_factor ∈ [0,1] computed from simulated telemetry during deployment events: delivered_mw / instructed_mw integrated over the event, with telemetry-availability derate (if a device is unreachable, its contribution counts zero). Configurable threshold below which a penalty factor applies (e.g., <90% ⇒ factor reduced or clawback multiplier). Exact ERCOT non-performance penalties are protocol-specific — parameterize, verify, and document the chosen simplification.
4CP savings attribution:
For each confirmed 4CP interval k: savings_k = fleet_net_load_reduction_kw_k × transmission_rate_($/kW-mo) × 12/4 (each CP month carries ~1/4 of the annual tag; make the allocation configurable). Attribution per home: proportional to that home's measured contribution during the interval vs. its baseline. Output both candidate-level and confirmed-level numbers.
Retailer margin view:
retailer_margin = retail_energy_revenue_from_customers + as_revenue_share + 4cp_savings + wholesale_energy_revenue(if registered) − wholesale_energy_cost − fleet_program_costs − device_incentive_payments. Program costs and per-home incentive/rebate payments are scenario inputs. Output a single SettlementReport (JSON) with per-home ledger lines and fleet rollups, one row per interval plus monthly/daily aggregates.
Emissions (optional feature flag):
Marginal or average emission-factor timeseries derived from ERCOT fuel-mix report data (§D.3) times per-fuel emission factors (use EPA eGRID-style static factors; configurable). Output emissions_kgco2 per interval alongside energy, and a run-level total vs. a no-dispatch counterfactual. Mark clearly: average-mix attribution is a simplification; marginal-emissions accuracy is out of scope v1.
D.5.2 Round-trip and losses
Charging energy, discharge energy, and round-trip efficiency MUST come from Part B physics (device-level), not assumed fleet-level efficiency. Settlement consumes interval energies only.
D.6 Dispatch Signal Flow — End-to-End Worked Example
Actor model: the user's external optimizer is a client of the simulator's API (Part C). Flow per run:
POST /v1/scenarios — bind fleet (Part A devices), time range, clock mode (replay/accelerated), and market.price_source.
POST /v1/runs — start; engine clock (Part B) begins.
Optimizer subscribes to the price stream: GET /v1/runs/{id}/stream/prices (SSE/WS, Part C).
Optimizer issues dispatch: POST /v1/runs/{id}/dispatch (Part C) — aggregate MW setpoints with validity intervals.
Optimizer consumes telemetry: GET /v1/runs/{id}/stream/telemetry.
At run end: GET /v1/runs/{id}/settlement → SettlementReport (§D.5).
Worked example: summer day, DAM plan → RT 5-min loop → one scarcity spike → settlement
Simulated date 2026-08-14 (CPT shown), fleet = 10,000 homes × ~5 kW / 13.5 kWh.
T-1 13:05 CPT — DAM results available. Optimizer pulls DAM SPPs and AS prices:
JSON
// GET /v1/runs/r_0183/market/dam?date=2026-08-14&location=LZ_HOUSTON
{"date":"2026-08-14","location":"LZ_HOUSTON","unit":"usd_per_mwh",
 "spp_hourly":[31.2,28.7,27.9,27.5,28.1,34.0,45.6,62.3,78.1,92.4,110.5,121.0,
               118.3,125.9,140.2,168.7,240.1,385.6,420.3,310.2,150.8,88.4,55.2,40.1],
 "as_mcpc_usd_per_mw":{"ECRS":[...],"RRS":[...],"NONSPIN":[...]}}
Optimizer submits a day-ahead plan (charge midday on solar-dip prices, hold for evening, reserve 20% headroom for RT spikes, carry 8 MW ECRS award 17:00–21:00):
JSON
// POST /v1/runs/r_0183/dispatch
{"kind":"schedule","tz":"America/Chicago",
 "entries":[
  {"start":"2026-08-14T10:00:00-05:00","end":"2026-08-14T15:00:00-05:00","aggregate_mw":-12.0,"note":"charge (solar dip)"},
  {"start":"2026-08-14T17:00:00-05:00","end":"2026-08-14T21:00:00-05:00","aggregate_mw":25.0,
   "as_commitment":{"product":"ECRS","awarded_mw":8.0},"note":"evening discharge + ECRS"},
  {"start":"2026-08-14T21:00:00-05:00","end":"2026-08-15T00:00:00-05:00","aggregate_mw":0.0}
 ]}
18:32 CPT — real-time loop. Per interval, the stream emits:
JSON
// stream/prices event
{"ts":"2026-08-14T18:30:00-05:00","interval_secs":900,"location":"LZ_HOUSTON",
 "lmp_usd_per_mwh":212.45,"ordc_adder_usd_per_mwh":0.0,"rdpa_adder_usd_per_mwh":0.0,
 "provenance":"RealTimeIndicative"}
// stream/telemetry event
{"ts":"2026-08-14T18:30:00-05:00","interval_secs":900,
 "aggregate":{"delivered_mw":24.7,"baseline_mw":41.3,"metered_mw":16.6,
              "soc_mean":0.58,"unavailable_mw":1.9,"homes_reporting":9981}}
18:45 CPT — scarcity spike. Reserves drop; ORDC adder engages for three intervals:
JSON
{"ts":"2026-08-14T18:45:00-05:00","interval_secs":900,"location":"LZ_HOUSTON",
 "lmp_usd_per_mwh":389.10,"ordc_adder_usd_per_mwh":2160.88,"rdpa_adder_usd_per_mwh":0.0,
 "provenance":"RealTimeIndicative"}            // effective SPP = 2549.98
Optimizer overrides the plan for the spike window (release the held 20% headroom; ECRS deployment event also arrives):
JSON
// POST /v1/runs/r_0183/dispatch
{"kind":"override","start":"2026-08-14T18:45:00-05:00","end":"2026-08-14T19:15:00-05:00",
 "aggregate_mw":41.0,"priority":"immediate","as_deployment":{"product":"ECRS","deployed_mw":8.0}}
The engine (Part B) disaggregates across homes, respects SOC/ramp/availability, and returns physical reality, e.g. delivered 38.2 MW of the 41.0 MW requested (partial — some homes offline/depleted). The spike lasts 3 intervals; prices revert.
Run end — settlement:
JSON
// GET /v1/runs/r_0183/settlement  (abridged)
{"run_id":"r_0183","settlement_interval_secs":900,
 "energy":{"wholesale_usd":48112.36,"retail_avoided_cost_usd":39877.10,
           "charging_cost_usd":-5120.44},
 "as":{"ECRS":{"awarded_mwh":32.0,"mcpc_avg_usd_per_mw":184.20,
       "gross_usd":5894.40,"performance_factor":0.93,"net_usd":5481.79}},
 "four_cp":{"candidate_intervals_hit":1,"candidate_reduction_kw":38200,
            "est_annual_savings_usd_at_3.5_per_kw_mo":401100,
            "status":"candidate_unconfirmed"},
 "retailer_margin_usd":79120.81,
 "emissions_kgco2_delta_vs_counterfactual":-18230.5,
 "baseline_method":"LastNDaysAverage{n:10, exclusion: event_days}",
 "provenance":"SettlementFinal"}
Normative points: the spike handling demonstrates why interval cadence matters — 3 intervals × ~38 MW × ~$2,550/MWh ≈ $48k, roughly the day's entire energy margin; a 15-min-resolution-only replay that averages away the 5-min structure would misprice this. (See §D.1.1 on the 5-min vs 15-min settlement verification.)
D.7 Time / Cadence Alignment
The engine ticks devices at 1-second resolution (Part B); markets settle at 5/15-min (RTM) and hourly (DAM/AS). Alignment rules:
Interval energy integration: for each device and each settlement interval, energy_kwh[device,i] = ∫ P dt over the interval, computed by exact accumulation of 1-s powers (trapezoidal on tick boundaries is acceptable; error < 0.1% at 1-s ticks). Fleet interval energy = Σ devices. Never approximate interval energy as P_avg_snapshot × Δt from a single sample.
Interval-average telemetry: telemetry stream emits interval aggregates: mean power, min/max SOC, delivered/baseline/metered MW, homes-reporting count. Sub-interval detail is available only via run recording, not the live stream (keeps stream volume bounded).
Clock authority: Part B's engine clock is the single time authority; PriceSource::Replay is indexed by simulated time; interval boundaries derive from epoch_ts + n × interval_secs in UTC, with CPT conversion only at presentation boundaries.
DAM hourly: DAM signals are step functions over clock hours (CPT hour-ending normalized to UTC starts, §D.3.3 DST rule).
AS response — explicit boundary: AS deployments (RRS/ECRS) are modeled as simplified ramp compliance: on deployment instruction at t0, each device ramps toward its allocated share at its Part A ramp rate (or a default fleet ramp, e.g., full output within 60 s), subject to SOC and availability; performance is scored on delivered-vs-instructed MW integrated from t0 + response_deadline(product) (e.g., 10 min for ECRS) onward. The engine MUST NOT attempt sub-second frequency physics — no inertia, no droop curves, no frequency-domain simulation, no synthetic grid frequency signal. RRS "seconds-scale" response is modeled as the same ramp with a shorter deadline. Any requirement finer than 1-s device response is out of scope by design; strategies that depend on sub-second dynamics cannot be evaluated here and the spec must say so rather than fake it.
4CP intervals: tracked at the RTM settlement-interval cadence (15-min candidates); system-load signal from system_signals().
D.8 Data Freshness & Caveats (MUST-VERIFY Checklist)
ERCOT report formats, endpoints, report IDs, cadences, market rules, and product definitions change (NPRR process; the RTC+B cutover of Dec 5, 2025 is a recent example that discontinued/replaced real-time data products, including the ORDC adders/reserves report NP6-323-CD). All adapter specifics in this Part MUST be verified against current ERCOT MIS / api.ercot.com documentation and the current Nodal Protocols at build time. The implementer MUST record, in the repo (adapter README or config comments), the verification date and source URL for each item below.
MUST verify before implementing adapters:
Report IDs, endpoints, column layouts, and file formats for every data product in §D.3.1 — including post-RTC+B replacements for discontinued reports.
Real-time settlement interval length post-RTC+B (5-min vs 15-min) and the exact definition/timing of RTM SPP publication.
HCAP/LCAP values ($5,000 / $2,000 assumed) and the Emergency Pricing Program trigger (hours-at-cap rule).
ORDC parameters (threshold, curve shape/VOLL) and RDPA applicability.
AS product set, response/duration requirements, and duration-based eligibility/derate rules for storage and aggregations (ECRS 10-min/2-h; Non-Spin duration; RRS qualification); status of DRRS.
ADER pilot status, aggregation rules, baseline methodology(ies), and telemetry requirements — all pilot details are provisional.
4CP definition details (months, interval definition, which costs flow through) and current transmission-rate magnitudes.
api.ercot.com registration/auth/rate-limit terms; MIS bulk-download terms of use.
Load zone / hub / settlement-point enumerations (NP4-160-SG-style mapping product).
Publication delays (48-h settlement reports; 60-day disclosure reports) and price-correction workflows.
Design consequence (normative): every ERCOT-specific constant lives in versioned configuration (batsim-ercot/config/ercot_rules.v<protocol-year>.toml), never as a bare literal in logic. The rules-config version is recorded in every SettlementReport and every ingested Parquet manifest, so results are auditable against the rule set that produced them.
Explicit non-goals for this Part: no bidding/clearing simulation of ERCOT markets themselves; no other ISO; no QSE wire protocols; no sub-second grid physics; no price forecasting (replay + synthetic stress only).
End of Part D.
