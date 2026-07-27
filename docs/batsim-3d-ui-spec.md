batsim 3D Web UI — Implementation Specification
Codename: batsim-ui
Companion to: Residential Battery Fleet Simulator — Implementation Specification (batsim, v1.0)
Scope: ERCOT-only fleet visualization & operations UI; pure client of the batsim API
Stack: React + TypeScript + three.js (React Three Fiber) + MapLibre GL, WebGL2
Audience: AI implementation agents (this document is the complete build brief)
Version: 1.0 — 2026-07-26
Status: Approved for implementation
0. Executive Summary
batsim-ui is the visual operations layer for the batsim residential battery fleet
simulator. The experience is best described as Factorio × Cities: Skylines × Bloomberg
terminal: a living scale model of a retailer's ERCOT service territory where every home
is inspectable, every watt is animated, and the market is always ticking.
The operator starts at a stylized 3D map of Texas — ERCOT load zones glowing with
real-time settlement prices, fleet clusters breathing with aggregate SOC — and zooms
seamlessly down: city, neighborhood street, dollhouse cutaway of a single home, all the
way into a Powerwall 3's SOC gauges. Homes are placed and configured from the real batsim
device registry (Tesla, Enphase, SolarEdge, sonnen). Time is a toy: pause, 3600×,
jump-to-next-scarcity-event. Snapshots are savegames; branching one is an A/B strategy
test. It is not a game — there are no points — but it is playable: the score is real
simulated settlement P&L against ERCOT.
The four commitments inherited from batsim, expressed in the UI:
Truth over beauty. Every visual element binds to a named telemetry field from the
batsim API. Nothing animates that isn't computed by the simulator.
Zoom is the interface. Five camera strata (Z1 grid → Z5 device) with seamless
transitions; the right information density appears at each altitude.
Server-authoritative. The UI never fakes a device response; dispatch commands
reconcile against the batsim audit log and telemetry.
60 fps with 10,000 homes. Instancing, LOD chains, chunked streaming, and worker-side
telemetry rings keep the main thread inside a 16.6 ms frame budget.
Document Map
Table
Part   Contents
Part A   Experience & game design — pillars, the Zoom Continuum, modes/tools, time UX, session loop, accessibility, non-goals
Part B   3D tech architecture — stack, MapLibre↔three.js hybrid solution, 10k-home scale architecture, telemetry-driven rendering, performance engineering, asset pipeline, testing
Part C   Data binding & batsim API integration — generated client, state architecture, telemetry pipeline, dispatch command path, time sync & savegames, resilience, type sketches, sequence diagrams
Part D   Visual scenes, assets & feedback — art direction & design tokens, scene-by-scene spec Z1–Z5, energy-flow particles, event choreography, asset inventory, HUD wireframes
0.1 Feature Set
Priority tiers: P0 = MVP (playable vertical slice), P1 = v1 (operations-grade),
P2 = v2 (delight & differentiation). References point to part sections.
World & Navigation
Table
#   Feature   Tier   Spec
U1   Zoom continuum Z1–Z5 with seamless MapLibre→three.js crossfade and unified camera math   P0   A.2, B.2
U2   Z1 ERCOT grid layer: stylized Texas, load zones, hub price chips, fleet clusters, price/SOC/outage/revenue/health lenses   P0   A.2, D.2
U3   Z3 procedural neighborhoods: seeded streets, 4–6 house archetypes, SOC rings, service-line energy flows   P0   B.3, D.2
U4   Z4 dollhouse cutaway with per-OEM equipment layout and AC/DC coupling flow visualization   P1   A.2, D.2
U5   Z5 device focus: trademark-safe OEM unit views, SOC/SOH/temp/efficiency panels, command mini-log   P1   A.2, D.2
U6   Day-night cycle and weather fronts driven by sim time & scenario feeds   P1   D.1
Simulation Control
Table
#   Feature   Tier   Spec
U7   Time controls: pause/play/speed multiplier, jump-to-settlement-interval, run-until-condition, dual-clock HUD   P0   A.4, C.5
U8   Timeline scrubber over local telemetry rings (snapshot-restore beyond retention)   P1   A.4, C.5
U9   Snapshot savegames: create/label/restore/branch; A/B strategy compare   P1   A.4, C.5
U10   Scenario mode: bind ERCOT replay date / synthetic regime / outages / seed   P0   A.3, C.6
Build & Operate
Table
#   Feature   Tier   Spec
U11   Build mode: place home archetypes, registry-driven OEM system picker, PV/backup/EV configuration   P0   A.3, C.2
U12   Inspect mode: click any entity to its telemetry truth; box-select + attribute filters   P0   A.3
U13   Dispatch mode: individual/group/fleet commands with in-flight ack visualization, idempotency, scheduling, undo   P0   A.3, C.4
U14   KPI HUD: fleet MW/MWh, aggregate SOC, LMP exposure, realized-vs-projected revenue, 4CP gauge   P1   A.5, D.6
U15   Alert choreography: scarcity pulse, rolling outage dark-wave, backup transfer flicker, dispatch ripple, settlement ticks   P1   D.4
Engineering
Table
#   Feature   Tier   Spec
U16   10k homes at 60 fps: LOD chain, instancing, chunk streaming, CI-gated draw/triangle budgets   P0   B.3, B.5
U17   Worker-side telemetry rings with WS/SSE transport seam, gap detection, resync   P0   C.3, B.4
U18   OpenAPI-generated TS client with breaking-change CI gate   P0   C.1
U19   Offline demo mode: recorded JSONL traces replayed through the real pipeline   P1   C.7
U20   Accessibility: CVD-safe palettes with shape redundancy, reduced-motion fallback, keyboard parity   P1   A.7, D.1
U21   Visual regression + performance CI gates per zoom stratum   P1   B.8
U22   Optional audio layer (load-driven hum, event chimes) — OFF by default   P2   D.7
Explicit Non-Goals
No multiplayer, no fantasy gamification (points/badges/levels) — the score is settlement P&L.
No mobile-native app; desktop web first, tablet tolerant.
No editing of batsim physics or device registry from the UI.
No new simulation backend: static hosting + optional thin BFF only.
0.2 Build Milestones (for AI implementation agents)
Table
Milestone   Deliverable   Exit criteria
M1 — Vertical slice   Z1 map with live price lens + one Z3 neighborhood, 250-home demo fleet, time controls, inspect mode (U1–U3, U7, U10, U12, U16–U18)   Playwright golden path: connect to batsim, bind scenario, watch 24 simulated hours at 3600× without frame-budget CI failure
M2 — Operate   Build mode, dispatch mode with ack ripple, KPI HUD, alerts (U11, U13–U15)   E2E: build 10 homes from registry → dispatch fleet during synthetic scarcity event → settlement counter increments; zero optimistic-UI violations in audit reconciliation
M3 — Deep zoom & saves   Z4 cutaway, Z5 device views, scrubber, snapshot savegames + A/B compare (U4, U5, U8, U9)   Snapshot branch produces hash-identical rerun; scrub within ring retention is instant; A/B compare provenance gates pass
M4 — Polish & hardening   Day-night, weather, full event choreography, accessibility suite, offline demo, audio (U6, U19, U20, U22)   10k-home 60fps CI gate green; CVD/reduced-motion audits pass; demo mode runs with no batsim process
Determinism note: the UI's procedural world generation is seeded from the batsim scenario
seed — same scenario, same streets, every time (B.3).
batsim 3D Web UI — Part A: Experience & Game Design
Status: normative for UX/game-design decisions. Sibling parts: Part B (3D tech stack & rendering architecture), Part C (data layer: API client, streaming, state store), Part D (scene-by-scene art direction & asset detail). This document defines what the operator experiences and why; it deliberately does not choose engines, shaders, or REST plumbing — where those matter, it names the contract and points to the sibling part.
Product stance, stated once: batsim-ui is a professional operations console that borrows its feel from city-builders and Factorio — immediacy, inspectability, systems visibly running — and borrows nothing from games' progression systems. There are no points, no badges, no unlocks, no loot. The score is realized ERCOT settlement P&L, and it is real.
1. Design Pillars
Six pillars. Every UX decision in Parts B–D must trace to at least one; any feature that contradicts one needs explicit sign-off.
P1 — The fleet is alive
The map is never a static dashboard. PV output breathes with the sun, batteries charge and discharge, prices flicker every 5-minute settlement interval, HVAC cycles. Even at 1x, something is always moving somewhere in frame. Motion is telemetry rendered, not decoration: every animation on screen MUST be traceable to a /v1/telemetry field or a dispatch event. No ambient animation that lies.
P2 — Every watt is inspectable
If energy flows, the operator can see it, select it, and read its number. There is no aggregated value in the UI that cannot be drilled into down to a single device's telemetry stream. Hover any flow, any home, any bubble: the number and its source field are one gesture away. This is the Factorio principle — everything running is clickable — applied to power.
P3 — Zoom is the interface
The camera IS the information architecture. We do not bury detail in page navigation or modal stacks; detail appears as you zoom in, context appears as you zoom out. Five continuous strata (§2) with no hard "screens" between them. An operator should be able to go from "Texas" to "the cell temperature of one Powerwall 3 in Plano" in under five seconds, by scrolling.
P4 — Consequences are visible
Every dispatch command produces a visible effect within one telemetry frame of its (jittered, per-device) execution: homes physically change state, flows reverse, the settlement ticker moves. Every market event has a spatial footprint on the map. The operator must never wonder "did that command do anything?" — the world answers.
P5 — Operator, not player
The user is an energy-retailer operations engineer or quant validating dispatch strategies against ERCOT. UI language is the industry's: LMP, ORDC adder, 4CP, DAM, RT settlement, AS, reserve SOC. No quest framing, no "level up," no celebratory confetti. Delight comes from legibility and control, not from game rewards. Fast, dense, keyboard-driven; every action reachable by a professional in ≤2 inputs.
P6 — Truth over beauty
When a rendering choice conflicts with numerical fidelity, fidelity wins. The UI MUST NOT smooth, interpolate, or extrapolate telemetry in ways that misrepresent device state; visual interpolation between telemetry samples is allowed for motion continuity but tooltips and readouts always show the last true sample with its timestamp. Simulated time, not wall time, is the canonical clock (§4).
Target user (normative)
Primary: fleet operations engineer / energy trader at a Texas retail electric provider, running dispatch strategies against ERCOT replay and synthetic scenarios. Secondary: quant/strategy developer A/B-testing control policies via snapshot branching. Tertiary: sales/exec demo viewer — the UI must be impressive in a conference room without compromising the primary user. We design for the operator on a 27"+ desktop monitor; everyone else is tolerated, not optimized for.
2. The Zoom Continuum
The core UX invention. One continuous camera, five semantic strata (Z1–Z5). Zoom is geometric and seamless — no loading screens, no mode switches — but each stratum has a defined information contract. Transitions are driven by scroll/pinch (continuous) and by click-to-focus (discrete jump that animates the camera to the object). Strata are not discrete "levels" in code; they are camera-distance bands with crossfaded layers (Part B defines the LOD/crossfade machinery; this section defines what MUST be visible and interactive in each band).
Stratum summary
Table
Stratum   Altitude metaphor   Primary question answered   Telemetry density   Canonical interactions
Z1 — ERCOT Grid   Satellite   "What is the market doing, and where is my fleet exposed?"   Aggregates only: per-zone fleet MW/MWh, avg SOC, LMP   Lens switching, zone click → Z2, fleet bubble click → Z3
Z2 — City / Territory   Helicopter   "How is this cluster composed and performing?"   Cluster aggregates + per-home state glyphs   Cluster select, group-dispatch targeting, click street → Z3
Z3 — Neighborhood   Drone at rooftop height   "Which homes are doing what, right now?"   Full per-home: state, SOC ring, net flow, PV   Multi-select, per-home dispatch, click home → Z4
Z4 — House cutaway   Inside the walls   "Where is energy going inside this home?"   Full home: battery, inverter, PV, per-load breakdown, grid exchange   Device selection, load inspection, click battery → Z5
Z5 — Device view   Standing at the unit   "Is this specific OEM unit behaving correctly?"   Full device stream: SOC/SOH, temps, P in/out, efficiency, mode, command log   Mode/setpoint edit, command history, vendor-API view
Z1 — ERCOT Grid Layer
Visible: stylized Texas map with ERCOT load zones (LZ_HOUSTON, LZ_NORTH, …) and hub boundaries; real-time price heatmap driven by the active scenario's price stream (RT settlement point prices + ORDC adder emphasized); fleet aggregate bubbles per zone sized by fleet MW, colored by aggregate dispatch state (net charging / discharging / idle); animated weather fronts bound to the scenario's weather channel; transmission congestion hints (zone-to-zone gradient arrows when price separation exceeds a threshold).
Interactive: lens selector (§2.6); click a zone → camera descends to Z2 centered on that zone's served territory; click a fleet bubble → descends toward the densest cluster; hover a zone → LMP, adder, fleet MW/MWh, net position.
Telemetry density: coarse. 5-minute settlement cadence is native here; sub-minute device data is intentionally aggregated away. This is the trading-floor view.
Operator value: this is the "Bloomberg terminal" stratum — scarcity events and price separation should be visible from across the room.
Z2 — City / Territory
Visible: clusters of served homes (subdivision-scale groupings), each cluster annotated with fleet composition chips (count by OEM system: PW2/PW3, IQ 5P/10C, SolarEdge 400V, ecoLinx/Core+), aggregate SOC bar, net fleet flow arrow to the grid, per-cluster revenue accumulator.
Interactive: lasso/box-select homes across clusters; click cluster → Z3; group-dispatch targeting (selection becomes the home_ids payload for /v1/dispatch); filter-by-attribute highlighting (§3.5).
Telemetry density: cluster rollups + per-home state glyphs (tiny colored markers on rooftops) — enough to spot the one misbehaving home in a 500-home cluster.
Z3 — Neighborhood
Visible: individual homes on streets. Each home carries: a state glyph (charging / discharging / idle / backup-islanded / offline), an SOC ring around the house (arc fill = SOC%, color-shifted at reserve floor), a PV flow indicator, and an energy-flow animation on the service line to the grid — direction and particle rate encode power sign and magnitude. Outage events darken the street's grid feed while backup-capable homes stay lit (pillar P4 made literal).
Interactive: click home → Z4; shift-click / box-select multi-home; right-click → dispatch context menu on selection; hover → live mini-panel (P, SOC, mode, LMP exposure).
Telemetry density: full per-home resolution at the stream's native rate; this is the default "watching the fleet work" stratum and the emotional heart of the product.
Z4 — House Interior / Cutaway
Visible: cutaway of the selected home's HomeSystem topology as composed by batsim (battery unit(s), inverter/hybrid inverter, system controller/gateway, PV array on the roof, critical-loads panel, main panel, meter). Live wiring-flow animations along the actual energy path for the home's coupling type (AC-coupled path with its L1/L2/L3 losses vs DC-coupled hybrid path — the topology tag from the registry determines the rendered path). Load breakdown panel: HVAC, EV charger, pool pump, water heater, other — each with live kW.
Interactive: click any component → Z5 for batteries/inverters, or a spec sheet popover (registry data) for others; toggle per-load visibility; scrub recent home-level telemetry in the side panel.
Telemetry density: full home stream, 1-s tick cadence as delivered by the stream layer (Part C). Losses are rendered as heat/dim-out on conversion stages — efficiency is visible, not just tabulated (P2).
Z5 — Device / Battery View
Visible: the selected OEM unit as the hero object, rendered with vendor-faithful enclosure (Part D). Live instrument cluster: SOC, SOH, cell/pack temperature, power in/out, instantaneous and session round-trip efficiency, operating mode (self-consumption / backup / TOU / grid-services), reserve SOC floor, firmware/registry model_id + entry_version, and a scrollback of recent commands from the dispatch audit log (with execution-latency annotation — batsim jitters per-device execution like real cloud APIs, and the UI shows commanded-vs-executed deltas honestly).
Interactive: edit mode / setpoint / reserve for this unit (→ /v1/dispatch, single-home scope); open the unit's vendor-API mimicry view (what the real Tesla/Enphase/SolarEdge/sonnen API would report — invaluable for integration debugging); pin this device to a watchlist that persists across zoom.
Telemetry density: maximum. This is the verification stratum — "zoom into any house and see what the battery is doing."
2.6 Lenses (global overlay system)
Lenses are map-wide recoloring/annotation layers, orthogonal to stratum (though density adapts). Any lens, any zoom, toggled instantly — including mid-transition.
Table
Lens   Encoding   Available strata   Primary use
Price lens   Heatmap by LMP + ORDC adder; iso-price contours at Z1, per-home exposure tint at Z3   Z1–Z3   Spot scarcity, price separation, exposure hotspots
SOC lens   Color = SOC with diverging midpoint at each home's reserve floor; below-reserve homes pulse   Z2–Z4   Reserve compliance audit, pre-event readiness
Outage lens   Grid feed state per segment; islanded homes ringed; backup-runtime estimate badges   Z1–Z4   Storm scenarios, backup verification
Revenue lens   Cumulative settlement P&L per home/cluster/zone, green/red against day plan   Z1–Z3   "Where did today's margin come from?"
Health lens   SOH / alerts / offline status; comms-dead devices in hazard hatch   Z2–Z5   Maintenance triage, offline-device hunt
Transition triggers (normative): scroll/pinch adjusts camera continuously; double-click an object focuses it (camera flies to that object's stratum); Esc ascends one stratum; breadcrumb HUD ("ERCOT ▸ LZ_NORTH ▸ Plano North ▸ 4512 Sage Hollow ▸ PW3 #2") is clickable for direct jumps. Transitions MUST be interruptible — the operator can reverse zoom mid-flight without animation queueing.
3. Modes & Tools
The UI has four tools (not screens), switched from a left rail or by hotkey. Camera and lenses are always live in every mode. The active tool determines what clicks mean.
3.1 Build mode
Create and edit the simulated world. Only tool that mutates home/fleet composition (homes/fleets CRUD).
Place home archetype: stamp homes from a template palette (e.g., "1970s ranch, gas heat", "2015 two-story, all-electric, pool", "new construction, EV + 8 kW PV"). Placement is on the map within an ERCOT zone — zone membership determines price exposure, so where you build is a strategy decision (P4).
Choose OEM system: the system picker is a live view over the batsim device registry (/v1/registry/batteries, /inverters, /controllers) — NOT a hardcoded catalog in the UI. Each card shows nameplate kW/kWh, coupling type (AC/DC-hybrid), backup capability, chemistry, and provenance-tagged spec notes. The picker MUST reflect the loaded registry_version (surfaced in the footer) and degrade gracefully (read-only banner) if the registry is unreachable.
Configure: per-home PV kW preset, battery quantity (multi-unit homes), backup/critical-loads panel toggle (requires a controller entry per registry composition rules), EV presence/charge power. The UI composes a HomeSystem object and submits it; server-side validation is authoritative — the UI renders validation errors (e.g., "backup asserted without a system controller") inline on the affected home.
Fleet organization: assign homes to fleets; fleet = dispatch and reporting scope.
3.2 Inspect mode (default)
Read-only truth-seeking. Click selects, hover reveals, lenses apply. Selection panel shows the object at its native fidelity with links to drill (home → topology → device). Multi-select shows comparative table + aggregate stats. This mode owns the "every watt is inspectable" pillar and is the tool the operator lives in while the sim runs.
3.3 Dispatch mode
Send control to individual homes, arbitrary selections, or whole fleets — one consistent interaction at three scopes.
Payload builder: kW setpoint (charge negative / discharge positive), reserve SOC floor, operating mode (self-consumption / backup / TOU / grid-services), PV curtailment, optional duration/schedule. Submits to /v1/dispatch with client-generated idempotency key; the UI displays per-home acks as they arrive (jittered execution latency is shown, homes flip state one by one — this realism is a feature, not a bug).
Group/fleet dispatch: current selection (from any stratum) becomes scope; a confirmation sheet previews affected home count, aggregate nameplate MW, and expected SOC floor conflicts before send.
Schedule editor: time-of-day dispatch programs on the scenario timeline (e.g., "16:00–19:00 discharge at 80% aggregate, hold 20% reserve") — the UI represents these as timeline blocks, submitted as scenario/dispatch configuration.
Audit surface: every command, ack, and per-device execution timestamp is inspectable from the device view (Z5) and a global command log. No silent control.
3.4 Scenario mode
Bind the simulation's world conditions (writes /v1/scenarios-equivalent configuration):
Time range + clock seed; determinism seed is first-class (displayed, copyable — reproducibility is a product feature).
Price source: ERCOT historical replay (pick a date — e.g., a summer scarcity day or the Uri window) or the seeded synthetic generator with regime selection (normal / negative-solar / ORDC scarcity / Uri-style storm, with post-Uri caps handled server-side).
Weather binding (drives PV + loads) and outage scripts (grid-loss events by zone/segment/time window — the input to the Outage lens and backup behavior).
Scenario config is summarizable as a one-line "world card" in the HUD at all times: REPLAY 2023-08-17 · LZ_NORTH-heavy · seed 0xA41F · 500 homes.
3.5 Selection, filtering, and control reference
Selection grammar (all modes): click = select; shift-click = add; box-select (drag) = multi; lasso at Z2+; double-click = focus-zoom; Esc = clear/ascend. Filter-by-attribute: a query bar (vendor:tesla soc<30 mode:backup fleet:west) highlights matches and dims non-matches at any stratum; filter result is convertible to a selection (and thus a dispatch scope) in one click. Filters compose with lenses.
Control reference (normative defaults; remappable):
Table
Input   Action
Scroll / pinch   Zoom continuum (Z1↔Z5)
RMB-drag / MMB-drag   Pan / orbit (orbit enabled Z3–Z5)
1/2/3/4   Build / Inspect / Dispatch / Scenario tool
L then 1–5   Lenses: price / SOC / outage / revenue / health
Space   Pause/play sim
, / .   Speed down/up (1x → 60x → 3600x)
F   Focus selection · Esc ascend stratum / clear
Ctrl+D   Dispatch current selection
Ctrl+S / Ctrl+Shift+S   Snapshot (quick-save) / named snapshot
Ctrl+B   Branch snapshot (fork save)
T   Timeline scrubber toggle
Gamepad (optional)   Right stick zoom, left stick pan, triggers speed, A select, Y dispatch menu — full parity NOT required; camera + time control only
Keyboard-only operation MUST be possible for every operator-critical path (§7).
4. Time as a Toy
The virtual clock is a first-class plaything, presented with the confidence of a game speed control and the precision of a trading system.
Transport bar (always visible): pause / play / step (one settlement interval, one minute, one tick — user-selectable quantum) / speed multiplier 1x · 60x · 3600x mapped to /v1/sim control. Speed changes are instant and safe at any zoom.
Dual-clock HUD (normative): simulated time and wall time are ALWAYS both visible, e.g. SIM 2023-08-17 16:42:10 CDT · 60x · WALL 14:03:22. When paused, SIM time is framed. The UI must never leave the operator ambiguous about which clock a number belongs to; every chart axis labels its time base.
Jump-to-settlement-interval: the timeline is ticked in ERCOT 5-minute settlement intervals; snapping jump (prev/next interval keys) exists because settlement accounting thinks in those quanta.
"Run until": run-until-condition control — e.g., run until price ≥ $1,000/MWh, run until 16:00, run until fleet SOC < 25% — a UI expression of the API's run-until primitive. This is the operator's trap-setting tool: define the condition, walk away (or speed 3600x), get interrupted exactly at the interesting moment.
Timeline scrubber: recorded telemetry is scrubbable — drag backwards to review the scarcity spike, then resume live. Scrubbing is a view operation (does not rewind the sim); rewinding the sim is done by restoring a snapshot, and the UI keeps these two concepts visually distinct (scrub = grey ghost overlay; restore = explicit dialog). This distinction MUST be unambiguous — confusing "looking at the past" with "changing the past" is a category error we refuse to ship.
Snapshots = savegames (the core metaphor): named saves ("pre-DAM baseline", "post-Uri t+2h"), quick-saves, autosaves on scenario events, full metadata (scenario config, seed, registry version, sim time). Branch a save: restore a snapshot as a fork to A/B-test dispatch strategies — "what if we'd held 30% reserve through the 17:00 spike?" becomes a two-click experiment. Branch lineage is shown as a simple tree in the snapshot browser; each branch's settlement P&L is diffable against its parent (§5 KPI). Snapshot integrity (hash) is surfaced as a trust mark.
5. The Session Loop
The "game loop" is a work day against the ERCOT market. Three loop shapes are supported; none is enforced.
5.1 The canonical operator day (narrative target)
Morning — DAM plan review (Z1, price lens): load the day's scenario (replay or synthetic), scan the price heatmap, check day-ahead positions. Fleet at overnight SOC; verify reserve floors. Take a snapshot: morning-baseline.
Midday — solar watch (Z2/Z3, SOC lens): PV ramps; watch SOC rings fill across neighborhoods; catch the two homes underperforming (health lens: one offline device, one clipped hybrid inverter). Log the offline unit.
Afternoon — the event (alerts fire): ORDC scarcity adder climbs; price alert at the configured threshold (or 4CP risk gauge enters the window). Operator slams to 3600x to the event edge via run-until, drops to Z1 to read exposure, box-selects the west fleet, opens Dispatch mode: fleet-wide discharge setpoint with a protected reserve floor, confirm sheet shows 3.1 MW aggregate. Send. Homes flip state across the map in jittered waves (P4). Zoom to Z3 and watch the flows reverse down the streets.
Settlement tick-up (revenue lens): 5-minute intervals settle; the realized-vs-projected revenue line diverges upward; KPI HUD updates. Snapshot post-event.
Evening — A/B review: branch morning-baseline, re-run the afternoon with the alternate strategy, diff settlement outcomes in the snapshot browser. Tomorrow's strategy meeting has receipts.
5.2 Scenario challenges (structured sessions)
Pre-packaged, sharable scenario definitions (no win screens — success is measured in settlement and reserve compliance):
"Uri Week" (storm scenario): Uri-style synthetic regime, multi-day, zone outages, scarcity pricing under post-Uri caps. Operator objective: keep backup-capable homes' critical loads served, manage reserve floors, and survive settlement. Outage lens is the star.
"August Scarcity Afternoon": single-day high-heat replay/synthetic with ORDC spikes and 4CP coincidence risk. Objective: maximize discharge revenue into the peak without breaching reserve commitments.
"Negative-noon": negative-solar regime — midday negative prices. Objective: charge cheap/negative, avoid curtailment losses, position for the evening ramp.
Challenges are configuration, not content: each is a scenario JSON + a suggested KPI target range, forkable and seed-locked for team comparability.
5.3 Sandbox
Unconstrained build-and-play: any fleet, any synthetic regime, any seed, free time control. Default for exploration, demos, and hardware verification ("does this SolarEdge 400V hybrid clip at 10 kW PV + full discharge? Zoom to Z5 and find out").
5.4 Alerts & notifications
Severity tiers: Info (toast, auto-dismiss) / Warning (toast + queue badge) / Critical (persistent rail banner + optional audio, requires ack).
Canonical alerts: scarcity price threshold crossing (with current adder), 4CP window proximity, device offline / comms loss, reserve-SOC breach (per home and fleet aggregate), dispatch command partial-failure (n of m homes acked), outage event start/end, scenario end.
Principles: alerts are spatially anchored — clicking one flies the camera to the offending homes at the right stratum. Alerts never steal pointer focus. All alerts are logged and filterable. No alert may fire on interpolated/predicted data — only on true telemetry or market events (P6).
5.5 KPI HUD (persistent, collapsible)
Table
KPI   Source   Notes
Fleet nameplate MW / MWh   registry-derived aggregate   split by fleet
Aggregate SOC % + reserve headroom   telemetry rollup   red when headroom < configured floor
Current LMP exposure   scenario price stream × fleet position   per-zone breakdown on hover
Realized vs projected revenue   settlement P&L (wholesale/retail arbitrage + AS with derates + 4CP attribution)   the "score"; charted per settlement interval
4CP risk gauge   coincident-peak probability   amber inside window, red at candidate peak
Sim/wall clock + scenario world card   /v1/sim, scenario config   always visible (§4)
6. Onboarding & Empty States
First run (empty world): no blank canvas. A guided build flow in three steps: (1) pick an ERCOT zone on the Z1 map — the map itself is the tutorial; (2) place a home from a starter template ("all-electric two-story + 8 kW PV"); (3) open the registry picker and install a recommended system (PW3 suggested, alternatives one click away). The flow ends by pressing play — the first thing every new user sees is their home breathing on the map. Total target time-to-alive-world: < 3 minutes.
Demo fleet option: one click instantiates a 250-home, multi-vendor demo fleet with a pre-bound replay day (a known scarcity day), so the full product — dispatch, alerts, settlement — is demonstrable with zero configuration. Demo fleet is clearly labeled and disposable.
Empty states elsewhere (no fleets, no snapshots, no alerts) each carry a single-sentence explanation + one primary action. Never a dead end.
Tooltips-as-documentation: every control, glyph, lens, and KPI has a hover tooltip; every tooltip's bottom line names the underlying batsim concept or endpoint (POST /v1/dispatch · reserve_soc_pct) so the UI doubles as API documentation. A "pro tips" rotation teaches zoom grammar, run-until, and snapshot branching contextually (first use of each mode), dismissible forever.
Progressive disclosure: advanced surfaces (vendor-API mimicry view, schedule editor, filter query language) are discoverable but never on the first-run path.
7. Accessibility & Comfort
Colorblind-safe palettes (normative): price heatmap uses a perceptually uniform sequential scale (viridis-class), NOT red-green; SOC uses a blue-amber diverging scale keyed to reserve floor; revenue uses teal-magenta with shape/arrow redundancy. No state may be encoded by hue alone — state glyphs pair color with icon/shape (charging ▲, discharging ▼, backup ◈, offline ✕). All palettes pass deuteranopia/protanopia simulation checks in CI (Part D owns tokens; this part owns the requirement).
Reduced-motion mode: one global switch (also auto-honors prefers-reduced-motion): energy-flow particles collapse to static directional arrows with numeric labels; camera transitions become fast crossfades instead of flights; pulsing alerts become static highlights. No information is lost — every animated channel has a static equivalent.
Keyboard-only operation: full focus model; every operator-critical path (build, select, dispatch, time control, snapshot, lens) is reachable by keyboard (§3.5 table) with visible focus rings. Box-select has a keyboard marquee mode.
Camera comfort: transition speed cap + ease curves tuned against motion sickness; zoom acceleration limits; "return to last view" key; orbit disabled by default above Z3. Text remains legible at all strata (screen-space minimum sizes; density adapts rather than shrinks past floor).
Audio (optional): sparse, functional sonification only (scarcity alert, command ack, outage) — off by default in shared spaces, never required for comprehension.
8. Explicit Non-Goals
No multiplayer. Single operator, single session. No presence, chat, shared worlds, or competitive features. (Snapshot files may be shared out-of-band; the UI does not build collaboration.)
No fantasy gamification. No XP, levels, achievements, leaderboards, streaks, unlock trees, or narrative quests. Settlement P&L and reserve compliance are the only scores, and they are real. Scenario challenges (§5.2) are professional exercises, not game levels.
No mobile-native. Desktop web first (27"+, mouse+keyboard). Tablet is tolerant: pan/zoom/inspect must work via touch on a tablet in a pinch, but build/dispatch density is not redesigned for small screens. Phones: read-only status at most, and not in scope for v1.
No editing batsim physics from the UI. Device parameters, efficiency curves, degradation, and market models live in the simulator's registry and core (server-side, provenance-tagged). The UI reads the registry; it does not author it. No "tune the battery" sliders that lie about hardware.
No OEM cloud account management. The UI talks only to batsim's API; vendor-API mimicry views are read-only debugging surfaces, not credential managers.
No sub-second/frequency-domain visualization. ERCOT 5-minute settlement is the floor of economic relevance; the UI does not pretend to grid-stability physics the simulator explicitly excludes.
No speculative/marketing telemetry. Every rendered number traces to the API or is clearly labeled as a projection (revenue lens projected line). No vanity metrics.
9. Cross-References (normative hand-offs)
Part B (3D tech stack): LOD/crossfade machinery for the Zoom Continuum; particle/flow animation systems honoring P1/P6 and reduced-motion; camera rig per §2/§7.
Part C (data layer): API client contracts for registry, homes/fleets CRUD, /v1/dispatch (idempotency, per-home acks), /v1/sim time control, /v1/telemetry SSE/WebSocket with filtering/downsampling per stratum (§2 density column), scenarios, snapshots; the state store that keeps HUD, map, and panels consistent; dual-clock bookkeeping.
Part D (art & assets): vendor-faithful device models (Z5), home archetypes (Z3/Z4), Texas/zone cartography (Z1), palette tokens implementing §7, glyph iconography per §7 shape rules.
Part B — 3D Tech Architecture
Scope: the client-side technical architecture of the batsim 3D web UI. The UI is a
pure client of the batsim REST + SSE/WebSocket API (/v1/*). No game state,
simulation state, or fleet truth lives in the UI; the only permitted backend additions
are static asset hosting and an optional thin BFF (auth token injection, header
rewriting). Experience design (strata Z1–Z5, modes, controls) is Part A; data
stores, endpoint bindings, and stream protocols are Part C; art direction and
asset content are Part D. This part commits to a stack and specifies the rendering,
scaling, telemetry-binding, performance, asset, and build architecture.
B.1 Stack decision
B.1.1 Decision (committed)
Table
Layer   Choice   Version major   Role
Language   TypeScript   5.x, strict: true, noUncheckedIndexedAccess   whole codebase
App framework   React   18.x (18.3; upgrade to 19 when r3f v9 stable)   HUD, panels, routing
Build   Vite   5.x   dev server, bundling, workers
3D renderer   three.js via @react-three/fiber + @react-three/drei   three r16x, r3f 8.x, drei 9.x   Z3–Z5 world, instancing, custom shaders
Post-processing   @react-three/postprocessing (wraps postprocessing)   2.x / 6.x   bloom, SMAA/FXAA, vignette, SSAO (quality-gated)
Geospatial base map   MapLibre GL JS   4.x   Z1–Z2 Texas/ERCOT map, zone polygons, price heatmap
UI state   zustand   4.x (5.x compatible)   mode state, camera state, selection; data stores per Part C
Worker comms   comlink   4.x   telemetry decode workers
Generated API client   orval (or kubb) from batsim OpenAPI   latest   packages/batsim-client
Test   vitest 2.x, playwright 1.4x, react-three-test-renderer   —   §B.8
Toolchain   Node 20 LTS, pnpm 9.x workspaces, turborepo (optional, only if build times demand)   —   —
B.1.2 Rationale and rejected alternatives
Why not Unity/Unreal web export. Three disqualifiers: (1) iteration speed — the
UI is a data-bound engineering tool that evolves with the batsim OpenAPI; a TS codebase
with HMR, generated clients, and React HUD ships in minutes, Unity WebGL builds in tens
of minutes with multi-hundred-MB artifacts; (2) API integration — orval generates a
fully-typed client from /v1 OpenAPI in CI; in Unity the equivalent is hand-maintained
C# DTOs; (3) hiring/ownership — React + three.js talent is an order of magnitude
larger than Unity-on-web talent, and the HUD (the majority of screen area per Part A)
is DOM anyway. Unreal pixel-streaming additionally violates the pure-client constraint
(it needs a GPU server per session).
Why not Babylon.js. Babylon is a fine engine — arguably better out-of-the-box
(scene optimizer, GUI lib, solid WebGPU story via WebGPUEngine). We still commit to
three.js + r3f for: ecosystem size (drei alone removes months of controls/loaders/
gizmos work), the declarative React scene graph matching our React HUD skills, and
@react-three/postprocessing. This is a commitment, not a dismissal: the rendering
code MUST be structured (§B.7 module boundaries) so the engine-specific surface stays
inside packages/world; if three.js ever blocks us, the state/camera/LOD logic ports.
Why not deck.gl as the only renderer. deck.gl excels at Z1–Z2 (it's built for
exactly "100k colored points on a map") and we will borrow its luma.gl math
conventions, but it cannot do Z4/Z5 (lit glTF cutaways, skeletal/detail scenes,
postprocessing). Two renderers are already forced on us by the map (§B.2); deck.gl
would make it three. Rejected.
Why react-three/fiber for an app that must avoid re-render storms. r3f's
reconciler is opt-in: imperative hot paths (instanced attribute writes, §B.4) run in
useFrame and direct three.js object mutation, never through React state. We treat
r3f as scene-graph construction and lifecycle management, not as the per-frame
update mechanism. Rule enforced by lint/code-review: no setState from
useFrame; no React state per entity.
WebGL2 now, WebGPU-ready. Target WebGL2 (three WebGLRenderer) for launch: it is
universally available on our supported matrix (§B.5.6) and every library above is
battle-tested on it. The WebGPU path is three.js WebGPURenderer + TSL, currently
stabilizing; r3f supports it behind a renderer factory. To keep that door open:
All custom shaders are isolated in packages/world/src/shaders/ with a thin
material-factory layer (createSocRingMaterial(), createFlowMaterial()); no
inline ShaderMaterial strings scattered through components.
No use of deprecated fixed-function idioms; instancing via InstancedMesh /
InstancedBufferAttribute (maps 1:1 to WebGPU).
The WorldRenderer constructor accepts a RendererBackend = 'webgl2' | 'webgpu'
flag (env-configurable, §B.9) so WebGPU can be A/B'd without refactors.
B.1.3 Repo layout (pnpm monorepo)
plain
batsim-ui/
├── package.json                  # pnpm workspace root, engines: node >=20, pnpm 9
├── pnpm-workspace.yaml
├── tsconfig.base.json
├── apps/
│   └── web/                      # Vite + React app shell; routing; HUD; mode controllers
│       ├── index.html
│       ├── vite.config.ts
│       └── src/
│           ├── main.tsx
│           ├── app/              # shell, error boundaries, boot sequence
│           ├── hud/              # React DOM HUD (panels, timeline, KPIs) — NOT canvas
│           ├── modes/            # build/inspect/dispatch/scenario controllers (§B.7.3)
│           └── styles/
├── packages/
│   ├── batsim-client/            # GENERATED by orval from batsim /v1 OpenAPI (checked in,
│   │                             # regenerated by CI on spec change; never hand-edited)
│   ├── world/                    # everything three.js: renderer, camera rig, LOD system,
│   │   ├── src/renderer/         #   chunk streaming, instancing pools, picking, effects
│   │   ├── src/camera/           # mercator math, CameraRig, transition controller (§B.2)
│   │   ├── src/lod/              # LOD chain, instanced pools (§B.3)
│   │   ├── src/chunks/           # neighborhood streaming, LRU (§B.3.4)
│   │   ├── src/procgen/          # deterministic neighborhood generator (§B.3.5)
│   │   ├── src/telemetry/        # attribute-buffer writers, interpolation (§B.4)
│   │   ├── src/shaders/          # all GLSL, one file per material
│   │   └── src/picking/          # GPU color-id picking (§B.5.4)
│   ├── state/                    # zustand stores, telemetry ingest, replay; the ONLY
│   │                             # importer of batsim-client besides the BFF config.
│   │                             # Store shapes owned by Part C; this package implements them.
│   ├── ui-kit/                   # design tokens (palette per Part D), HUD primitives,
│   │                             # virtualized lists, timeline control
│   └── assets/                   # built glTF/KTX2 output + manifest.json (from tools/)
├── tools/
│   ├── asset-pipeline/           # Blender headless export scripts, gltfpack/toktx configs
│   ├── perf-harness/             # playwright perf runner, frame-time tracing (§B.5.7)
│   └── map-style/                # MapLibre style JSON + ERCOT zone overlays source
└── docker/
    ├── Dockerfile                # multi-stage → nginx static (§B.9)
    └── nginx.conf
Dependency rules (enforced with eslint import/no-restricted-paths or nx boundaries):
web → world, state, ui-kit, batsim-client; world → (nothing app-level; receives data
through typed ports); state → batsim-client; ui-kit → nothing; batsim-client →
nothing. world MUST NOT import batsim-client — it consumes typed buffer contracts
(§B.4.1), which is what keeps the offline/demo mode (Part C) and the replay tool
(§B.7.5) trivial.
B.2 The hybrid map/3D problem
The single hardest technical problem: strata Z1–Z2 need real cartography (Texas
geometry, ERCOT load zones LZ_HOUSTON / LZ_NORTH / LZ_SOUTH / LZ_WEST, hub markers
HB_NORTH etc., price heatmap, readable labels) while Z3–Z5 need a free 3D world
(lit houses, cutaways, camera at ground level). Part A defines the zoom continuum as
seamless; the seam must therefore be engineered, not designed, away.
B.2.1 Options considered
(a) Custom globe-less terrain in three.js with raster map tiles draped on a height
plane. One renderer, full control. Rejected: at Z1 the view spans ~1,200 km; raster
tiles draped on terrain give no vector labels, no client-side zone styling, blurry
heatmap at oblique pitch, and we would re-implement tile pyramid management, label
placement, and collision — months of work for a worse map than MapLibre gives free.
(b) MapLibre GL JS owns Z1–Z2; three.js owns Z3–Z5; a shared camera-math handoff
crossfades between them in a transition band. Two canvases, one authoritative
camera. Risk: the seam (double render cost during transition, projection mismatch
visible as sliding). Mitigations are specified below (§B.2.4–B.2.6) and are bounded,
testable math.
(c) Everything in three.js, including vector tiles rendered by us. Rejected for
launch: we would rebuild a vector-tile renderer (MVT parse → tessellate → label
SDF placement) — that is a product in itself. Keep as a documented P2 escape hatch:
because the camera math (§B.2.5) is projection-agnostic, replacing the MapLibre layer
with an in-three.js tile renderer later changes only the Z1–Z2 backend, not the
continuum.
B.2.2 Decision
Commit to (b). Justification: time-to-quality. MapLibre is the best-in-class open
vector map and covers 100% of Z1–Z2 requirements (zone polygons as fill layers, price
heatmap as a data-driven fill/heatmap layer over ERCOT zone GeoJSON, 10k home markers
as a clustered symbol/circle layer or a GL custom layer). The three.js world only has
to be good from ~2 km altitude downward, where cartography is irrelevant. The seam is
a solved class of problem (Mapbox's own three.js examples, deck.gl/MapLibre
interleaving) and our crossfade band (§B.2.4) tolerates residual projection error.
Stratum ↔ zoom mapping (MapLibre zoom z_map; three.js scene altitude h in meters
above local ground; authoritative thresholds live in packages/world/src/camera/strata.ts):
Table
Stratum (Part A)   Owner   z_map range   scene altitude h   Notes
Z1 state/grid   MapLibre   4.5 – 9.0   —   zones + heatmap + aggregated fleet glyphs
Z2 city/county   MapLibre   9.0 – 13.0   —   instanced home dots via MapLibre custom WebGL layer (§B.3.2)
Transition band   both   13.0 – 14.5   ≈ 1500 m → 350 m   dual render + crossfade (§B.2.4)
Z3 neighborhood   three.js   —   350 m – 60 m   chunked procgen neighborhoods
Z4 house   three.js   —   60 m – 8 m   detailed house glTF
Z5 battery cutaway   three.js   —   < 8 m   hero cutaway scene (Part D)
B.2.3 Coordinate systems
Three frames, all conversions in packages/world/src/camera/mercator.ts, unit-tested
to round-trip error < 1e-6 (§B.8.1):
Geodetic: WGS84 (lng, lat) — what batsim home placements and ERCOT zone
GeoJSON use.
Web Mercator (EPSG:3857): meters, (mx, my) = R·(λ, ln tan(π/4 + φ/2)),
R = 6,378,137. MapLibre's internal plane.
Scene frame (ENU, three.js Y-up): a local tangent plane anchored at a scene
anchor A = (lng0, lat0) (the centroid of the currently loaded neighborhood
chunk cluster). Scene units are true meters: 1 unit = 1 m.
Conversion:
plain
(mx, my)   = mercator(lng, lat)
east_m     = (mx - mercator(lng0,lat0).x) / k        # k = 1/cos(lat0): mercator scale
north_m    = (my - mercator(lng0,lat0).y) / k        #   distortion at anchor latitude
scene_pos  = (east_m, altitude_m, -north_m)          # three.js: +X east, +Y up, -Z north
At ERCOT latitudes (26°N–36.5°N) k ∈ [1.026, 1.244]; ignoring k would stretch
neighborhoods up to 24% — the correction is mandatory and covered by unit tests.
Anchors are re-based when the camera focus moves > 25 km from A (Z3+ only;
re-basing is a chunk-manager concern, §B.3.4, and is invisible because it happens
while no chunk straddles the old/new boundary at Z4/Z5 detail).
B.2.4 Transition band mechanics (z_map 13.0 → 14.5)
During the band, both canvases render and are composited by CSS (map canvas
under, world canvas over, opacity crossfaded on an easing curve of normalized band
progress u = smoothstep(13.0, 14.5, z_effective)). One authoritative CameraRig
(§B.7.2) owns the logical camera (focus_lnglat, h, pitch, bearing); per frame it:
writes MapLibre via map.jumpTo({center, zoom, pitch, bearing}) (MapLibre is a
slave in the band; outside the band at Z1–Z2 it owns its own interaction and
publishes logical camera changes back to the rig);
writes the three.js camera via the math in §B.2.5.
The three.js scene during the band renders the chunk set for the neighborhood under
the focus with fog (per Part D palette) fully opaque at band entry thinning to zero
at band exit, which masks pop-in and hides the far clip plane. Budget note: the band
is the worst-case frame cost (map + full world); the perf gate (§B.5.7) tests the
band explicitly, and quality auto-scaling (§B.5.5) is allowed to drop dynamic
resolution only inside the band before degrading anything else.
B.2.5 Camera unification math
Map → scene. Given logical camera (lng, lat, h, pitch p, bearing b):
plain
eye    = enu(lng, lat) + (0, h, 0)                        # §B.2.3
f_h    = normalize( ( sin b, 0, -cos b ) )                # horizontal forward, b cw from north
fwd    = f_h · cos p + (0, -1, 0) · sin p                 # p: 0 = horizon … 90° = nadir
camera.position = eye
camera.quaternion = lookAtMatrix(eye, eye + fwd, up=(0,1,0))
camera.fov = MAP_FOV_DEG (= 30, matched to MapLibre default fov; MUST be a shared constant)
camera.near = max(0.5, h/2000);  camera.far = max(h*20, 5000)
MapLibre altitude↔zoom equivalence (for publishing z_map from a scene camera and
for matching scale across the seam): with 512 px tiles,
metersPerPixel(z, φ) = 40075016.686 · cos φ / (512 · 2^z), and a viewport of height
H_px with vertical fov θ sees ground height 2·h·tan(θ/2); equating gives
plain
z_map = log2( 40075016.686 · cos φ · H_px / (1024 · h · tan(θ/2)) )
Scene → map. Inverting: from a three.js camera at Z3+, ray-cast fwd onto the
ground plane y=0 to get the focus point; h = eye.y; convert focus ENU → lng/lat
(inverse mercator + re-apply k); recover b from atan2(f_h.x, -f_h.z), p from
asin(-fwd.y), z_map from the formula above. Both directions are implemented as
pure functions logicalToSceneCamera() / sceneToLogicalCamera() with property-based
tests (fast-check) asserting round-trip |Δh|/h < 1e-4, |Δb| < 1e-4 rad.
B.2.6 Known residual risks (honest list)
Label/road sliding at band entry: MapLibre labels fade as the world fades in;
we keep MapLibre's default label fade and start world fade-in
only at u > 0.25, so labels never visibly fight world geometry. Verified by the
Z-band visual regression test (§B.8.4).
MapLibre terrain/3D-buildings: disabled at launch (flat map). If Part D later
wants 3D buildings at Z2, they render in the MapLibre custom layer, not three.js.
Device pixel ratio mismatch between the two canvases: both are forced to the
same effective DPR by the dynamic-resolution controller (§B.5.5).
WebGL context count: two contexts + picking RT is fine (limit ~8–16/browser);
we never create more than these.
B.3 Scale architecture for 10,000 homes
Reference load: 10,000 homes (batsim /v1/homes), each with 1 battery system from the
registry (tesla-powerwall-3, enphase-iq-5p, solaredge, sonnen — per Part A registry),
distributed across ERCOT zones. Rendering must never be O(10k) in draw calls.
B.3.1 LOD chain
Each home carries a 5-stage LOD chain selected by screen-space size (projected
bounding-sphere radius in pixels) with hysteresis (±15%) to prevent LOD flicker:
Table
LOD   Strata   Representation   Tris/instance   Storage   Transition
L0 glyph   Z1   point sprite in MapLibre custom layer; color = SOC, size = kW   2 (quad)   1 Float32Array attribute block   —
L1 block   Z2–Z3 far   instanced low-poly massing block (6 archetype variants)   60–120   InstancedMesh per archetype   alpha dither at 40 px
L2 house   Z3 near–Z4   instanced detailed house glTF per load archetype (Part D), roof/street variation via per-instance UV offset into trim atlas   2,000–4,000   InstancedMesh × archetype (≤8)   200 px
L3 hero   Z4 near   unique (non-instanced) hero house w/ interior shell, animated flow pipes   25,000–60,000   glTF, ≤6 concurrent heroes   600 px, crossfade
L4 cutaway   Z5   battery unit cutaway scene per OEM model (Part D), animated cells/BMS LEDs   50,000–150,000   glTF + custom shaders   camera-driven at h<8m
Supporting geometry (streets, parcels, trees, poles, transformers) is chunk-merged
(static BufferGeometry merge per 256 m chunk, §B.3.4), never per-instance.
B.3.2 Z1–Z2: instanced markers inside MapLibre
A MapLibre custom style layer (render(gl, args) with maplibregl.CustomLayerInterface)
draws all 10k homes as one instanced quad draw using a mat4 from
args.defaultProjectionData (MapLibre 4.x globe/projection API) so glyphs track the
map exactly. Per-instance attributes: position (mercator), SOC (→ palette-ramp color
per Part D), dispatch state (→ ring pulse phase), size (→ kW capacity). Attribute
buffers are the same Float32Array ring buffers written by the telemetry ingest
(§B.4.2) — zero-copy handoff between strata. Clustered aggregates (fleet KPI per
zone) are ordinary MapLibre fill/symbol layers driven by Part C stores at ≤1 Hz.
B.3.3 Z3–Z5: instancing pools, culling, distance tiers
InstancedPool per (archetype × LOD): preallocated InstancedMesh with capacity
from the chunk manager's visibility estimate; per-frame, the LOD system compacts
the visible set into pool slots (swap-remove), writes instanceMatrix for moved
or newly visible instances only (static homes: matrix written once at chunk load).
Frustum culling: chunk-level (bounding box vs frustum) then three's built-in
per-InstancedMesh culling; we do not pay per-instance CPU culling — overdraw at
L1/L2 is cheaper than 10k sphere tests.
Distance tiers for animation: active (h-visible & within 500 m: full
telemetry-driven animation, flow particles), warm (visible: attribute-driven
shader animation only, no CPU), cold (off-screen: data updated in Part C stores,
zero render work). Counts per frame are surfaced to the perf HUD.
Picking: §B.5.4 GPU color-id pass; instance id → home_id via chunk-local table.
B.3.4 Neighborhood streaming (chunks)
The world is partitioned into 256 m × 256 m chunks keyed by web-mercator tile at
fixed level (quadkey level 17 ≈ 305 m at lat 30°, snapped to 256 m scene grid —
ChunkId = (level, x, y)). The ChunkManager:
computes the required chunk set from the camera frustum + look-ahead in the
velocity direction (prefetch radius 1 chunk at Z3);
loads chunk payloads asynchronously: static geometry (procgen output, §B.3.5) from
a Worker, home roster + initial attribute state from Part C's snapshot store —
never a per-home REST fetch; roster comes from the /v1/homes page already in
the store (Part C) filtered by chunk bounds via the spatial index (§B.4.4);
enforces an LRU budget: 96 loaded chunks max (≈ 25k typical homes covered only
when dense; Texas suburbs ≈ 40–120 homes/chunk → 96 chunks ≈ 4k–12k instanced
slots, within pools). Eviction disposes geometries via a shared Disposer that
also returns pool capacity; eviction is deferred 5 s to survive camera oscillation;
all chunk build work happens in a procgen Worker (§B.4.4) transferring
ArrayBuffers back; main thread only creates GPU objects.
B.3.5 Deterministic procedural neighborhoods
Chunk content is procedurally generated, not stored: street graph (grid/curvilinear
variants per region class), parcel split, house placement/orientation, vegetation,
props. Generator contract (packages/world/src/procgen/):
plain
seed  = splitmix64( master_seed  XOR  hash64(chunk_id) )          # chunk-level
home  = splitmix64( seed  XOR  hash64(home_id) )                  # per-home stream
PRNG: splitmix64 + xorshift128+ (portable, exact across JS/WASM; no
Math.random anywhere in procgen — lint-enforced). master_seed comes from the
batsim scenario/snapshot id (Part C), so a given fleet always produces the identical
neighborhood on any machine, any session — required by Part D's art spec (archetype
placement consistency), by visual regression tests (§B.8.4), and by replay (§B.7.5).
Home visual archetype binds to the batsim load archetype; battery yard/garage
placement binds to the registry model (Part D mapping table). Versioned:
PROCGEN_VERSION is part of the seed; a version bump intentionally re-lays-out the
world and invalidates visual baselines.
B.3.6 Draw-call & triangle budget (per zoom stratum, 10k-home scenario)
Table
Stratum   Draw calls (max)   Triangles (max)   What dominates
Z1   ≤ 30   ≤ 150 k   map fills + 10k glyphs (1 call) + heatmap
Z2   ≤ 60   ≤ 400 k   glyphs + zone fills + labels
Transition band   ≤ 350 (both renderers)   ≤ 2.0 M   dual render; dynamic-res allowed
Z3   ≤ 220   ≤ 2.5 M   ≤96 chunks merged statics (~60 calls) + L1/L2 instanced pools (~16 calls) + sky/fog
Z4   ≤ 260   ≤ 3.0 M   ≤6 heroes (150 calls w/ interiors) + L2 pool + near chunks
Z5   ≤ 200   ≤ 1.5 M   cutaway hero + postprocessing passes
Budgets are gates: the perf harness (§B.5.7) fails CI if any stratum exceeds its
column by >10% on the reference scene.
B.4 Telemetry-driven rendering
Data provenance, store shapes, and stream protocols are Part C. This section
specifies the render-side contract that lets 1 s telemetry (batsim dt = 1 s ticks)
and 5-minute settlement aggregates drive visuals without React re-render storms.
B.4.1 The buffer contract (world ⇄ state boundary)
packages/state owns decoded telemetry; packages/world receives only these
typed objects (no JSON, no class instances crossing per frame):
plain
TelemetryFrame { t_sim: f64, n: u32, home_ids: Uint32Array(view of id→slot table),
                 soc: Float32Array, p_batt: Float32Array, p_pv: Float32Array,
                 p_load: Float32Array, state_flags: Uint8Array }   // transferables
SettlementFrame { t_interval, per-home interval-end SOC + avg powers, zone aggregates }
Slot indirection: each home owns a stable slot (0..n-1) for its lifetime in the
store; all render attribute buffers are indexed by slot, so telemetry writes are
memcpy, not lookups. home_id → slot table lives in the store; slot → instance
coords tables live in chunks.
B.4.2 Writing visuals without re-renders
SOC ring / glyph color: InstancedBufferAttribute socColor (n×3) +
socLevel (n×1); telemetry ingest writes changed slots into the underlying
Float32Array and marks attribute.addUpdateRange(start, count) /
needsUpdate — three.js uploads only dirty ranges. Zero React involvement.
Power flow speed/direction: encoded as per-instance float flow (signed kW,
normalized); consumed by a shader uniform-attribute pair that scrolls a flow
texture along conduit splines at L3/L4 and modulates glyph pulse at L0/L1. Speed
mapping and particle styling per Part D; the mechanism is one float per home
per frame.
State flags (outage, curtailment, dispatch-active bitmask) → shader-driven
effects (flash, desaturation) via a Uint8Array texture; no geometry swaps.
Interpolation: batsim emits 1 s telemetry frames and 300 s settlement
aggregates. Rendering never snaps: each animated attribute keeps
(prev, next, t_prev) and the shader lerps by a per-frame uniform
u_interp = clamp((t_render - t_prev)/dt_frame) (Hermite for SOC so interval-end
derivative discontinuities don't pump; linear for powers). CPU writes once per
telemetry frame (≤1 Hz per home), GPU interpolates 60×/s. This is the core
"no re-render storm" mechanism: React re-renders only for HUD state changes;
the 3D world is updated by buffer writes and uniforms.
Time scrub / playback: the store keeps a per-home ring buffer
(Float32Array, stride 5: soc,p_batt,p_pv,p_load,flags) at 5 s decimation for
the last 2 h of virtual time: 10k homes × 1,440 samples × 5 × 4 B ≈ 288 MB
(inside the §B.5.3 memory budget; ring capacity is quality-scaled). Scrubbing
seeks the ring and re-emits synthetic TelemetryFrames through the identical
§B.4.2 path — visuals cannot tell live from scrub. Scrubbing beyond the local
horizon triggers Part C's batch fetch (/v1/telemetry/homes/{id}/series /
fleet series) into a scrub cache; the render path is unchanged. Time control
semantics (pause, 1×/N×, run-until-interval per batsim's
realtime/fast_forward/run_until modes) are Part C; the renderer only consumes
the resulting frame stream.
B.4.3 Update ordering per frame (main thread)
plain
1. ingest:  adopt decoded transferable buffers from worker (postMessage, zero-copy)
2. store:   commit to zustand (single batched update per frame, transient subs only)
3. world:   LOD select → pool compaction → attribute dirty-range writes → uniforms
4. hud:     React renders only what subscribed selectors say changed (≤1 Hz typical)
B.4.4 Workers and optional WASM
Telemetry decode worker (packages/state/src/workers/decode.ts, comlink):
receives raw WS/SSE payloads (Part C), decodes JSON/MsgPack into the
TelemetryFrame typed arrays above, transfers buffers back (transferable —
no structured clone). Main-thread ingest cost target ≤ 2 ms (§B.5.2).
Procgen worker (§B.3.4/§B.3.5): builds chunk geometry off-thread.
Spatial index: home positions are static per scenario; a flat
Flatbush-style packed R-tree (or a small Rust→WASM port of rstar if we need
dynamic insertion) is built once per scenario in the decode worker and used for
chunk roster queries and map-side picking hit-tests. WASM is optional behind the
SpatialIndex interface; ship JS first, add WASM only if profiling justifies.
B.5 Performance engineering
B.5.1 Targets
Table
Tier   Hardware reference   Target
Discrete   laptop RTX 3060-class, 1080p, DPR ≤ 2   60 fps (p95 frame ≤ 16.6 ms) at every stratum incl. transition band
Integrated   Iris Xe / M1 iGPU, 1080p   30 fps (p95 ≤ 33 ms), quality auto-scaled (§B.5.5)
B.5.2 Frame budget (16.6 ms, discrete tier)
Table
Slice   Budget   Contents
Sim-data ingest   ≤ 2.0 ms   adopt transferables, store commit, dirty-range marks
Scene update   ≤ 4.0 ms   LOD select, pool compaction, chunk manager, camera rig, uniforms
Draw (CPU submit)   ≤ 8.0 ms   three.js render incl. postprocessing chain
Slack/GC/browser   ≥ 2.6 ms   compositor, events, GC headroom
GPU frame at Z3 reference scene must be ≤ 10 ms on the discrete tier (measured via
EXT_disjoint_timer_query_webgl2 where available, else inferred from p95).
B.5.3 Memory budget (10k-home scenario, 24 h session): < 2 GB total
Table
Bucket   Budget   Notes
Geometry (glTF + procgen chunks)   ≤ 350 MB   LRU-enforced; shared archetype geometry
Textures (KTX2 atlases, heroes)   ≤ 400 MB   §B.6 sizes enforced in pipeline CI
Instance/attribute buffers   ≤ 120 MB   pools + telemetry attributes
Telemetry rings + scrub cache   ≤ 350 MB   §B.4.2 ring + fetch cache
JS heap (React, stores, map)   ≤ 500 MB   leak-tested: 1 h soak, heap Δ ≤ 5%
Headroom   ~280 MB   browser overhead
B.5.4 three.js-specific engineering rules (binding)
Merged/instanced geometry only for anything > 20 occurrences; shared
Material instances via a material registry keyed by shader+flags (never
per-object .clone()); texture atlases for procgen facades/trim (Part D atlas
spec), one 4k + one 2k atlas for all L1/L2 content.
Zero per-frame allocation in hot paths: no new Vector3/Matrix4/Color inside
useFrame or LOD/pool loops — module-scope temp objects; array pooling in the
compaction pass. Verified by the perf harness allocation counter
(performance.memory sampling + a dev-only proxy allocator that throws).
GPU color-id picking: on pointer click (not per frame), render the pick
scene — instanced meshes with a flat color-id material — into a ¼-res
WebGLRenderTarget, readRenderTargetPixels a 3×3 neighborhood, decode
(chunk, instance) → home_id. CPU raycasting against 10k instances is forbidden.
frustumCulled left on with correct bounding spheres; matrixAutoUpdate=false
on all static objects; powerPreference: 'high-performance'; stencil off,
logarithmic depth off (we manage near/far per stratum, §B.2.5).
Postprocessing chain is quality-gated: SMAA→FXAA→off, SSAO and bloom
independently toggleable; the chain is a single EffectComposer from
@react-three/postprocessing (one geometry pass).
B.5.5 Quality auto-scaling
A governor samples EMA frame time over 60 frames and steps a 5-level quality ladder
(dynamic resolution 1.0→0.6 ×DPR first — including MapLibre's pixel ratio during the
transition band — then particle counts, then shadows, then SSAO/bloom, then chunk
prefetch radius). Hysteresis: ≥10 s between level changes; user override in settings
pins the ladder. Level changes are announced to the HUD (Part A "performance mode"
indicator).
B.5.6 Browser support
Chrome/Edge 110+, Firefox 115+, Safari 16.4+ (WebGL2 floor). No IE, no WebGL1
fallback (we detect and show a blocking unsupported-browser panel). Mobile/tablet:
out of scope for P0 (Part A non-goal), but the renderer must not crash on mobile —
degrade to a read-only Z1/Z2 map.
B.5.7 Perf harness & CI regression gates
tools/perf-harness: playwright drives headless Chrome (GPU-enabled runner;
SwiftShader only for smoke, never for gates) through scripted camera tours —
z1_overview → band_crossing → z3_flythrough → z4_hero → z5_cutaway → scrub_2h —
with window.__perf hooks (exposed by packages/world in perf builds) recording
per-frame times, draw calls (renderer.info), and heap. Outputs JSON + traced
waterfalls. CI gates (fail the build): p95 frame time per stratum vs §B.5.1;
draw-call/tri budgets per §B.3.6; initial-load time-to-interactive ≤ 4 s on throttled
CI network (§B.6.4); heap growth ≤ 5% over the 20-min soak scenario; any gate
regression > 10% vs main-branch baseline.
B.6 Asset pipeline
B.6.1 Formats and compression
Geometry: glTF 2.0 binary (.glb) with meshopt (EXT_meshopt_compression)
as the default for all world content — decode is ~10× faster than Draco and the
three.js MeshoptDecoder integrates cleanly; Draco is permitted only for the
Z5 hero cutaways where ratio beats decode cost (battery internals are heavy,
decoded once). Pipeline CI asserts every .glb carries the declared extension
and that KHR_texture_transform, KHR_materials_* usage matches the world
material registry.
Textures: KTX2/Basis Universal — UASTC for normal maps and hero albedo
(quality), ETC1S for atlases and props (size). Encoded with toktx/basisu;
pipeline CI enforces max texture dimensions (atlas ≤ 4096, hero ≤ 2048,
props ≤ 1024) and mip completeness.
Pipeline chain per source asset: Blender 4.x headless export script
(tools/asset-pipeline/export_*.py, deterministic: fixed unit scale 1 unit = 1 m,
Y-up, applied transforms, named-node conventions from Part D) → gltfpack
(meshoptimizer CLI: quantize, weld, simplify LODs, meshopt) → toktx → emit
into packages/assets/ with content-hashed filenames.
B.6.2 Manifest and lazy loading
packages/assets/manifest.json (build-generated) maps logical asset ids
(house/ suburban-ranch/l2, battery/tesla-powerwall-3/cutaway, atlas/facades-01)
→ {url, hash, bytes, dependencies[]}. Loading policy:
Boot bundle (Z1–Z2 interactive): core JS + L0 glyph sheet + ERCOT zone
GeoJSON + fonts/HUD → hard budget < 15 MB total to interactive at Z1 on the
CI-throttled profile (§B.5.7): ~3.5 MB gz JS (app+world+state, three.js
tree-shaken), ~1 MB gz map style/zones, ~1 MB glyphs/UI, remainder headroom —
map tiles themselves stream from the tile host and are excluded but counted in
a separate ≤ 8 MB "first minute" budget.
Stratum bundles: z3_core (L1/L2 houses + atlases, ~12 MB), z4_heroes
(~20 MB), z5_cutaways per-OEM (4–10 MB each) — prefetched on camera approach
(band entry → z3_core; first L3 selection → z4_heroes; Z5 entry → the one
OEM cutaway needed), all behind AssetManager with in-flight dedup, priority
queue, and Cache-Control: immutable content-hashed URLs.
Everything loads through drei useGLTF-compatible async boundaries with
Suspense fallbacks that are world-consistent (placeholder massing blocks, not
spinners, per Part A/Part D).
B.7 App architecture
B.7.1 Module boundaries
plain
┌────────────────────────────────────────────────────────────┐
│ HUD (React DOM overlay — panels, timeline, KPIs, toasts)    │  apps/web/hud, ui-kit
│  ├─ never renders inside canvas; text/ARIA/virtualized lists│
├────────────────────────────────────────────────────────────┤
│ Mode controllers (build / inspect / dispatch / scenario)    │  apps/web/modes
│  ├─ consume InputIntents, drive store actions (Part C)      │
├────────────────────────────────────────────────────────────┤
│ CameraRig (authoritative logical camera + strata machine)   │  world/camera
├────────────────────────────────────────────────────────────┤
│ WorldRenderer (three.js: LOD, chunks, instancing, picking,  │  world/*
│  effects)  ◄── TelemetryFrame/SettlementFrame buffers ◄──┐  │
├──────────────────────────────────────────────────────────│──┤
│ MapLayer (MapLibre + custom glyph layer)                 │  │  world/map + tools/map-style
├──────────────────────────────────────────────────────────│──┤
│ State stores (zustand) — shapes owned by Part C ─────────┘  │  state/*
│  batsim-client (generated OpenAPI) ◄── /v1 REST + WS/SSE    │  batsim-client
└────────────────────────────────────────────────────────────┘
B.7.2 CameraRig
Single source of truth for (focus, h, pitch, bearing) + stratum state machine
(Z1…Z5 + band). Owns input-derived camera goals, critically-damped smoothing
(fixed-timestep integrator, §B.7.5 determinism), collision floor (never below
ground + 0.5 m), and the MapLibre↔three.js slave publishing of §B.2.4. Mode
controllers request camera moves (rig.flyTo(home_id)) but never write cameras
directly — this is what makes seamless zoom, transition crossfade, and replay all
consistent.
B.7.3 Mode controllers
One class per Part A mode (BuildMode, InspectMode, DispatchMode,
ScenarioMode), with lifecycle enter/exit/handleIntent(frame). Modes translate
intents into store actions (place home → Part C POST /v1/homes; draw dispatch
box-select → POST /v1/dispatch/commands; etc.) and into world overlays (ghost
preview mesh in build mode, selection brackets in dispatch mode) — overlays are
ephemeral scene children owned by the mode, disposed on exit. Optimistic UI vs
server truth is Part C's policy; modes render whatever the stores say.
B.7.4 Input system abstraction
Raw DOM/pointer/wheel/keyboard events are normalized by an InputMapper into
semantic intents: pan, zoom(delta, anchor), select(point|box),
dragBegin/Move/End, hotkey(action), contextMenu. Mode controllers and the
CameraRig subscribe to intents, never to DOM events — enabling: (1) consistent
behavior across mouse/trackpad/keyboard (Part A controls reference), (2) synthetic
intent injection for e2e tests and replay, (3) future gamepad/touch without touching
modes. Intent stream is timestamped against the render clock and logged by the
recorder (§B.7.5).
B.7.5 Deterministic replay (dev tool)
packages/state/src/replay/ records a session log:
{ master_seed, procgen_version, batsim snapshot id, intent stream (t, intent),
telemetry frame digests, store-action stream }. Replay mode: fixed-timestep driver
(120 Hz logical) feeds recorded intents + recorded telemetry through the real
stores and world; rendering is free-running but all state/camera evolution is
deterministic (camera integrator uses fixed dt; procgen seeded, §B.3.5; no wall-clock
reads outside the driver — enforced by a Clock interface injected everywhere,
lint-forbidding Date.now/performance.now in world/state except the driver).
Uses: bug reports ("attach replay.json"), perf tours (§B.5.7), visual regression
(§B.8.4). Honest limit: GPU-level frame output is not bit-deterministic across
drivers; determinism is claimed for state and camera, with pixel-diff tolerances
in tests.
B.7.6 Error handling
React error boundaries per HUD panel and one around the world canvas (WebGL context
loss: show non-blocking "graphics reset" banner, attempt restoreContext, re-upload
buffers from stores — the §B.4.1 buffer contract makes re-upload a pure function of
store state). batsim API failures surface per Part C's policy (retry/backoff/
offline banner); world never sees them.
B.7.7 Offline / demo hook point
TelemetrySource interface in packages/state:
LiveSource (WS /v1/telemetry/ws + REST bootstrap) vs ReplaySource (bundled
recorded trace). Selected by env config (§B.9.3) or when health-check
(GET /v1/system/health) fails at boot. Everything downstream — buffers, scrub,
replay, world — is source-agnostic. Details and the demo dataset are Part C.
B.8 Testing & quality
B.8.1 Unit (vitest)
Mercator/ENU round-trips and camera unification (§B.2.3–B.2.5, property-based with
fast-check), LOD selection incl. hysteresis, chunk-set computation and LRU eviction,
procgen determinism (same seed ⇒ byte-identical geometry buffers), ring-buffer and
interpolation math (§B.4.2), quality governor ladder transitions. Target: ≥ 90%
line coverage on world/src/camera, world/src/lod, state/src/* pure modules;
coverage is not gated on React components (covered by e2e instead).
B.8.2 Scene-graph tests (react-three-test-renderer)
Chunk mounts produce expected instance counts per pool; LOD swaps at scripted
altitudes; picking decode maps synthetic color-ids back to home_ids; overlay
lifecycle on mode enter/exit; context-loss recovery remounts buffers. These run
headless without WebGL (mock renderer) and assert structure, not pixels.
B.8.3 E2E golden paths (playwright)
Scripted against a real batsim container (docker-compose in CI) with a seeded
1,000-home scenario:
Build: enter build mode → place 3 homes at Z3 → assert POST /v1/homes
payloads and world instance-count deltas.
Dispatch: Z2 box-select a neighborhood → issue dispatch → assert
POST /v1/dispatch/commands and the dispatch-active flag reaches glyph
attributes within 2 s.
Time: accelerate to N×, pause, run_until next 5-min boundary, scrub back
30 min — assert timeline/store/world SOC attributes agree with
/v1/telemetry/homes/{id}/series ground truth (tolerance = one tick).
Zoom continuum: scripted Z1→Z5→Z1 tour asserts stratum transitions, band
crossfade, and no console errors.
B.8.4 Visual regression
Playwright screenshots at fixed camera keyframes per stratum (Z1 ERCOT overview,
mid-band, Z3 over chunk 12×40, Z4 hero, Z5 powerwall-3 cutaway) with replay-driven
deterministic state, fixed DPR=1, animations frozen at t=10 s. Diff via pixelmatch
with per-stratum thresholds (Z1: 0.5% — map tile nondeterminism; Z3–Z5: 0.1%).
Baselines versioned with PROCGEN_VERSION.
B.8.5 Performance gate
§B.5.7 harness runs nightly (and on PRs touching world/, state/, assets/):
frame-time p95 per stratum, draw-call/tri budgets, TTIZ1, memory soak. Regression
10% vs baseline fails CI.
B.9 Build & deploy
B.9.1 Vite configuration notes
build.target: 'es2022', manualChunks: vendor-react, vendor-three
(three+r3f+drei+postprocessing — the big one, loaded only by the world entry),
vendor-maplibre (Z1–Z2 path), app shell. Mode controllers and stratum bundles
are dynamic import()s keyed by mode/stratum so a Z1 session never downloads
Z5 code (§B.6.2 budget depends on this; bundle-size CI enforces per-chunk caps).
Workers via Vite ?worker + comlink; WASM (optional spatial index) via
vite-plugin-wasm; GLSL via vite-plugin-glsl (compile-time minify, keeps
§B.1.2 shader isolation); KTX2/meshopt decoders copied as static assets with
content hashes.
Dev proxy: /v1 → batsim dev server (http://localhost:8080 default) so the
generated client works identically in dev and prod; SSE/WS proxied with
ws: true and no buffering.
B.9.2 Code splitting per mode
Each mode controller is a lazy chunk (React.lazy + route-level boundary); the
world renderer preloads stratum bundles per §B.6.2. Shared code (ui-kit, stores)
stays in the shell chunk to keep mode chunks < 150 KB gz each.
B.9.3 Environment configuration
Runtime env (injected via env.js generated at container start — not baked at
build, so one image serves all environments):
BATSIM_API_BASE_URL (e.g. https://batsim.acme.com/v1),
BATSIM_WS_URL (default ${BASE}/telemetry/ws),
MAP_STYLE_URL / tile host,
BFF_URL (optional; when set, the client calls the BFF which injects the auth
token to batsim — the UI never stores credentials),
OFFLINE_DEMO=1 → boot into ReplaySource (§B.7.7),
RENDERER_BACKEND=webgl2|webgpu (§B.1.2; default webgl2).
Boot sequence validates the server: GET /v1/system/version — UI declares a
compatible batsim API range; mismatch shows a blocking-but-informative panel.
B.9.4 Docker / hosting
Multi-stage docker/Dockerfile: node:20-alpine build (pnpm fetch → build) →
nginx:1.27-alpine serving dist/: brotli+gzip precompressed, Cache-Control:
immutable, max-age=31536000 on hashed assets, no-cache on index.html/env.js,
SPA fallback, COOP/COEP headers not set (SharedArrayBuffer not required at P0;
revisit if WASM threads land). Health endpoint serves a static 200 for k8s probes.
B.9.5 Versioning & release alongside batsim
UI is semver'd independently but pinned to a batsim API compatibility range
recorded in batsim-client/COMPAT.md (generated client is regenerated by CI
whenever the batsim OpenAPI changes; a type-breaking change fails UI CI the same
day — this is the contract that keeps "pure client" honest). Releases: container
tags batsim-ui:<ui-semver>+batsim.<api-min>; deploy matrix documents which UI
image pairs with which batsim release. Rollback = redeploy previous static image;
no state migration exists by design (all state lives in batsim).
B.10 Cross-part contract summary (what this part promises)
To Part A: seamless Z1–Z5 continuum via §B.2; all four modes implemented as
§B.7.3 controllers with §B.7.4 intents; time controls render through §B.4.2.
To Part C: the TelemetryFrame/SettlementFrame buffer contract (§B.4.1),
TelemetrySource hook (§B.7.7), generated-client ownership, and a strict
no-JSON-per-frame boundary.
To Part D: deterministic procgen placement (§B.3.5), atlas/KTX2/glTF
constraints (§B.6), palette-ramp SOC colors and flow shaders as the delivery
mechanism (§B.4.2), LOD visual definitions (§B.3.1).
Part C — Data Binding & batsim API Integration
Scope. This part specifies how the 3D web UI (React + three.js, Part B) binds to a
running batsim instance: generated API client, client-side state architecture, telemetry
ingestion, the dispatch command path, virtual-time synchronization, snapshot/savegame
flows, scenario and ERCOT price data, resilience/reconnect, security/configuration, and
the concrete TypeScript contracts that glue it together. It defines data flow and data
contracts only: zoom strata (Z1–Z5) and modes are Part A; renderers, worker pool
layout, and frame budgets are Part B; visual encodings (colors, particles, alerts) are
Part D. This part references those parts by contract, never duplicates them.
The UI is a pure client. It adds no backend of its own beyond a static-file host and
an optional dev proxy (§C.8). Every mutation of simulation truth goes over batsim's
published OpenAPI 3.1 surface at /v1; every piece of rendered state derives from
either (a) batsim responses/streams or (b) purely local, ephemeral UI state.
References to batsim's own spec (the server spec, spec_parts/part_c_api.md and
part_d_ercot.md) are cited as [server §C.x] / [server §D.x]. Endpoint names,
problem-type registry, retention tiers, and idempotency semantics below are normative
from the server spec; this document only decides how the client consumes them.
Out of scope: the vendor-API mimicry surface (/v1/vendor-api/* [server §C.3.9]).
The UI never talks to it; it exists for external OEM-shaped clients.
C.0 Invariants
Server is truth. Home configuration, device state, sim time, dispatch outcomes,
and settlement numbers are rendered from batsim data or not at all. The client never
simulates physics, never extrapolates SOC beyond the last received value, and never
"fakes" a device response to a command (§C.4).
One-way data flow. Server → ingest pipeline → state layers → renderer. User
intent → command layer → server → (telemetry/audit confirmation) → state layers.
No render-path component ever writes to the server-truth cache directly.
Determinism is a feature to expose. Given snapshot + seed + audit log, runs
reproduce exactly [server §C.5.2]. The UI treats this as the backbone of savegames,
A/B compare, and demo mode (§C.5, §C.7.4), and surfaces snapshot_hash /
registry_version wherever reproducibility matters.
High-frequency data never touches React state. Telemetry above ~2 Hz per entity
lives in typed-array ring buffers in a Web Worker (§C.2.2); React re-renders on
throttled derived snapshots only.
C.1 API Client Generation
C.1.1 Tool choice (committed)
Decision: openapi-typescript + openapi-fetch. Not Orval.
Rationale:
batsim is OpenAPI-3.1-first (utoipa-generated, openapi_version: "3.1.0" in
/v1/system/version [server §C.3.11]). openapi-typescript consumes 3.1 directly and
emits zero-runtime type declarations, which keeps the trust boundary explicit: types
are erased at runtime, and runtime validation is a separate, deliberate layer (§C.1.3).
The hot paths of this UI — SSE/WS telemetry, ring-buffer writes, dispatch fan-out
progress — are not request/response and would bypass Orval's react-query codegen
anyway. Orval would generate hooks we then half-ignore. A thin hand-written data layer
over openapi-fetch (wrapped in TanStack Query where caching semantics help, e.g.
registry and series fetches) is smaller, more honest, and easier to keep type-exact.
openapi-fetch's middleware hooks give one place to attach the bearer token,
Idempotency-Key injection, trace_id capture, and RFC 9457 error normalization
(§C.4.5, §C.8).
Generation pipeline (committed):
plain
crates/batsim-server  →  GET /openapi.json (checked into ui repo as openapi/batsim.openapi.json)
ui/src/api/gen/batsim.d.ts        ← openapi-typescript (types only; do not hand-edit)
ui/src/api/client.ts              ← createClient<paths>() + middleware (hand-written)
ui/src/api/query/*                ← TanStack Query wrappers for cacheable GETs
ui/src/api/transport/*            ← SSE/WS + replay transports (hand-written, §C.3)
Rules: batsim.d.ts is regenerated, never edited; client.ts is the only module
permitted to import openapi-fetch; all feature code imports from ui/src/api/index.ts
(facade). This makes the trust boundary greppable: rg "from 'openapi-fetch'" must
return exactly one hit in src/.
C.1.2 CI: regeneration and breaking-change gate
CI job api-sync (runs on every PR and nightly):
Version bump trigger. batsim releases are tagged vX.Y.Z. A scheduled workflow
fetches /openapi.json from the released artifact (or runs batsim --dump-openapi
against the pinned container image), writes openapi/batsim.openapi.json, and opens
an automated PR bumping batsim.version in ui/package.json.
Regenerate. openapi-typescript openapi/batsim.openapi.json -o src/api/gen/batsim.d.ts
and commit. Build must pass with zero // @ts-expect-error additions.
Breaking-change gate. Run oasdiff breaking openapi/batsim.openapi.previous.json
openapi/batsim.openapi.json --fail-on ERR. Any breaking change (removed endpoint,
narrowed enum, changed required field, changed problem code enum) fails the PR. The
UI ships pinned to a batsim semver range (engines.batsim: ">=0.4 <0.6"); minors
are adopted by regenerating, majors require a human PR. At runtime the UI re-checks
via /v1/system/version (§C.7.3).
Contract lint. A repo test asserts that every problem code referenced in
ui/src/api/errors.ts exists in the OpenAPI-described problem registry, and vice
versa — error handling cannot drift silently from the server's SCREAMING_SNAKE enum
[server §C.3.1].
C.1.3 Runtime narrowing at the trust boundary
Generated TS types are compile-time only. At runtime, the boundary modules
(client.ts, transport/*) apply assertion guards before data enters the state
layers:
Library: zod (committed), schemas co-located in ui/src/api/guards.ts. Schemas
are written for envelope and contract-critical shapes, not every response: problem+
json envelope, TelemetryTick, TelemetryGap, DispatchAccepted,
DispatchStatus, SimStatus, SnapshotMeta, SystemVersion, SettlementReport
header. Deep entity bodies (registry battery models, full home config) are type-trusted
after a shallow shape check — the failure mode of a slightly-wrong battery spec field
is a wrong label, not corrupted state.
Policy: guard failures in query paths throw a typed TrustBoundaryError →
surfaced as an error state (§C.4.5 toast taxonomy). Guard failures in stream paths
increment a stream_guard_failures counter and drop the frame (never throw inside the
ingest loop — one malformed tick must not kill the pipeline). A non-zero counter shows
a "protocol mismatch — check batsim version" banner wired to §C.7.3.
Discriminated unions stay discriminated. Dispatch actions are a closed oneOf
with discriminator type [server §C.3.7]; guards must validate the discriminator
first and branch, so TypeScript narrowing remains sound downstream.
C.2 State Architecture
Three layers, strictly separated. Layer (a) is low-frequency server truth; layer
(b) is high-frequency telemetry; layer (c) is ephemeral UI. The renderer reads only
(b) and (c) — server truth reaches the screen either by being copied into (b)-style
buffers by the ingest pipeline, or via throttled selectors that Part B's render loop
polls at most once per frame.
C.2.1 Layer (a): Server truth — normalized entity cache
Committed: zustand + immer. RxJS is used only inside stream pipelines (§C.3) as
an operator chain between transport and stores; it is not an application state
container. Rationale: entity cache operations are boring CRUD normalization — zustand +
immer + a hand-rolled normalizer is ~200 LOC, trivially testable, and devtools-friendly.
RxJS stores would buy nothing here and cost every future contributor a mental model.
Store: useEntityStore (zustand, immer middleware). Normalized shape:
TypeScript
interface EntityCache {
  homes:    Record<HomeId, HomeEntity>;            // §C.9.1
  fleets:   Record<FleetId, FleetEntity>;
  devices:  Record<DeviceId, DeviceEntity>;        // battery + inverter instances
  scenarios: Record<ScenarioId, ScenarioBinding>;  // §C.9.5
  registry: { batteries: Record<ModelId, BatteryModel>;
              inverters: Record<ModelId, InverterModel>;
              version: string };                   // from /v1/registry/version
  indexes: {
    homesByFleet: Map<FleetId, HomeId[]>;
    homesByZone: Map<LoadZone, HomeId[]>;
    deviceByHome: Map<HomeId, DeviceId[]>;
  };
  meta: { fetchedAt: number; cursorExhausted: boolean; etag?: string };
}
Population and invalidation rules:
Boot hydrate: GET /v1/registry/version + battery/inverter lists (cached in
IndexedDB keyed by registry_version; registry changes only on batsim upgrade, so
cache-miss is rare). Then paginate GET /v1/homes?limit=1000&cursor=… until
has_more: false [server §C.3.1]; cursors are opaque — the client stores the last
cursor only for resumption after reload, never constructs one.
Write-through only via command layer. Mutations (POST /v1/homes, PATCH,
fleet create/expand, scenario activate) apply the server response body into the cache
on success. The client never applies a local guess of a mutation's result.
Consistency: GET /v1/homes/{id} after observing SSE tick N returns state ≥ N
[server §C.5.2 happens-before rule]; the cache stamps every entity with
lastTick and refuses to overwrite a newer entity with an older one.
Scale: 10k homes × ~2 KB normalized config ≈ 20 MB in the cache — fine. Entity
objects are frozen after write (immer auto-freeze); selectors use shallow-equal
memoization. The cache is not the telemetry source: a home's live state.soc in the
cache updates at most on explicit refetch or low-frequency reconcile (≤ 0.2 Hz), never
per tick.
C.2.2 Layer (b): Telemetry ring buffers in a Web Worker
Structure. A dedicated telemetry.worker.ts (Part B owns the worker pool; this part
owns the buffer schema and the message protocol). Per metric, per home: a
Float32Array ring with head index and capacity. Metric set (committed, order fixed):
plain
M = [battery_power_kw, pv_power_kw, load_power_kw, grid_power_kw, soc, battery_temp_c]
plus per-frame scalar metadata (sim_time as epoch-ms Float64, tick as Uint32).
Two-tier retention (mirrors the server's tiers [server §C.3.8]):
Table
Tier   Coverage   Resolution   Retention   Math   Size
Fleet aggregate   1 series per fleet (+ per-load-zone)   1 Hz   24 sim-h   86 400 × 8 ch × 4 B × ~8 series   ≈ 22 MB
Hot homes   viewport + selected + dispatch-targeted homes, cap 1 000   1 Hz   1 h   1 000 × 6 × 3 600 × 4 B   ≈ 86 MB
Focus home   the single Z5 battery-cutaway home   1 Hz (raw tick)   4 h   6 × 14 400 × 4 B   ≈ 0.35 MB
Full-fleet per-home rings at 1 Hz/1 h would be 10 000 × 6 × 3 600 × 4 B ≈ 864 MB —
rejected. The hot-home cap of 1 000 is a hard invariant enforced by an LRU keyed on
(viewport-membership score, selection, recent dispatch). Evicted homes lose client-side
history; their historical data is re-pullable from /v1/telemetry/homes/{id}/series
(resolution 1m beyond raw retention) — the scrubber semantics in §C.5.3 depend on
this. Total steady-state layer-(b) budget: ≤ 128 MB, enforced by an
allocator that refuses new rings beyond budget and forces LRU eviction.
Transfer strategy (committed): SharedArrayBuffer when available, transferable chunks
otherwise. Feature-detect crossOriginIsolated; the static host must send
Cross-Origin-Opener-Policy: same-origin and Cross-Origin-Embedder-Policy: require-corp
(§C.8.3) to enable it. With SAB, the render thread maps ring heads directly (zero-copy,
no per-frame postMessage; a Atomics-guarded write index per ring). Without SAB, the
worker posts transferable Float32Array slices at frame cadence (≤ 60 msg/s aggregate,
batched by Part B's render tick). Both paths expose the same RingView interface to the
renderer, so Part B code is transport-agnostic.
No React state for telemetry. The worker computes throttled derived snapshots
(fleet aggregate at 2 Hz, visible-home summaries at 2 Hz, focus home at 10 Hz) and
pushes them into a tiny zustand store (useTelemetryMetaStore) — that is the only
telemetry-adjacent state React sees, and it is already aggregated. Per-vertex/per-instance
animation reads RingView directly in Part B's render loop.
C.2.3 Layer (c): Ephemeral UI state
useUiStore (zustand, vanilla, no immer — updates are flat). Contents: mode
(build/inspect/dispatch/scenario, Part A), selection set (Set<HomeId> with max 10 000,
stored as sorted array + dirty flag), camera stratum + pose (written by Part B's camera
rig, read by nobody except persistence), time-control intent (playing/paused/speed
request — distinct from server's actual state, §C.5.1), toasts, open inspector panels,
command-in-flight map (§C.4.2), and savegame labels (§C.5.4, persisted to IndexedDB).
Everything here is disposable: a full reload may lose layer (c) without any correctness
impact. Persistence to localStorage is allowed only for camera pose and mode (session
convenience), never for selection sets > 500 entries.
Access rules (linted): render code (Part B) imports only from (b) and (c); the
command layer imports (a)+(c) and the API facade; the ingest pipeline writes (a)+(b);
nothing else may import useEntityStore outside selectors. Enforced via
eslint-plugin-boundaries zones.
C.3 Telemetry Ingestion Pipeline
C.3.1 Transport: SSE vs WebSocket (decision)
batsim offers both with the same JSON message schema: GET /v1/telemetry/stream (SSE,
with Last-Event-ID resume, id = tick number) and GET /v1/telemetry/ws (WS,
subprotocol batsim.v1+json) [server §C.3.8].
Table
Criterion   SSE   WebSocket   Weight
Resume after drop (Last-Event-ID = tick)   native   manual (send {"resume_from_tick": N} control msg)   high
Bidirectional (change filter/downsample without reconnect)   no — new GET per subscription change   yes — control frames on same socket   high
Binary frames (future MessagePack, §C.3.2-OPT)   never   yes   medium
Proxy/browser friendliness, auto-reconnect   native EventSource   manual   medium
Connection count vs zoom churn (Part A Z1↔Z5 transitions change subscriptions constantly)   churn = reconnect storms   one socket, re-subscribe in place   high
Backpressure signal   server bounded channel → event: gap   same schema   equal
Decision: WebSocket primary, SSE automatic fallback. WS wins on in-place
subscription changes — zoom transitions would otherwise force an SSE reconnect per
stratum change, dropping ticks exactly when the user is most active. Fallback triggers:
WS handshake failure, subprotocol rejection, or proxy-induced 1006 storms (≥ 3 abnormal
closes in 60 s) → pin to SSE for the session. Both transports implement the same
BatsimTransport interface (§C.9.6) so the pipeline is transport-agnostic — this is
also the seam demo mode plugs into (§C.7.4).
C.3.2 Message framing
As-specced (normative). batsim streams JSON: event: tick frames carrying
{sim_time, tick, fleet{…}, price_rtm} for fields=aggregate, or per-home vectors for
fields=raw (≤ 500 homes [server §C.3.8]); plus event: dispatch, event: gap,
event: sim (time-control state). The client pipeline:
Transport delivers raw text frames to the telemetry worker.
Worker parser validates with the §C.1.3 guards, then re-frames into columnar
batches: { t: Float64Array(epoch_ms), tick: Uint32Array, cols: Float32Array[metric][n] }.
Row-oriented JSON is a wire concern only; everything downstream of the worker is
columnar. Ring-buffer writes are direct Float32Array sets — no per-row object
allocation after parse (GC pressure at 10k×1 Hz JSON would otherwise dominate).
Batched dispatch to rings. Frames are accumulated for one render frame (16 ms)
or 256 rows, whichever first, then committed. Commit = write columns + advance
Atomics head (SAB) or post one transferable batch.
OPTIONAL — batsim API extension proposal (not required for UI v1; server change):
binary telemetry via WS subprotocol batsim.v1+msgpack, MessagePack-encoded columnar
frames {t:[…], h:[home_id…], m:[[power_w…],[soc‰…]]} with delta-encoded ticks and
home_id interning (first frame sends the id→slot map; subsequent frames send slot
indices). Estimated 6–10× bandwidth cut at 10k-home raw streaming. The UI must function
fully without it; if the server negotiates batsim.v1+msgpack, the worker swaps in the
binary parser behind the same columnar output. Until batsim implements this, it is
explicitly out of contract — flagged here so the worker's parser is already behind a
FrameParser interface.
C.3.3 Backpressure, gaps, resync
The server declares the stream lossy-under-overload: bounded per-consumer channel
(1024 events), then event: gap { missed_ticks: [a, b] } [server §C.3.8]. Client
policy:
Gap detection (primary): tick sequence. Every tick frame carries tick; the
worker tracks lastTick and treats tick > lastTick + 1 as a gap even without an
explicit gap event (WS reconnect mid-stream, dropped WS frames under TCP
retransmit stalls). event: gap is the authoritative form; sequence arithmetic is
the safety net.
Drop policy (client-side, when the worker itself falls behind): keep aggregates,
drop intermediate per-home frames. Concretely: a bounded ingress queue (2 048 frames);
on overflow, drop the oldest raw per-home batches first, never aggregate frames,
never the newest frame of any kind (freshness > completeness). Record
dropped_frames_total{kind} and surface a subtle "streaming degraded" indicator —
Part D owns the visuals; this part exports the counter.
Resync: for each gapped tick range, fetch
GET /v1/telemetry/homes/{id}/series?from=…&to=…&resolution=1s for hot homes and the
fleet aggregate series for the fleet tier; write into rings marked backfilled.
Cap resync at the raw-retention tier (24 sim-h [server §C.3.8]); older gaps are
filled at 1m resolution and flagged degraded so charts can render the fidelity
change (Part D). If resync fails (sim moved on), leave the gap — rings are allowed
holes; renderers must treat holes as "no data", never interpolate across them.
Resume: SSE reconnect sends Last-Event-ID: <lastTick>; WS reconnect sends
resume_from_tick. Both are best-effort — the resync path above is the guarantee.
C.3.4 Downsampling ladder per zoom stratum
Subscription parameters derive from the active stratum (Part A) and selection. The
server's fields=raw 500-home cap [server §C.3.8] is the binding constraint.
Table
Stratum   Subscription   Rationale
Z1 (ERCOT map)   fleet_id=<active>&fields=aggregate&downsample=5s + per-load-zone aggregates   Map glyphs need fleet/zone rollups only; 10k-home raw is pointless and forbidden.
Z2 (city/neighborhood)   aggregate at 1 s + home_ids=<visible∩hot, ≤500>&fields=raw&downsample=1s   Visible-home instancing (Part B) reads per-home rings; > 500 visible ⇒ rank by viewport score, top 500 raw, remainder colored from zone aggregate.
Z3 (street)   same, hot set shrinks to street (≤ 120 homes typical)   full 1 Hz raw fits easily.
Z4 (house)   home_ids=<house>&fields=raw&downsample=1s + GET /v1/homes/{id} at 0.2 Hz for config-state fields not in telemetry (mode, reserve SOC)   
Z5 (battery cutaway)   focus-home ring at full tick rate; plus /v1/homes/{id} on demand   Part D internals animation needs SOC/temp/power at tick fidelity.
Ladder transitions are in-place WS re-subscriptions (§C.3.1): the client sends one
control message per stratum change; the worker keeps rings for evicted hot homes for a
60 s grace period (fast zoom-back without refetch), then LRU-evicts.
Aggregate subscription params (normative request shape):
GET /v1/telemetry/stream?fleet_id=flt_…&fields=aggregate&downsample=<5s|1s> (SSE) or
equivalent WS subscribe message; zone-level aggregates are requested as one subscription
per load zone present in the fleet's homesByZone index, using the server fleet-series
filter by zone where offered, else computed client-side from hot-home rings plus
server fleet aggregate (documented approximation; exact when ≤ 500 homes).
C.3.5 Jitter and latency display (data contract)
Part D renders latency; this part supplies the numbers:
Stream latency: per tick frame, stream_lag_ms = wall_recv − wall_parse_of_previous_tick_interval; rolling p50/p95 exposed at 1 Hz. Combined with server
achieved_speed/lag_ticks from /v1/sim:status (§C.5.1) to distinguish
"server can't keep up" from "network/UI is behind".
Dispatch execution latency: per-target latency_ms and executed_at_sim_time
from GET /v1/dispatch/commands/{command_id} detail [server §C.3.7], joined with the
client-observed first telemetry tick reflecting the action. Both are stored per
command in the in-flight map (§C.4.2) and exported to Part D's ripple/ack rendering.
Clock skew: sim_time in tick frames vs Date.now() — displayed as the drift
readout (§C.5.2); not a defect signal, a sim-speed signal.
C.4 Command Path — Dispatch
C.4.1 Authoritative model
POST /v1/dispatch is queue-ack, not device-ack [server §C.5.2]. A 202-style
{accepted: true, status: "queued"} means batsim enqueued (execute_at_tick, action)
per target home; physics-limited application happens later, at per-home tick boundaries,
with per-home jitter drawn from execution.latency_ms [server §C.3.7]. Therefore:
Decision: server-authoritative with explicit in-flight states. No optimistic device
response, ever. The UI may optimistically render the envelope ("command accepted,
5 000 targets queued") because that is server-confirmed truth; it must never render a
battery as discharging before telemetry or the audit log says so. This is both a
correctness rule (the sim may partially apply: applied_kw: 3.2 vs requested_kw: 5.0)
and a product rule — the tool's purpose is to observe fleet behavior; faking it
destroys trust in every other number on screen.
C.4.2 Command lifecycle in the client
plain
idle → validating (client pre-check) → sending → queued → in_flight
     → completed | completed_with_errors | failed | cancelled
Client pre-check (advisory only): closed action enum, target expansion preview
(client computes target count from homesByFleet + filter; server is authoritative),
power window vs nameplate from registry. Pre-check failures block send with an inline
error; they never reach batsim.
Idempotency (mandatory): every send carries header Idempotency-Key:
<uuidv4> and body command_id (ULID, monotonic — sorts in the audit log)
[server §C.3.1/§C.3.7]. Retries (network flake, user double-click, React StrictMode
double-invoke in dev) reuse the same key and id, so the server dedups before
enqueue; a replayed response (Idempotent-Replay: true) is treated as fresh.
The command layer generates both at intent creation, not at send time.
Tracking: response gives status_url; the client records an
InFlightCommand { command_id, targets, action, sentAtWall, statusByHome? } in
layer (c) and tracks completion via SSE/WS event: dispatch frames first (no
polling in the common case), falling back to polling status_url at 1 s if the
stream is down. Rollup: queued → in_flight → completed | completed_with_errors.
Reconciliation: on completed_with_errors, fetch full per-target detail
(applied|partial|rejected|timeout, applied_kw, latency_ms) and store keyed by
command_id for the inspector; homes with partial|rejected get a per-home outcome
entry that Part D renders. The entity cache is not updated from dispatch outcomes —
only from telemetry/home refetch (invariant C.0.1).
C.4.3 Fan-out UX for 10 000 homes (data side)
The server's fan-out gives deterministic per-home jitter (300–2 500 ms default uniform)
[server §C.3.7]. Client data obligations:
Progress model: acked = Σ targets with execution record, sourced from dispatch
events + detail polling. Progress is a count and a per-home latency scatter, never a
fabricated percentage. Expected completion window = max(latency_ms) from the
command's own execution spec; overdue = now_sim > sent_tick + max_latency + timeout_s
with acks outstanding → escalate to completed_with_errors locally if the server
hasn't (server remains right when it eventually reports).
Per-device acks: hot homes get ring-buffer markers (command tick + applied tick)
so Part D can ripple acknowledgments across the map with the actual jitter
distribution; non-hot homes contribute only to the aggregate counter.
Throughput guard: max 8 in-flight fleet commands per client; beyond that, queue
client-side with visible position. (The server will accept more; the UI constraint is
about legible feedback, not server load — rate limiting is intentionally absent
server-side [server §C.4].)
C.4.4 Undo and scheduled commands
Undo = compensating command, not rollback. Once applied, a dispatch has perturbed
deterministic state; the only true undo is snapshot restore (§C.5.4). For UX undo
within a command's life: while queued, DELETE /v1/dispatch/commands/{id} cancels
[server §C.3.7]; after application, the UI offers "revert" = issue the inverse action
(clear_override, or restore the pre-command mode/reserve captured from the entity
cache at send time) as a new command with its own idempotency key and an
undo_of: <original command_id> client-side link. The inspector shows the pair.
Scheduled commands: batsim has no future-dated dispatch endpoint; the UI
implements a sim-time scheduler: ScheduledCommand { command_template, fire_at_sim_time,
fired? } in layer (c). A 1 Hz watcher on the virtual clock (§C.5.1) issues the command
when sim_time ≥ fire_at. Scheduler entries are persisted to IndexedDB and re-armed on
reload (with "missed while closed → fire now or skip?" prompt). Caveat surfaced in UI:
scheduled commands fired by the client are part of the deterministic audit log only
once sent; a branched replay (§C.5.5) re-fires them only if the branch runs with the
same client session. For fully deterministic scheduled behavior, users bind outage/
regime events in the scenario (§C.6.1) instead — the scenario editor is the
deterministic path; the scheduler is the interactive one.
C.4.5 Error surfacing (RFC 9457)
batsim errors are application/problem+json with a fixed problem-type registry and
stable code enum [server §C.3.1]. Client mapping (committed, table-driven in
ui/src/api/errors.ts, contract-linted per §C.1.2):
Table
Server code / status   UI treatment
validation-error (400)   inline form errors on the originating control; no toast
unauthorized (401)   auth banner + token re-entry (§C.8.1); queue held
not-found (404)   toast "…no longer exists" + entity cache eviction
conflict / sim-running / sim-not-running (409)   toast + inspector detail; offer the legal transition (e.g. "pause sim to edit home") as a one-click action
unprocessable (422, physics/rule, e.g. DISPATCH_WINDOW_VIOLATION)   toast + inspector detail rendering detail and per-target invalid_targets (id, reason, available_kw) as a table; offending homes flagged in selection
idempotency-key-reuse (409)   client bug — toast + Sentry; never auto-retry
internal (500)   toast with trace_id copy button
Every problem response's trace_id is captured into the toast/inspector detail. Toast
text is always title; detail and extensions live in the inspector. Part D owns
toast visuals; this part owns the mapping table and the guarantee that no problem+json
response is ever swallowed.
C.5 Time Synchronization, Scrubbing, Savegames
C.5.1 Virtual clock model
batsim owns time: SimTime, tick-aligned, wall-clock never enters the sim core
[server §C.1, §C.3.6]. The client maintains a clock estimator, not a clock:
Sources: tick frames (sim_time, tick — highest frequency, authoritative) and
/v1/sim:status polled at 2 Hz (state, sim_time, tick, speed, achieved_speed,
lag_ticks, queued_commands).
Estimation: between tick frames, displayed sim time = last_sim_time +
achieved_speed × elapsed_wall, clamped so it never runs ahead of the next tick
frame by more than one frame budget. Pause = freeze estimator. The estimator is a
pure function of (last frame, status poll); no independent timers drive it.
Time-control intent vs state: layer (c) holds the requested speed/play state
(from Part A time controls); the rendered speed readout always shows the server's
speed/achieved_speed. Control calls: POST /v1/sim:start|pause|resume|stop,
PUT /v1/sim:speed {multiplier}, POST /v1/sim:step {ticks} (≤ 86 400 unless
?allow_large=true), POST /v1/sim:run-until {until}. Illegal transitions return
409 sim-running/sim-not-running → mapped per §C.4.5, with the UI offering the
legal predecessor action.
C.5.2 Drift display (data contract)
The HUD drift readout (Part D renders) consumes, at 1 Hz: sim_time, wall_time,
drift_s = (sim_time − session_start_sim) − speed×(wall − session_start_wall),
achieved_speed / speed ratio, and lag_ticks. Persistent lag_ticks > 0 at
multiplier ≤ target means the host can't sustain the speed [server §C.6] — the readout
turns advisory ("simulation slower than requested") rather than pretending otherwise.
C.5.3 Run-until and scrubber semantics
Run-until: POST /v1/sim:run-until is synchronous and can run long at high
tick counts. Client policy: issue it from a dedicated fetch with a generous timeout;
while in flight, the UI shows blocking-but-live progress (tick frames continue to
stream — the estimator keeps HUD time honest). For jumps > 1 sim-day, prefer chunked
run-until calls (per sim-day) so progress is smooth and the request is cancellable
between chunks; a cancel is simply "don't issue the next chunk" plus sim:pause.
Timeline scrubber (the savegame-adjacent mechanic, Part A):
Within client retention (hot rings, §C.2.2): scrub is pure replay from local
ring buffers — zero server calls, works while paused or running (pauses the
estimator, not the sim). Renderer reads historical ring windows; a "return to live"
affordance snaps back. This is the default scrub experience.
Beyond retention: snapshot + re-run. The client (1) finds the newest snapshot
with sim_time ≤ target from GET /v1/sim:snapshots, (2) POST
/v1/sim:snapshots/{id}:restore (requires sim stopped/paused — UI orchestrates:
pause → restore → verify snapshot_hash in the response matches the recorded one),
(3) run-until target at max speed, (4) resumes live subscriptions. The user sees
an explicit "re-simulating from savepoint" phase (Part D) — this is honest about the
cost and leans on determinism as the guarantee that the re-run equals what happened.
C.5.4 Snapshot-as-savegame
batsim snapshots: POST /v1/sim:snapshots → snap_01J…; tick-aligned, stop-the-world,
never torn; binary .batsim.snap downloadable; restore is atomic and returns
snapshot_hash [server §C.3.6, §C.5.4]. Savegame model (§C.9.4):
Create: pause → capture → record client-side metadata. Labels live client-side:
the server snapshot store has no label field, so SaveGameMeta { snapshot_id,
snapshot_hash, label, notes, created_at_wall, sim_time, scenario_id, fleet_id,
registry_version } is persisted in IndexedDB keyed by snapshot_id. On batsim
restart without persistence, snapshots vanish — the UI detects stale labels via
GET /v1/sim:snapshots and offers download/archival before shutdown (export =
GET /v1/sim/snapshots/{id} binary + JSON sidecar of the meta; import is a server
extension point, flagged OPTIONAL: POST /v1/sim:snapshots with .batsim.snap body).
Restore: orchestrated sequence as §C.5.3 steps 2–3; confirm snapshot_hash
equality before resuming; restore wipes nothing client-side except telemetry rings
(which are now future-less — scrub-back beyond restore point is disabled until rings
refill; the UI says so).
Branch (clone → new scenario): restore snapshot S, create a new ScenarioBinding
differing in any of {seed, prices, outages, strategy commands} (§C.6.1), activate,
run. Client records lineage: SaveGameMeta.parent_snapshot_id and
branch_of: scenario_id, rendering a savegame tree (Part A owns tree UI).
C.5.5 A/B strategy compare — data requirements
One batsim instance = one world; side-by-side A/B therefore needs two runs with
identical provenance. Two supported configurations:
Two batsim instances (recommended): docker-compose profile ab runs
batsim-a, batsim-b (ports 8080/8081); the UI holds two BatsimTransport +
entity-cache stacks (stores are factory-created, not singletons — hard requirement
for this feature and for tests). Both restore the same snapshot (verify identical
snapshot_hash on both), apply branch-specific scenario deltas, run at matched
speeds. This is the only mode that supports live side-by-side.
Sequential, single instance: run A → capture recorded trace (§C.7.4) +
settlement report → restore same snapshot → run B → compare A's recording against
B live. Cheaper; A is frozen.
Hard data requirements for valid compare (UI must verify and display all):
identical snapshot_hash, registry_version, scenario seed (unless seed is the
independent variable), and price-series identity (source, date range, provenance
[server §D.3] — settlement-final vs real-time-indicative must match);
aligned time axes: both branches keyed by tick, not wall time; all charts/telemetry
joined on tick;
per-branch captures: fleet aggregate ring, per-metric series export
(/v1/telemetry/fleets/{id}/series), dispatch audit slice (/v1/dispatch/commands?
since=…), and GET /v1/runs/{id}/settlement at matched sim-time checkpoints;
a CompareBundle { A: BranchCapture, B: BranchCapture, provenance_checks: [...] }
exported to Part A/D for the side-by-side view; divergence metrics (Δ SOC mean, Δ
fleet power L1, Δ cumulative P&L) computed client-side from the captures.
Any failed provenance check renders the compare "indicative only" with the failing check
named — compare without provenance discipline is worse than no compare.
C.6 Scenario & Price Data
C.6.1 Scenario editor → POST /v1/scenarios
A scenario binds time + data sources + seed to the fleet [server §C.3.5]. The editor
(Part A scenario mode) produces a ScenarioBinding (§C.9.5) mapping 1:1 onto the
server schema:
Time: start, end, tick_seconds (default 1; UI warns above 60 s that Z5
animations lose fidelity).
Prices: source: "replay" binds date_range, market: RTM|DAM,
settlement_point (typed Hub | LoadZone | Node per [server §D.1]; UI offers the
four competitive load zones + hubs as enums); source: "synthetic" binds
{ profile, volatility, seed } (e.g. summer_peak); source: "live" binds the live
ERCOT adapter. The editor exposes replay availability as validated by batsim —
:activate preloads series and 422s on missing replay data; the editor pre-flights
with a price-series range probe (§C.6.2) so the error is caught before activation.
Outages: list of { start, end, scope.load_zones, probability } — rendered on
the timeline as event bands (Part D); the editor edits them as first-class timeline
objects but serializes to the exact server shape.
Weather, ancillary, seed: passthrough fields; seed is surfaced prominently
(determinism, §C.5.5) with a "lock seed" toggle.
Lifecycle UX: exactly one scenario may be active; :activate requires sim stopped
— the UI orchestrates stop → activate → start as one user gesture with per-step error
mapping (§C.4.5). Activating a scenario invalidates: entity cache meta.fetchedAt,
all telemetry rings (flush — new time base), savegame labels referencing a different
scenario are shown but marked cross-scenario.
C.6.2 Price series fetching strategy
Endpoint: GET /v1/market/prices?settlement_point=…&from=…&to=…&market=RTM|DAM
[server §C.3.10]. RTM is 5-min settlement (post-RTC+B interval length is server config
settlement_interval_secs, 300 or 900 — the UI must read it from
/v1/system/config, never assume) [server §D.1]; DAM is hourly.
Windowing: fetch chart-visible window ± 50 % margin; a 7-day RTM window is
2 016 points at 5-min — trivially fetched whole; month+ windows are fetched at
server-side bucket resolution when offered, else fetched whole and decimated
client-side.
Decimation (chart-ready): min-max bucket decimation (preserves spikes — the
economically important feature, e.g. Uri's $9 001 prints) into Float32Array
pairs sized to 2× canvas pixel width; LTTB is acceptable but min-max is preferred
because it never clips a price spike between samples. Decimation runs in the
telemetry worker; main thread receives transferables.
Overlay alignment: DAM hourly steps and RTM 5-min series are separate traces on
one axis; joins by interval-start timestamp. provenance (RealTimeIndicative /
SettlementFinal / Synthetic [server §D.3]) is stored per series and displayed —
settlement-grade P&L must not be eyeballed against indicative prints.
Caching: TanStack Query keyed (settlement_point, market, from, to, provenance),
staleTime: Infinity for historical replay ranges (immutable), 60 s for live.
C.6.3 Settlement reports
GET /v1/runs/{id}/settlement → SettlementReport (per-home ledger lines + fleet
rollups, one row per settlement interval, plus daily/monthly aggregates; baseline
method recorded for auditability) [server §C.3.10 workflow, §D.5].
When to fetch: settlement is meaningful only for closed intervals. The client
tracks interval close from the virtual clock (interval length from config) and polls
the report endpoint with backoff (5 s → 30 s) while the run is active, plus one
final fetch at run end (sim:stop / scenario end reached). No push channel exists;
polling cadence is tied to interval closes, not wall time, so at 10× speed a 15-min
interval closes every 90 wall-seconds and the poll schedule compresses accordingly.
Client handling: report is append-mostly by interval; the client diffs by
interval index and accumulates a cumulative P&L series into the telemetry worker as
another ring channel (so P&L scrubs with everything else). Per-home ledger lines are
fetched on demand (inspector), never in bulk to the renderer — 10k × intervals rows
stay server-side until a user asks for a slice.
A/B compare: settlement captures are the scoreboard for §C.5.5; both branches
must be fetched at the same tick.
C.7 Resilience
C.7.1 Reconnect & catch-up
Backoff (committed): initial 1 s, ×2 to a 30 s ceiling, ±20 % jitter, reset on
first successfully parsed frame. Applies to WS, SSE, and REST poll loops alike
(/v1/sim:status, health). No max retry count — batsim is a local tool that may be
restarted minutes later; the UI backs off quietly and shows connection state
(§C.7.5).
Stream catch-up: on reconnect, resume by tick (Last-Event-ID /
resume_from_tick), then §C.3.3 resync for any residual gap. While disconnected,
rings simply stop advancing; on resume, the gap is backfilled from series endpoints
and marked backfilled.
Entity catch-up (snapshot-diff): after any disconnect > 10 s: re-pull
/v1/sim:status; if tick jumped, refetch changed entities via
GET /v1/homes?fleet_id=… cursor walk, diffing against layer (a) by (id,
lastTick) and applying only changed entries. Full re-hydrate only if the fleet
composition hash (count + max updated marker from the list page metadata) differs.
The client never assumes its cache survived a server restart: on new
/v1/system/health.uptime_s regression (uptime went down = server restarted), force
full re-hydrate and flush rings.
C.7.2 Idempotent recovery
All in-flight commands at disconnect are re-polled by command_id (never re-sent)
via GET /v1/dispatch/commands/{id}; unknown → the client may re-POST with the same
idempotency key and command_id, making ambiguity safe [server §C.3.1].
C.7.3 API version negotiation
Boot sequence: GET /v1/system/version → check version against the build's pinned
engines.batsim range and openapi_version == 3.1.x. Out-of-range → modal:
"UI built for batsim ≥0.4 <0.6, found 0.7.0" with options proceed (degraded) or
switch to demo mode. Degraded mode keeps REST reads but disables dispatch and
time-control (write paths are where schema drift bites). Re-checked on every
/v1/system/health transition from down→up.
C.7.4 Demo / offline mode — recorded traces
The UI must be fully demoable with no batsim process. Mechanism: a loopback
transport implementing BatsimTransport (§C.9.6) that replays a recorded trace
through the identical worker pipeline (same parser, same guards, same ring writes).
Because ingest is transport-agnostic, demo mode is not a mock layer — it is the real
pipeline with a file-shaped server.
Trace format (normative, versioned): a directory (or zip) containing:
plain
manifest.json          # required
telemetry.jsonl        # required — one JSON object per line
dispatch.jsonl         # optional — audit/dispatch events
prices.json            # optional — { market, settlement_point, interval_s, t[], v[] }
entities.json          # required — homes/fleets/registry snapshot (hydrates layer (a))
settlement.json        # optional — SettlementReport as captured
manifest.json:
JSON
{
  "format": "batsim-trace/1",
  "recorded_from": { "batsim_version": "0.4.2", "registry_version": "…" },
  "scenario": { "name": "feb-2021-uri-replay", "seed": 1337, "tick_seconds": 1 },
  "tick_range": [51200, 60000],
  "sim_time_range": ["2021-02-15T07:00:00Z", "2021-02-15T09:26:40Z"],
  "fleets": ["flt_01J…"], "homes": 10000,
  "channels": ["aggregate", "hot_homes"], "hot_home_ids": ["home_01J…"]
}
telemetry.jsonl: one line per event, using the exact SSE data: payload schema
plus an event field: {"event":"tick","tick":51234,"sim_time":"…","fleet":{…},
"price_rtm":…} — so the loopback transport can feed the parser verbatim. Playback
controls map onto the same clock estimator (play/pause/speed/scrub = seeking line
offsets; the loopback transport indexes byte offsets of tick lines at load for O(1)
seek). Dispatch in demo mode: the command layer is shimmed to respond from
dispatch.jsonl recordings (commands the trace captured) or return a synthetic
unsupported-in-demo problem for anything else — visibly badged, never silent.
Recorder: a client-side "record session" toggle writes this format from the live
pipeline (JSONL sink in the worker). This is also how §C.5.5 sequential A/B captures
branch A. Sample traces ship in ui/public/traces/ so vite dev demos instantly.
C.7.5 batsim-unreachable empty state
When /v1/system/health is unreachable (not 503 — 503 means starting, show its
sim_state progress): full-screen empty state (Part D owns art) with: connection
target (§C.8.1), last error, retry-now button, and two escape hatches — open demo
mode and edit connection. State layers are not torn down on transient
disconnects (a 30 s blip shouldn't lose a 10k-home hydrate); they are torn down only on
explicit "switch server" or version-incompatible reconnect.
C.8 Security & Configuration
C.8.1 API base URL and auth
Resolution order (first hit wins): (1) runtime config.json fetched at boot from the
static host ({ "apiBaseUrl": "…", "authRequired": bool }), exposed as
window.__BATSIM_CONFIG__; (2) build-time env VITE_BATSIM_API_BASE_URL; (3) default
http://localhost:8080. Runtime-over-build lets one artifact serve dev, compose
(two-instance A/B gets apiBaseUrlB), and demo without rebuilds.
batsim auth is an optional bearer token (Authorization: Bearer <key> or
X-Api-Key; constant-time compared server-side; read-only keys exist) [server §C.4].
Client policy: the token is entered at runtime and held in memory only (module
scope in client.ts), attached by openapi-fetch middleware; never in localStorage,
never in the URL, never in the Vite bundle. WS attaches it via the subprotocol-secured
query/header path the server documents; SSE cannot set headers natively, so the SSE
fallback uses a fetch-based EventSource polyfill. 401 → §C.4.5 flow. The principal
stamped into batsim's audit log is the key's configured name — the UI displays it in
the inspector's "command provenance" line.
C.8.2 CORS and dev proxy
batsim is single-tenant, localhost-first, and does not promise a CORS surface for
arbitrary origins [server §C.4]. Therefore: dev — Vite dev-server proxy
(server.proxy: { "/v1": "http://localhost:8080", "/openapi.json": … }, ws: true
for /v1/telemetry/ws); the browser sees same-origin. Prod/demo — the static host
reverse-proxies /v1 to batsim (nginx/compose), keeping everything same-origin and
letting COOP/COEP (below) apply cleanly. If a deployment must hit batsim
cross-origin, it needs a batsim CORS config change — flagged as an operator concern,
not a UI feature.
C.8.3 Headers and isolation
Static host must send Cross-Origin-Opener-Policy: same-origin and
Cross-Origin-Embedder-Policy: require-corp to enable crossOriginIsolated →
SharedArrayBuffer (§C.2.2). All proxied batsim responses and glTF/map assets (Part B/D)
must be same-origin or carry Cross-Origin-Resource-Policy: cross-origin, or SAB
silently disables; the client detects the fallback and logs it (perf regression, not a
bug).
C.8.4 No PII
Homes are synthetic entities: home_01J… ids, archetype load profiles, generated
geometry (Part B procedural neighborhoods). The UI must not introduce real customer
identifiers — no name/address/account fields anywhere in entity models, inspector
labels, exports, or analytics. Street/place labels in the UI are procedurally generated
from the scenario seed. Analytics (if any) carry only home_id hashes and fleet
aggregates. This is a standing code-review rule, not a runtime mechanism: the entity
schemas in §C.9 contain no PII-shaped fields by construction.
C.9 Type Sketches
UI-side contracts. Field names mirror batsim's OpenAPI schemas; where the UI adds
client-only fields they are namespaced under ui or in separate meta types. Server
shapes are imported from api/gen/batsim.d.ts (components.schemas) — the sketches
below show the composed client models.
C.9.1 HomeEntity (layer a)
TypeScript
type HomeId = `home_${string}`;
type FleetId = `flt_${string}`;

interface HomeEntity {
  id: HomeId;
  fleetId: FleetId;
  config: {                                  // mirrors POST /v1/homes body (validated echo)
    battery:  { modelId: string; count: number };
    inverter: { modelId: string };
    pv:    { peakKw: number; azimuthDeg: number; tiltDeg: number };
    load:  { archetype: string; annualKwh: number };
    location: { ercotLoadZone: LoadZone; climateZone: string };
    initialSoc: number;
  };
  state: {                                   // low-frequency reconcile copy only (§C.2.1)
    soc: number; mode: HomeMode;             // self-consumption|backup-only|time-of-use|grid-services
    batteryPowerKw: number; pvPowerKw: number;
    loadPowerKw: number; gridPowerKw: number;
  };
  lastTick: number;                          // happens-before guard (§C.2.1)
  ui: { worldPos: [number, number, number];  // assigned by Part B layout, cached here
        hotScore: number };                  // §C.2.2 LRU input
}
C.9.2 TelemetryFrame (worker wire format, post-parse)
TypeScript
interface TelemetryFrame {                   // one SSE/WS event, parsed & guarded
  kind: "tick" | "dispatch" | "gap" | "sim";
  tick: number;                              // = SSE id; sequence-check key
  simTimeMs: number;                         // epoch ms, Float64
  fleet?: FleetAggregate;                    // present when fields=aggregate
  homes?: ColumnarHomes;                     // present when fields=raw
  priceRtm?: number;                         // $/MWh, RTM print for this tick
  gap?: { missedFrom: number; missedTo: number };
}

interface ColumnarHomes {                    // worker re-frames rows → columns (§C.3.2)
  homeSlots: Uint32Array;                    // interned ring slots, not ids
  batteryPowerW: Float32Array;
  soc: Float32Array;                         // 0..1
  tempC?: Float32Array;
}

type RingChannel = 0|1|2|3|4|5;              // index into M (§C.2.2)
interface RingView {                         // SAB- or transferable-backed (§C.2.2)
  capacity: number; head: number;            // head via Atomics when SAB
  col(c: RingChannel): Float32Array;
  window(fromTick: number, toTick: number): Float32Array[] | null; // null = hole
}
C.9.3 DispatchCommand (client command layer)
TypeScript
interface DispatchCommand {                  // body of POST /v1/dispatch
  commandId: `cmd_${string}`;                // ULID, client-supplied, dedup key
  target: { fleetId?: FleetId; homeIds?: HomeId[];
            filter?: { mode?: HomeMode[]; socGt?: number };
            samplePct?: number };
  action:                                    // closed oneOf, discriminator "type"
    | { type: "charge_to"; kw: number; durationS?: number }
    | { type: "discharge_to"; kw: number; durationS?: number }
    | { type: "set_reserve_soc"; soc: number }
    | { type: "set_mode"; mode: HomeMode }
    | { type: "curtail_pv"; pct: number }
    | { type: "clear_override" };
  execution?: { latencyMs?: { uniform: [number, number] };
                timeoutS?: number; ramp?: "immediate" | "ramped" };
}

interface InFlightCommand {                  // layer (c) tracking record (§C.4.2)
  cmd: DispatchCommand; idempotencyKey: string;       // uuidv4, minted at intent
  status: "validating"|"sending"|"queued"|"in_flight"
        | "completed"|"completed_with_errors"|"failed"|"cancelled";
  targets: number; acked: number;            // from dispatch events / detail poll
  sentAtWall: number; sentAtTick: number;
  expectedDoneByTick: number;                // sentAtTick + max latency + timeout
  undoOf?: string;                           // §C.4.4 compensating link
  perTarget?: Record<HomeId, {               // filled on completion (detail fetch)
    status: "applied"|"partial"|"rejected"|"timeout";
    requestedKw?: number; appliedKw?: number;
    executedAtSimTime?: string; latencyMs?: number }>;
  traceId?: string;                          // from problem+json on failure
}
C.9.4 SaveGame (client meta over server snapshot)
TypeScript
interface SaveGameMeta {                     // IndexedDB, keyed by snapshotId (§C.5.4)
  snapshotId: `snap_${string}`;
  snapshotHash: string;                      // verify on every restore
  label: string; notes?: string;             // client-only — server has no label field
  createdAtWall: string; simTime: string; tick: number;
  scenarioId: string; scenarioName: string; fleetId: FleetId; homes: number;
  registryVersion: string;                   // provenance gate (§C.5.5)
  parentSnapshotId?: string;                 // branch lineage (savegame tree)
  branchOfScenarioId?: string;
  archivedPath?: string;                     // if .batsim.snap was exported
}
C.9.5 ScenarioBinding
TypeScript
interface ScenarioBinding {                  // mirrors POST /v1/scenarios body
  id?: `scn_${string}`;                      // absent pre-create (editor draft)
  name: string;
  time: { start: string; end: string; tickSeconds: number };
  prices:
    | { source: "replay"; replay: { dateRange: [string, string];
        market: "RTM"|"DAM"; settlementPoint: SettlementPoint } }
    | { source: "synthetic"; synthetic: { profile: string; volatility: number;
        seed: number } }
    | { source: "live" };
  ancillary?: { programs: AsProduct[]; dispatchModel: string };
  outages?: OutageWindow[];                  // { start, end, scope.loadZones[], probability }
  weather?: { source: "replay"|"synthetic"; dataset?: string; years?: number[] };
  seed: number;                              // surfaced + lockable in editor (§C.6.1)
  ui?: { seedLocked: boolean; lastActivateError?: ProblemJson };
}
C.9.6 BatsimTransport (the seam for WS/SSE/demo)
TypeScript
interface BatsimTransport {
  connect(): Promise<void>;
  subscribe(s: TelemetrySubscription): void;   // in-place; re-issue on stratum change
  onFrame: (f: TelemetryFrame) => void;        // into worker pipeline
  resumeFromTick?: number;                     // set by pipeline before reconnect
  kind: "ws" | "sse" | "replay";               // replay = demo mode (§C.7.4)
  close(): void;
}
interface TelemetrySubscription {
  fleetId?: FleetId; homeIds?: HomeId[];       // ≤500 when fields=raw (server cap)
  fields: "aggregate" | "raw";
  downsample: "1s" | "5s" | "15s";
}
C.10 Sequence Diagrams
C.10.1 Live dispatch round-trip
plain
User        Mode UI        CommandLayer       batsim /v1          TelemetryWorker     PartB/D
 |  select 5k homes, "discharge 5 kW"               |                  |                   |
 |----------------->| intent + precheck (registry)  |                  |                   |
 |                  | mint cmd ULID + idem-key      |                  |                   |
 |                  | POST /v1/dispatch ----------->| expand targets,  |                   |
 |                  |                               | draw jitter/home,|                   |
 |                  |                               | enqueue per home |                   |
 |                  |<----- 202 {queued,targets}----|                  |                   |
 |<-- "in flight" chip (server-confirmed) ----------|                  |                   |
 |                  |                               | tick N+k: apply  |                   |
 |                  |                               | per-home @jitter |                   |
 |                  |                               |-- SSE dispatch -->| ring markers      |
 |                  |                               |   + tick frames  |-- power curves -->| ripple/ack
 |                  |<-- poll status_url (fallback) |                  |   render, real jitter
 |                  | GET /dispatch/commands/{id} ->| per-target detail|                   |
 |<-- inspector: applied 4 988 / partial 12, latency scatter ---------|                   |
 |                  | reconcile: NO entity-cache write; truth arrives via telemetry       |
C.10.2 Time scrub beyond retention
plain
User        Scrubber       ClockEstimator     CommandLayer        batsim /v1          Rings
 | drag to T-3 days (beyond 24h raw / ring capacity) |               |                   |
 |----------------->| retention check: MISS          |               |                   |
 |                  | hand off to "re-simulate" flow >|              |                   |
 |                  |                                 | GET /v1/sim/snapshots            |
 |                  |                                 |<-- newest snap with sim_time<=T--|
 |                  |                                 | POST /v1/sim:pause               |
 |                  |                                 | POST /v1/sim/snapshots/{id}:restore
 |                  |                                 |<-- verify snapshot_hash ---------|
 |                  | pause estimator, "re-simulating"| POST /v1/sim:run-until {until:T} |
 |                  |                                 | (chunked per sim-day; SSE ticks  |
 |                  |<-- live progress via tick frames|  stream while it runs) --------->| refill
 |                  | resume estimator @ T            | PUT /v1/sim:speed (restore prior)|
 |<-- scrubber now replays within freshly-filled ring window ---------------------------|
C.10.3 Snapshot branch A/B compare (two-instance)
plain
User        CompareCtl     ClientA (stores+transport)   batsim-a        ClientB        batsim-b
 | pick savegame S, define deltaB (strategy cmds/seed)   |               |               |
 |-------------->| spawn B stack (factory stores)        |               |               |
 |               | POST /sim:snapshots/S:restore ------->|               |               |
 |               |<-- hash_A -----------------------------|               |               |
 |               | POST snapshot upload/restore ----------+-------------->+------------->|
 |               |<-- hash_B; ASSERT hash_A==hash_B; registry_version equal; seed equal* |
 |               | (*unless seed is the independent variable — recorded in CompareBundle)|
 |               | scenario A unchanged; POST /scenarios (deltaB) -------+------------->|
 |               | start both, matched speed ------------>| ticks       | ticks         |
 |               |<-- aggregate frames -- rings_A         |             | rings_B ------|
 |               | at checkpoints: GET /runs/{id}/settlement (both, same tick)           |
 |<-- CompareBundle{A,B,provenance_checks} → Part A/D side-by-side; ΔSOC/ΔP&L computed --|
 |               | any failed provenance check → "indicative only" badge with reason     |
C.11 Non-goals (this part)
No server-side changes except the two explicitly OPTIONAL proposals (MessagePack
telemetry subprotocol §C.3.2; snapshot upload for savegame import §C.5.4).
No client-side physics or price prediction; no interpolation across ring holes.
No multi-user/auth model beyond batsim's single bearer key [server §C.4].
Vendor-API mimicry surface (/v1/vendor-api/*) is never consumed by this UI.
Part D — Visual Scenes, Assets & Feedback Systems
Project: batsim 3D Web UI — Rust residential battery fleet simulator (ERCOT-only)
Scope of this document: art direction, design tokens, per-stratum scene specs (Z1–Z5), the unified energy-flow particle system, event/alert choreography, asset inventory & pipeline budgets, HUD information design, and optional audio.
Depends on: Part A (strata/mode model, AC vs DC coupling semantics), Part B (three.js/R3F renderer, instancing/LOD, glTF pipeline, 60 fps frame budget), Part C (telemetry ring buffers, field names, update cadences).
Audience: implementing AI agent. Every visual element below names its data binding, every asset has poly/texture budgets, every color is a hex token from §0.3. Do not invent colors, fields, or assets not listed here; extend tables instead.
0. Conventions
0.1 Units, cadence, signs
Table
Convention   Value
Power sign   + = discharge from battery / import from grid; − = charge / export. Sign is ALWAYS carried by the field, never by color alone.
SOC/SOH   percent, 0–100, float
Prices   USD/MWh (ERCOT native); HUD also shows ¢/kWh derived = lmp/10
Sim time   epoch seconds (sim.time_s); wall time from performance.now()
Default telemetry cadence   per-home power/SOC: 1 s; hub LMP (RTM): 5 min; SCED/system lambda: 5 s; settlement: 15 min intervals; weather front positions: 60 s; dispatch acks: event-driven
Ring buffer refs   all bindings below resolve through Part C buffer keys; visual systems must tolerate stale (flagged .stale=true) data by dimming to --text-dim
0.2 Telemetry field namespace (bindings used throughout Part D)
Table
Field   Type   Cadence   Drives (examples)
sim.time_s, sim.speed_mult, sim.scenario_name   int, float, string   1 s   dual clock, day-night cycle, top bar
ercot.hub.<HB_NORTH|HB_WEST|HB_SOUTH|HB_HOUSTON>.lmp_usd_mwh   float   5 min   Z1 hub chips, price lens
ercot.system_lambda_usd_mwh   float   5 s   KPI strip, audio hum pitch
ercot.operating_reserves_mw   float   5 s   scarcity detection, KPI
ercot.scarcity_active   bool (edge-triggered)   5 s   §4 scarcity choreography
ercot.four_cp.window_active, ercot.four_cp.current_rank   bool, int   15 min   4CP gauge
zone.<LZ_*>.load_mw, zone.<LZ_*>.lmp_usd_mwh, zone.<LZ_*>.outage_active   float, float, bool   5 s / 5 min / event   Z1 zone glow, outage lens
weather.front.<i>.{lat,lon,heading_deg,speed_kmh,cloud_cover_pct,temp_c}   struct   60 s   Z1 weather drift, PV dimming
fleet.cluster.<id>.{centroid_latlon,home_count,agg_power_kw,avg_soc_pct}   struct   5 s   Z1/Z2 cluster markers
home.<id>.battery.{soc_pct,soh_pct,power_kw,mode,cell_temp_c,efficiency_pct,cycles_cum,throughput_kwh_cum,degradation_pct}   struct   1 s   SOC rings, Z4/Z5 gauges
home.<id>.pv.power_kw   float   1 s   PV glint, Z4 PV path
home.<id>.load.power_kw, home.<id>.load.circuit.{hvac,ev,water_heater,pool,other}_kw   float   1 s   Z4 load bars
home.<id>.grid.{power_kw,outage_active}   float, bool   1 s / event   service-line particles, outage state
home.<id>.backup.{active,transfer_state} (idle|transferring|on_backup)   struct   event   §4 transfer flicker
home.<id>.oem.model (pw2|pw3|iq5p|iq10c|se400v|ecolinx|coreplus)   enum   static   asset variant selection
home.<id>.archetype (ranch_small|suburban_2story|estate_pool|townhouse|bungalow|infill_modern)   enum   static   Z3 house model selection
home.<id>.revenue.{cum_usd,interval_usd}   float   15 min   Z5 ledger, KPI revenue
dispatch.cmd.<id>.{target_set,t_issue_s,t_ack_s[],power_kw,mode}   struct   event   §4 fan-out ripple
settlement.interval.{idx,t_close_s,fleet_revenue_usd,cum_revenue_usd}   struct   15 min   §4 settlement tick, KPI
feeder.<id>.{outage_event_id,home_ids_ordered_by_distance}   struct   event   §4 rolling dark wave ordering
0.3 Design tokens (single source of truth)
All values are low-saturation, warm-neutral. Forbidden: blue→purple gradients, neon/saturated cyan-magenta, pure #000/#FFF, glow bloom >0.35 intensity.
Table
Token   Hex   Usage
--bg-base   #201C17   app background, deep space behind world
--bg-deep   #171410   vignette, Z1 sky-dome zenith at night
--surface   #2B261F   panel base
--surface-raised   #383227   cards, inspector headers
--surface-glass   #2B261F @ 72% alpha   floating HUD panels (with 12 px backdrop blur)
--hairline   #4D4536   1 px borders, dividers, chart gridlines
--text-primary   #EFE7D8   bone — headings, key numbers
--text-secondary   #B0A591   greige — labels, units
--text-dim   #7C7462   stale data, disabled
--terrain-base   #8B8371   Z1 ground albedo (warm grey-olive)
--terrain-elev   #A79B82   elevation highlight
--terrain-water   #5E6B66   desaturated slate-green water, NO blue
--slate-line   #6E7478   neutral infrastructure (feeders, roads)
--energy-discharge   #DFA33C   amber — battery discharge, export-to-grid family
--energy-discharge-deep   #9C6B21   amber shadow/edge
--energy-charge   #86A96B   sage — battery charging, solar family
--energy-charge-deep   #4F6B3C   sage shadow/edge
--energy-solar   #A8BE85   light sage tint — PV generation specifically
--energy-export   #C7903F   amber tint — grid export (discharge family, distinct tint)
--energy-charge-cvd   #7FA38A   CVD-safe alternate charge hue (teal-leaning sage), enabled by CVD toggle only
--alert   #C4452F   restrained signal red — scarcity, outage, faults ONLY
--alert-deep   #7E2B1D   alert edges/pulse trough
--warn-amber   #C77F2E   4CP window, approaching thresholds (not an emergency)
--price-p0 … --price-p4   #3A352C, #6E6247, #B98A3A, #DFA33C, #C4452F   5-stop price ramp (neg/low → scarcity)
--soc-s0 … --soc-s2   #DFA33C, #A8A06B, #86A96B   SOC ramp low→mid→full
--outage-dark   #0E0C09   unpowered state multiply tint
--dc-path   #B98A3A   DC conduit core (amber-brown)
--ac-path   #8F8574   AC conduit (warm grey, braided)
--pv-glass   #4A4F52   PV panel albedo (dark slate, not blue)
Colorblind safety (mandatory pair verification): discharge amber #DFA33C (L*≈70) vs charge sage #86A96B (L*≈61). Under protanopia/deuteranopia simulation (verify with Coblis or colorblind-sim CI step) the pair retains a ≥9-point L* separation, but hue separation is weak. Therefore: (1) state is NEVER carried by hue alone — charge flows animate toward the battery glyph, discharge flows away, export flows toward the grid glyph (§3); (2) SOC ring uses fill fraction + numeric label, not just ramp color; (3) a settings toggle swaps --energy-charge→--energy-charge-cvd which passes hue AND luminance under all three CVD types. CI gate: screenshot diff under simulated deuteranopia must keep all state distinctions legible.
1. Art Direction
1.1 Pillars
A living scale model of ERCOT. The whole state reads as a miniature diorama / model-railroad layout on a warm workbench: hand-placed buildings, slightly exaggerated silhouettes, visible "made-ness" (clean bevels, flat-ish shading, subtle paper-grain texture). Facilitates the Factorio/city-builder zoom fantasy (Part A) — every stratum feels like leaning closer into the same model, not teleporting to a different renderer.
Stylized realism, never photoreal, never cartoon. Low-poly geometry with premium finish: 1–2 mm-equivalent bevels on all hard edges, baked AO, flat or smooth-stepped shading (no normal-map noise at distance), restrained PBR (roughness 0.6–0.9, metalness ≈0 except small fixtures). No outlines, no cel bands, no squash-and-stretch, no emoji-faces. Proportions: houses ~1.15× vertical exaggeration, trees ~0.8×, so the scene reads as a model, not a toy.
Warm-neutral world, energy is the only color. The environment lives in bone/greige/slate (--terrain-*, --surface, --slate-line). Saturation budget: environment ≤25% sat; ONLY energy states (amber/sage), price ramps, and the rare --alert red exceed that. Scarcity red is a spice — if more than ~3% of screen pixels are --alert outside an actual scarcity/outage event, the design is wrong.
Data legibility over decoration. Every moving thing encodes a telemetry field (§0.2). Ambient motion that encodes nothing is limited to weather drift, foliage sway, and dust motes — all at ≤30% opacity of attention vs. data motion.
1.2 Palette application rules
Discharge/export family = ambers (--energy-discharge, --energy-export); charge/solar family = sages (--energy-charge, --energy-solar). Never cross-assign.
Alert red --alert is reserved for: scarcity pricing events, outages, hardware faults, 4CP critical hour flag. Marketing/decorative use is forbidden.
Price heatmap uses only the 5-stop --price-p* ramp; do not interpolate outside it.
Outage is expressed by REMOVING warm light (multiply toward --outage-dark), not by adding red; red appears only as the small outage glyph.
1.3 Day-night cycle (bound to sim time)
Sun/sky rig driven by sim.time_s mapped to a Central-Time solar model (fixed latitude 31°N blend, seasonal declination from sim date). Time-lapse is the default view of time passing — at high sim.speed_mult the cycle is a feature, not a bug.
Table
Phase (local solar)   Key light   Fill/ambient   Sky-dome   Notes
Day 08:00–16:00   warm white #F2E8D5, int 1.0, elev 55°   hemispheric bone/slate 0.45   --terrain-elev zenith→#CFC4AB horizon   PV glint active (§2.3)
Golden 16:00–19:00   #E8C07A, int 0.85, long shadows   0.35   #D9B77E horizon band   "diorama magic hour" default screenshot look
Dusk 19:00–20:30   #D98E4A, int 0.5, elev 8°   0.25, warm brown   #8A5A38→--bg-deep   warm dusk is the signature look; house windows begin warm glow (#E8C07A) bound to home.load.power_kw > 0.2
Night 20:30–06:00   moon slate #8E9298, int 0.18   0.12   --bg-deep   window glow + streetlights (small amber points) carry readability; energy flows get +15% emissive at night
Dawn 06:00–08:00   #E0B98A, int ramping   ramping   #C9A87E band   symmetric with dusk
Transitions are smooth 30-min sim-time blends. Scarcity sky pulse (§4.1) modulates the horizon band only, never the key light.
1.4 Typography
Table
Role   Family   Fallback stack   Required features
HUD/UI grotesk   IBM Plex Sans (preferred; Inter acceptable alternate)   system-ui, sans-serif   weights 400/500/600 only
Numeric/mono   IBM Plex Mono (preferred; JetBrains Mono alternate)   ui-monospace, monospace   weights 400/500; font-feature-settings:"tnum" forced on every numeric element app-wide
Prohibition   —   —   no italics in HUD; no all-caps body text; uppercase allowed only for 10 px micro-labels with +8% tracking
All numeric readouts (prices, kW, SOC, clocks, revenue) use the mono face with tabular numerals so ticking values never reflow. Negative values get a leading − (U+2212), colored per sign only where the number IS the state (else --text-primary).
1.5 Material & lighting rules (renderer handoff to Part B)
One shared 1k environment atlas + per-archetype 1k albedo/ORM atlases (§5); no per-asset unique 4k textures.
Shading: MeshStandardMaterial with baked AO; emissive reserved for windows, status LEDs, flow particles.
Shadows: single directional CSM at Z3–Z5, static baked shadow blobs at Z1–Z2 (shadow maps off past 2 km camera height).
Bloom: threshold 0.85, strength ≤0.35 — just enough to make energy flows read at night; never a neon look.
Post: warm filmic tonemap (ACES), vignette 0.15 --bg-deep, fine film grain 0.02 for the "model photograph" feel.
2. Scene-by-Scene Specification (Z1–Z5)
Camera/transition math, picking, and culling live in Part B; this section defines WHAT exists, WHAT it looks like, and WHAT it binds to. "Lens" = the map-wide data overlay mode selected from the top bar (price | soc | outage | revenue | none).
2.1 Z1 — ERCOT Grid Map of Texas
Composition. Tilted orthographic-feel perspective (fov 28°, pitch 42°, yaw free ±60°). Texas fills ~80% of frame at entry. The state is a raised diorama slab: terrain mesh sits on a visible 40-unit-thick plinth edge (--surface), like a model on a table; beyond the state line the world fades into --bg-base void with a soft vignette.
3D content.
Table
Element   Geometry   Material/finish   Binding
Texas terrain   1 mesh, ~45k tris LOD0 (quantized 90 m DEM, elevation exaggerated ×3, smooth-stepped)   --terrain-base albedo, elevation tint lerp to --terrain-elev, paper-grain overlay 4%   none (static); Gulf/coast water plane --terrain-water with 0.2 Hz sine shimmer
Load-zone regions   8 flat decal meshes draped on terrain (LZ_HOUSTON, LZ_NORTH, LZ_SOUTH, LZ_WEST, LZ_RAYBN, LZ_CPS, LZ_AEN, LZ_LCRA)   fill --surface-raised @ 18% alpha; boundary = 2 px-equivalent ribbon, emissive --energy-discharge-deep @ 0.15 "soft glow"   zone.<LZ_*>.lmp_usd_mwh tints fill via price ramp when lens=price; zone.<LZ_*>.outage_active → fill to --outage-dark
Hub price chips   DOM (not 3D), anchor-projected at HB_NORTH, HB_WEST, HB_SOUTH, HB_HOUSTON   chip = --surface-glass pill, mono price, delta arrow   ercot.hub.<*>.lmp_usd_mwh; color via --price-p* ramp; tick animation 150 ms on update
Fleet cluster markers   instanced discs + vertical hairline pin, 1 draw call (max 512 instances)   disc --surface-raised with inner ring   fleet.cluster.<id>.*: ring fill = avg_soc_pct (SOC ramp), ring pulse rate ∝ |agg_power_kw|, hue = sign (amber discharge/sage charge), pin height ∝ log(home_count)
Weather fronts   3–6 translucent billboard planes + slow noise-displaced fog shader   --text-secondary @ 12% alpha, no saturation   weather.front.<i>.* — position lerp each 60 s tick, drift speed_kmh × sim speed, cloud_cover_pct also feeds PV dimming factor consumed at Z3/Z4
Scarcity sky tint   sky-dome horizon band shader uniform   --alert @ ≤10% alpha pulse   ercot.scarcity_active (see §4.1)
Transmission hint lines   faint great-arc lines zone→zone (decorative-topological, not real topology)   --slate-line @ 15%   none; hidden when lens≠none to reduce noise
HUD / DOM overlays (Z1). Persistent top bar, left rail, KPI strip, bottom timeline (§6.1 wireframe); lens selector segmented control top-right; event ticker (single-line, above timeline); hub chips as above; tooltip on hover of cluster/zone (DOM, 150 ms delay).
Lens overlays.
Table
Lens   Visual treatment   Binding
price   zone fills lerped into --price-p* ramp by zone.*.lmp_usd_mwh; legend chip bottom-left with ramp stops at −10/25/75/250/1000+ $/MWh   zone LMP; hub chips always visible
soc   cluster markers enlarged ×1.3, ring = avg_soc_pct; zone fills neutral   fleet.cluster.*.avg_soc_pct
outage   zones with outage_active darken to --outage-dark + small --alert glyph; affected cluster rings show broken-arc style   zone.*.outage_active, home.*.grid.outage_active aggregated
revenue   cluster disc fill = revenue heat (bone→amber ramp, fleet.cluster revenue aggregated from home.*.revenue.cum_usd); ticker shows settlement.interval.cum_revenue_usd   settlement + home revenue
Idle ambient motion. Weather drift (only unbound-but-allowed motion), cluster ring breathing (bound, see above), terrain cloud shadows drifting with fronts (multiplies terrain albedo ×0.92 under cloud_cover_pct>50), water shimmer. Nothing else moves at Z1.
Update cadence budget (Z1): 5 s data → all markers tweened (no pops); LMP 5 min → chip tick; terrain/static: zero per-frame cost.
2.2 Z2 — City / Territory Clusters
Composition. Mid-zoom transitional stratum (Part B blends Z1↔Z2↔Z3 by camera height, 150 m–2 km). The single fleet cluster under the camera dissolves: instanced cluster disc cross-fades into a procedurally scattered city-block arrangement of its member homes. Dissolve = markers scale down + fade out over 400 ms while block layout scales up from 0.85 + fades in; home world positions are deterministic from hash(home.id) so the dissolve never shuffles.
3D content.
Table
Element   Geometry   Material   Binding
City blocks   procedural parcels: 6–20 lots per block, blocks snapped to a coarse seeded street grid (seed = scenario_id)   streets --slate-line @ 30%, parcels --surface-raised   layout static per scenario; lot assignment from home.<id> cluster membership
Miniature homes (LOD2)   instanced archetype shells (§5), flat-shaded, no interiors   bone albedo variants ±5%   home.<id>.archetype; roof PV quad shown if pv configured
Substations   1–3 per cluster, small fenced yard prop with transformer cylinders   --slate-line, tiny amber status LED   status LED: aggregate home.*.grid.outage_active fraction (dark if >50% out)
Aggregate flow lines   spline feeders from each block centroid → substation; particles per §3   --slate-line conduit   particle rate/hue from Σ home.*.grid.power_kw of that block (sign → direction: import toward homes = sage-neutral? NO — see rule below)
Cluster label   DOM chip   --surface-glass   fleet.cluster.<id>.home_count, agg_power_kw, avg_soc_pct
Flow direction rule (applies everywhere): particles travel in the direction of ENERGY movement. Grid→home (import) particles move toward homes and use --slate-line-neutral dots with sage tint only if the energy is charging batteries; home→grid (export) particles move toward the substation in --energy-export. At Z2 the aggregate hue is the net sign of block power.
HUD / overlays. Same chrome as Z1; lens overlays persist across the dissolve (price lens now colors block parcels instead of zone fills, interpolated from zone LMP — documented so agents don't drop the lens mid-zoom). Hover a block → DOM tooltip with block aggregates; click → camera dives to Z3 on that street.
Idle ambient motion. Feeder particles (bound), substation LED blink 1 Hz when exporting (bound), rooftop PV glint sweep when cloud_cover_pct<30 and solar phase (bound), faint rooftop heat-shimmer on estates with pool running (bound to load.circuit.pool_kw>0).
2.3 Z3 — Neighborhood Streets
Composition. Street-level oblique view (pitch ~35°, camera 15–60 m). One procedural neighborhood tile (~200×200 m, 12–40 homes) fully dressed; adjacent tiles render as LOD2 shells. This is the stratum where the "living model" feel peaks: warm window glow at dusk, pool water shimmer, tiny service drops.
Procedural generation (deterministic). Street graph = seeded grid with irregular offsets (seed = scenario_id + tile coords; L-system lite: main arterials, curving residential loops, cul-de-sacs 20%). Lot fill = archetype-weighted placement matching batsim load archetypes exactly:
Table
batsim archetype id   House model (§5)   Defining props   Typical systems
ranch_small   house_ranch   carport, small lot   PV optional, PW2 or IQ5P
suburban_2story   house_suburban   gabled roof, garage   PV common, PW3 / SE400V
estate_pool   house_estate   pool plane (shimmer shader), pool pump shed   PV large, ecoLinx / stacked IQ10C
townhouse   house_townhouse   row-shared walls, rear alley   small battery or none
bungalow   house_bungalow   porch, hip roof   IQ5P typical
infill_modern   house_modern   flat roof, parapet   PV + PW3
3D content & per-home status.
Table
Element   Spec   Binding (per home.<id>)
SOC ring   floating flat ring, 1.2 m radius, hovering at 4 m over each battery-equipped home; 24-segment torus, thickness 0.06 m + 0.05 m×|battery.power_kw|/10 kW; fill arc = battery.soc_pct; ramp --soc-s0→s2; 1 s updates, 250 ms tween; numeric % in mono only within 30 m camera distance (DOM micro-label)   battery.soc_pct, battery.power_kw
Service-line flow   spline from house meter point → pole transformer (shared by ≤4 homes); particles per §3: speed 1.5 m/s × (|grid.power_kw|/5 kW, clamped 0.2–3×), direction = energy direction, color: export --energy-export, import-to-charge --energy-charge, import-to-load --slate-line-neutral @ 60%   grid.power_kw, battery.power_kw (for hue disambiguation)
Rooftop PV   instanced panel quads on roof faces per archetype PV config; subtle glint sweep during solar phase when producing; panels dim ×0.6 under weather front   pv.power_kw (glint intensity ∝ kW), weather.front.*.cloud_cover_pct
Window glow   emissive quads, warm #E8C07A; count/brightness ∝ load; ON only dusk/night phases   load.power_kw
Outage state   house multiply-tints to --outage-dark, window glow off, streetlight segment off; if backup.active: battery icon (small DOM glyph or 3D plaque) pulses amber at 1 Hz over the home and interior critical-loads glow stays ON at 40% — the only lit house on a dark street is the backup story, make it readable from 60 m   grid.outage_active, backup.active
Pole transformer   shared prop per ≤4 homes; tiny LED: amber pulse while exporting cluster   Σ member grid.power_kw
Street furniture   streetlights (dusk+), trees ×3 species instanced, mailboxes/driveways procedural   none (ambient only)
HUD / overlays (Z3). Chrome per §6.2; right inspector opens on home click showing the Z4-entry summary card (SOC, power, OEM model, revenue); lens overlays persist (price lens recolors SOC ring halo — ring keeps SOC fill, outer 2 px halo takes price ramp, documented to avoid double-encoding confusion); dispatch mode (Part A) shows selectable-home outlines in --energy-discharge hairline.
Idle ambient motion. Bound: particles, PV glint, pool shimmer (load.circuit.pool_kw), HVAC condenser fan spin (load.circuit.hvac_kw>0.3), EV charge port glow (load.circuit.ev_kw). Unbound allowance: tree sway (vertex noise, ≤0.5°), rare bird flyover (1 per 90 s, may be disabled in perf mode).
2.4 Z4 — House Cutaway ("Dollhouse")
Composition. Single home, camera 8–15 m, pitch 25°. On entry the roof lifts (translate +1.8 m, 600 ms ease-out, slight 2° tilt) and parks floating above — the classic dollhouse section; front wall clips away via clipping plane. Rooms are IMPLIED: floor slab per room, 1.1 m partial-height interior walls, furniture silhouettes (flat 2.5D cutouts, --surface-raised), no interior clutter. Scale trick: interior equipment rendered at 1.3× so devices read at this zoom.
3D content & bindings.
Table
Element   Spec   Binding
Battery unit(s)   OEM-correct model (§5.2) placed per OEM norms: PW3 wall-mounted in garage; Enphase IQ 5P/10C cabinet on exterior side wall; SolarEdge 400V tower in garage corner; sonnen ecoLinx/Core+ cabinet utility room. Small status LED per unit: breathing sage when charging, amber when discharging, off when idle, --alert 2 Hz on fault   battery.mode, battery.power_kw sign
Inverter   string inverter wall unit OR microinverter nubs under PV panels (per OEM config); conversion-node glow at DC→AC junction   static config; glow intensity ∝ |battery.power_kw| + pv.power_kw
Critical-loads sub-panel   small grey cabinet beside main panel; its circuits stay lit during outage   backup.active
Wiring flows (the Part A AC/DC story — MUST read differently)   DC segments (battery→inverter on DC-coupled systems; PV strings→inverter): single solid conduit, --dc-path core, SLOW dense particles (§3 profile dc), no braid. AC segments (inverter→panel, panel→circuits, microinverter→trunk cable): twisted tri-line conduit --ac-path, FASTER sparse particles (profile ac). AC-coupled systems (Enphase, AC-coupled PW): PV path enters panel as AC directly — visibly NO shared DC run to the battery; battery has its own AC run + small integrated-inverter glow at the unit. Conversion node (inverter) is the only place DC and AC conduits meet; it pulses at the net conversion power.   oem.model (topology selection), pv.power_kw, battery.power_kw, grid.power_kw for per-segment rate/direction
Live load bars   5 vertical DOM-anchored bars floating over the panel (or 3D slabs), one per circuit: HVAC, EV, water heater, pool, other; height ∝ kW (0–8 kW scale), --text-primary fill, HVAC/EV get tiny glyphs; bar flashes 100 ms on step changes >1 kW   load.circuit.{hvac,ev,water_heater,pool,other}_kw
Backup transfer flicker   see §4.3 — whole-interior warm light dips ~1 s on transfer   backup.transfer_state
SOC ring   persistent from Z3, re-anchored beside the battery unit   battery.soc_pct
HUD / overlays (Z4). Chrome per §6.2 with inspector showing full home telemetry table; "Enter device view (Z5)" button on battery click; lens overlays collapse to a compact chip row (full map lenses are meaningless at this scale) — documented behavior, not a bug.
Idle ambient motion. Bound only: wiring particles, load-bar motion, LED breathing, ceiling-fan silhouette rotation if load.circuit.other_kw>0.5 (whimsy budget: this one joke, that's all).
2.5 Z5 — Device Focus (One Battery Unit)
Composition. Studio turntable: the OEM unit centered on a --surface plinth, --bg-base void, single warm key + rim light (the "product photo of a miniature" look). Slow auto-orbit 4°/s, pauses 6 s on user input. Scroll/pinch triggers exploded view: shell slides +0.4 m, module pack separates into 2–4 module blocks, a stylized cell-stack hint (instanced rounded-prisms, NOT real cell counts) spreads 0.15 m — 700 ms spring. Section mode alternates via toggle: front quarter cutaway with clipping plane.
Industrial-design cues per OEM (evocative, trademark-safe). Hard rule: no logos, no brand wordmarks, no exact dimension replication; capture silhouette language only. HUD label uses generic text ("Wall-mounted integrated battery, 13.5 kWh class") unless the sim exposes oem.model, in which case the plain model family string is allowed in the inspector, never on the 3D asset.
Table
oem.model   Silhouette cues to model   Distinctive detail (non-trademarked)
pw3   flat, thin rectangular wall slab, near-flush mount; slightly taller than wide   integrated-inverter step/hump along the TOP edge; single slim vertical LED slit, lower-left; conduit knockouts on side
pw2   same slab language, gently bowed face, no top hump (separate inverter box beside it)   rounded corner radius noticeably larger than pw3
iq5p / iq10c   white vertical floor cabinet, width ≈ 2× depth   louvered lower-third base (vent slots), horizontal door seam(s) — 5P: one seam pair; 10C: double-wide two-cabinet arrangement
se400v   dark charcoal floor TOWER of visibly stacked modules   3–4 module seams, slim vertical status slot on the tower's upper third, small separate dark inverter cube beside
ecolinx / coreplus   minimalist matte light cabinet, flush door, no visible fasteners   recessed horizontal handle line near top, one small circular status light, base plinth shadow gap
Info panels (DOM, docked right; mono numbers, §1.4). Every gauge names its binding; all 1 s cadence except where noted.
Table
Panel element   Visual   Binding
SOC gauge   270° arc dial, fill --soc-s* ramp, mono % center   battery.soc_pct
SOH gauge   slim arc under SOC, bone fill, mono %   battery.soh_pct
Cell temp   horizontal thermometer strip, bone→--alert ramp above 45 °C   battery.cell_temp_c
Power in/out   bipolar horizontal bar, center-zero: right/amber = discharge, left/sage = charge; mono kW   battery.power_kw
Efficiency now   small donut, % of round-trip realized this interval   battery.efficiency_pct
Mode badge   pill chip: charging / discharging / idle / backup / dispatched (from battery.mode), dispatched adds --warn-amber dot   battery.mode
Command history mini-log   last 8 rows: HH:MM  −2.5 kW  ack 0.8 s; 15 min cadence entries collapsed   dispatch.cmd.* filtered to this home
Degradation bar   full-width thin bar: capacity fade --text-dim, throughput mono label X MWh · Y cycles   battery.degradation_pct, battery.throughput_kwh_cum, battery.cycles_cum
Revenue chip   cum + this-interval revenue   revenue.cum_usd, revenue.interval_usd
Idle ambient motion. Turntable orbit, LED breathing, internal flow particles visible through section mode along DC bus (bound to battery.power_kw), cooling-fan spin cue + faint heat shimmer above unit when cell_temp_c > 35 (bound).
3. Energy-Flow Particle System (Unified Primitive)
One primitive everywhere: FlowSpline — a Catmull-Rom spline with a particle train. Every energy visualization in Z1–Z5 is an instance of it; no stratum may invent its own flow visual.
3.1 Parameters
Table
Param   Type   Semantics
rate   particles/s, 0–60   ∝ |power|: `rate = clamp(6 ×   kW   , 0, 60)`; zero power → 0 particles (no fake motion)
speed   m/s world   profile-based (below) × `clamp(   kW   /5, 0.2, 3)`
color   token   from sign+context rules: discharge --energy-discharge, charge --energy-charge, export --energy-export, neutral import --slate-line@60%, DC conduits tint toward --dc-path, solar origin --energy-solar
density   spacing px along spline   dense (DC) 12 px / normal 24 px / sparse (AC) 40 px
direction   +1 / −1   ALWAYS the physical energy direction; this is the primary CVD-safe encoding (§0.3)
dot_size   world px   3 (Z1–Z2), 2 (Z3), 1.5 (Z4–Z5)
fade   head/tail alpha   15% head fade, 40% tail fade
3.2 Implementation (handoff to Part B)
One instanced-quad system, one draw call per stratum. Per-instance attributes: spline_id, phase (0–1), speed, color_idx (palette LUT, never per-instance RGB), size. Splines baked into a SplineLUT texture (256 samples/spline, RGBA32F: pos.xyz + arclength); vertex shader samples LUT by fract(phase + t × speed / length) — zero CPU per-frame updates.
Soft dot sprite: 64 px radial gradient, alpha-blend (NOT additive) with emissive boost 1.3 at night only — keeps the warm, non-neon look under the bloom cap of §1.5.
Particle pass budget: ≤0.5 ms GPU at 1080p, ≤8 192 live particles total (frame-budget tie-in per Part B).
3.3 Per-stratum caps & profiles
Table
Stratum   Splines   Max live particles   Profile
Z1   zone↔zone hint lines (decorative, lens=none only) + cluster pins   2 000   sparse, slow 8 m/s
Z2   block→substation feeders   4 000   normal 6 m/s
Z3   service lines (≤40 visible × ≤24 dots) + PV→meter runs   3 000   normal 1.5 m/s (street scale)
Z4   ≤8 conduit segments (DC dense / AC sparse)   600   dc: 0.4 m/s dense; ac: 1.2 m/s sparse
Z5   internal DC bus (section mode only)   200   dc 0.2 m/s
Global governor: when total requested >8 192, shed in order Z1 decorative → Z3 off-screen street segments → Z2 minor feeders → never shed the selected home's flows.
3.4 Reduced-motion fallback (accessibility, mandatory)
prefers-reduced-motion or settings toggle: particle trains replaced by static chevron arrows stamped every 48 px along the spline, pointing in direction, same color tokens, plus a mono numeric label of kW at the spline midpoint (Z3+). No scrolling, pulsing, or drifting anywhere in the UI; day-night cycle pauses at current sim lighting (sun position still correct, just not animated continuously — it steps on zoom/scene changes).
4. Event & Alert Choreography
General rules: (1) every event has a single authoritative trigger from §0.2 — never trigger off a derived visual; (2) all pulses use cosine-ease, never linear blink; (3) alert red budget (§1.1) applies DURING events; (4) each event lists a dedup key — replays of the same key within the cooldown window are ignored (telemetry flapping must not cause UI strobe); (5) at high sim.speed_mult (>32×), all choreography compresses to ticker-line entries only (no map animations) — documented behavior.
4.1 Scarcity price spike
Table
Trigger   rising edge ercot.scarcity_active OR any ercot.hub.*.lmp_usd_mwh crossing $1,000/MWh upward
Visual layers   (a) Z1 sky-dome horizon band tints --alert @ 8% alpha, 2 pulses × 1.2 s; (b) affected zone boundary ribbons shimmer (emissive 0.15→0.5 sine, 3 s) — no fill flash; (c) hub chip border goes --alert, price flips mono-red, chip scales 1.0→1.06→1.0; (d) HUD ticker line: SCARCITY · HB_HOUSTON $4,381/MWh · ORDC adder active; (e) KPI strip LMP exposure cell inverts to --alert text on --surface-raised
Duration   5 s total choreography; persistent state (red price) until LMP < $500
Cooldown/dedup   key scarcity:{interval_15m}; max 1 full pulse sequence per 60 s wall
Sound (P2)   low two-tone chime, −18 dB, optional
4.2 Grid outage onset (incl. correlated rolling dark wave)
Table
Trigger   zone.<LZ>.outage_active rising edge (bulk) or home.<id>.grid.outage_active (single)
Visual layers   (a) Z3: correlated outage plays as a rolling dark wave — homes extinguish in feeder.<id>.home_ids_ordered_by_distance order, 80 ms stagger per home (a 40-home feeder sweeps in ~3.2 s); per home: window glow off, house multiply→--outage-dark over 300 ms, streetlight segment off; (b) backup homes counter-signal: 400 ms after going dark, battery plaque pulses amber 1 Hz and critical-loads glow returns at 40% (§2.3) — the wave visually "skips" then half-relights resilient homes; (c) Z1: zone fill darkens, small --alert outage glyph appears on zone centroid; (d) ticker: OUTAGE · LZ_NORTH feeder F-118 · 214 homes · 61 on backup
Duration   wave 3.2 s max; persistent dark state until outage_active clears (reverse wave, same stagger, on restore)
Cooldown/dedup   key outage:{feeder.outage_event_id}; restore is a separate key
Sound (P2)   soft low thump + hum dropout (§7), optional
4.3 Backup transfer flicker
Table
Trigger   home.<id>.backup.transfer_state transitions idle→transferring (and transferring→on_backup)
Visual layers   Z4/Z5 (or Z3 if home selected): interior warm light dips to 15% for 0.9–1.2 s (matches real transfer times; use measured transfer_state dwell, floor 0.5 s for readability), then restores; battery LED flips to amber breathing; sub-panel circuits stay lit — the dip, not darkness, tells the transfer story. Accompanying mono micro-label: transfer 0.9 s
Duration   data-driven, clamp 0.5–2.0 s
Cooldown/dedup   key transfer:{home_id}:{edge_id}; no cooldown needed (physical event), but identical edges within 3 s are dropped
Sound (P2)   faint relay click, −24 dB
4.4 Dispatch command fan-out ripple
Table
Trigger   new dispatch.cmd.<id> with t_issue_s (stream event)
Visual layers   (a) Z1/Z2: expanding ring wave from the ops point (scenario-defined; default state centroid) at 250 km-equivalent/s — reaches targets in 0.4–1.5 s of visual time; wave is a hairline --energy-discharge ring @ 40% alpha; (b) each targeted home/cluster marker plays an ack tick: 120 ms scale-pop + small check glyph when its t_ack_s[] entry arrives — a visible wave of acknowledgments sweeping the fleet; (c) targeted SOC rings flash a 2 px --warn-amber halo for 2 s; (d) ticker: DISPATCH · −2.5 kW × 1,204 homes · 98.1% ack · median 0.7 s; (e) Z5 command mini-log prepends the row
Duration   wave ≤2 s; ack ticks as they arrive, trailing window 10 s then summary chip
Cooldown/dedup   key dispatch:{cmd.id}; overlapping commands offset new waves by +150 ms and alternate ring dash pattern so two waves never z-fight
Sound (P2)   soft tick per ack, rate-limited to 8/s, optional
4.5 Settlement interval close
Table
Trigger   settlement.interval.t_close_s (new idx observed)
Visual layers   (a) KPI revenue counter ticks from old→new cum_revenue_usd over 1.5 s with mono tabular digits (odometer ease-out, no digit reflow thanks to tnum); delta shown +$412.07 / 15 min; (b) bottom timeline stamps a settlement marker diamond at t_close_s; (c) revenue lens (if active) re-heats cluster discs with 500 ms lerp; (d) Z5 revenue chip updates
Duration   1.5 s
Cooldown/dedup   key settlement:{idx}
Sound (P2)   muted cash-register-adjacent "tink", −24 dB, optional (one per interval max — at high sim speed suppressed by the >32× rule)
4.6 4CP coincident-peak window (ERCOT-specific, bonus)
Table
Trigger   ercot.four_cp.window_active rising edge
Visual layers   KPI 4CP gauge sweeps to --warn-amber, shows current_rank (#2 this month); ticker: 4CP WINDOW · June peak forming · curtailment advised; no map pulse (amber family, deliberately quieter than scarcity)
Duration/cooldown   persistent while window active; key 4cp:{month}
5. Asset List & Pipeline
5.1 Global budgets (alignment with Part B, <15 MB initial load)
Table
Bucket   Budget   Notes
Initial-download total   ≤ 15 MB   Z1 + Z2 + Z3-LOD1 + all HUD; Z4/Z5 interiors stream on first dive (≤ 4 MB more)
Geometry (Draco-compressed)   ≤ 4.5 MB   all LODs baked into per-asset glb LOD chain
Textures (KTX2/BasisU)   ≤ 8 MB initial   shared atlases; UASTC for UI-ish crispness on chips? No — chips are DOM. ETC1S for albedo, UASTC for ORM
Audio (P2, lazy)   ≤ 1.5 MB   streamed, off by default
5.2 Asset inventory
Poly = triangles at LOD0/LOD1/LOD2. Tex = contribution to shared atlases (no unique per-asset maps unless noted). Src: P = procedural (generated at build or runtime from parameters), B = Blender-modeled, exported glTF.
Table
Asset   Type   Poly L0/L1/L2   Tex   Variants   Src
terrain_texas   terrain slab + plinth   45k / 12k / 3k   1k albedo + 1k elev-tint ramp   1   P (DEM-quantized)
zone_ribbon_set   8 boundary ribbons + fills   8k / 2k / 0.5k   none (unlit emissive)   8 zones   P (from GeoJSON)
hub_pylon   marker pylon for 4 hubs   400 / 120 / 40   shared env   1   B
cluster_marker   instanced disc + pin   96 / 48 / 16   none   1 (instanced ×512)   P
weather_billboard   fog plane + shader   2 / 2 / 2   512 noise (shared)   6   P
street_tile_set   road segments, sidewalks, driveways, cul-de-sac cap   2k/tile / 0.8k / 0.2k   1k road atlas   12 segments   P
house_ranch   archetype shell + cutaway interior   6k / 1.5k / 350   1k atlas A   4 colorways   B
house_suburban   〃   8k / 2k / 400   1k atlas A   4   B
house_estate   〃 + pool plane + pump shed   10k / 2.5k / 450   1k atlas B   3   B
house_townhouse   row module (joinable)   5k / 1.2k / 300   1k atlas A   4   B
house_bungalow   〃   5.5k / 1.4k / 320   1k atlas A   3   B
house_modern   〃   6k / 1.5k / 340   1k atlas B   3   B
pv_array   instanced panel quad + frame   24/panel / 12 / 4   512 PV (shared, --pv-glass)   3 tilt kits   B
pole_transformer   pole + can + service mast   800 / 250 / 60   shared props 512   2   B
service_line   spline conduit (meter→pole)   16/seg / 8 / 4   none   —   P
substation   fenced yard, 2 transformers, bus bars   4k / 1k / 250   shared props 512   2 sizes   B
feeder_line   Z2 aggregate conduit spline   16/seg   none   —   P
batt_pw3   Z5/Z4 unit   7k / 1.5k / 250   512 dedicated   1   B
batt_pw2   〃 + separate inverter box   6.5k / 1.4k / 250   512 dedicated   1   B
batt_iq5p   white cabinet, louvered base   6k / 1.3k / 220   512 dedicated   1   B
batt_iq10c   double-wide cabinet   8k / 1.8k / 300   shares iq5p tex   1   B (2× iq5p shell variant)
batt_se400v   dark stacked tower + inverter cube   7k / 1.6k / 260   512 dedicated   3–4 module heights   B
batt_ecolinx   minimalist cabinet   5.5k / 1.2k / 200   512 dedicated   ecolinx/coreplus trim   B
inverter_string   wall unit   1.5k / 400 / 80   shared props   2   B
inverter_micro   per-panel nub (instanced)   60 / 30 / 10   shared props   1   B
subpanel_critloads   small cabinet + breaker hint rows   900 / 250 / 60   shared props   1   B
load_hvac   outdoor condenser, fan spins   1.2k / 300 / 60   shared props   2   B
load_ev   car silhouette + wall charger   3k / 800 / 150   shared props   3 silhouettes   B
load_waterheater   cylinder   400 / 120 / 40   shared props   1   B
load_poolpump   small pump + pad   500 / 150 / 40   shared props   1   B
furniture_silhouettes   flat 2.5D cutout set (bed, sofa, table…)   ~40 each   none (vertex color)   14   P
tree_set   3 species, vertex-sway rigged   900 / 300 / 60   512 foliage (alpha)   3×2 seasons   B
streetlight   pole + warm head   300 / 90 / 30   shared props   2   B
flow_dot_sprite   64 px radial sprite   —   64 px (shared)   1   P
chevron_arrow   reduced-motion marker   12   none   1   P
Rollup check: LOD0 geometry ≈ 190k tris worst-case single stratum (Z3 tile) — within Part B instancing budget; compressed initial payload ≈ 12.8 MB (4.1 geo + 7.6 tex + 1.1 misc) ≤ 15 MB ✔.
5.3 LOD tiers & transitions
Table
Tier   Screen-space size   Used at   Shadow   Notes
LOD0   >25% screen height   Z4/Z5 focus, Z3 selected home   cast+receive   full detail, interior included for houses
LOD1   2–25%   Z3 neighborhood, Z2 near blocks   cast only   shells + props
LOD2   <2% or >150 m   Z2 far, Z1   baked blob   silhouette shells, interior stripped
Crossfade (dither) transitions over 200 ms, hysteresis 10% to prevent LOD flapping; selection forces LOD0 regardless of size.
5.4 Pipeline & naming (per Part B)
Blender 4.x → glTF-Binary export (+Y up, apply transforms, no cameras/lights).
gltf-transform chain: draco (quantize pos 14/norm 10/uv 12) → ktx2 (albedo ETC1S q=80, ORM UASTC) → atlas into the shared 1k sets of §5.2 → lod-bake writes _lod1/_lod2 meshes into the same .glb as EXT_mesh_lod nodes.
CI gates: per-asset poly/texture budgets from §5.2 enforced by script; CVD screenshot check (§0.3); total-initial-payload check ≤15 MB.
Naming: batsim_<cat>_<name>_v<NN>.glb (cat ∈ terrain|infra|house|batt|load|prop|fx); textures <asset>_<albedo|orm|emissive>_<512|1k>.ktx2; splines/materials by token name only — hex values never hardcoded in shaders, always from the token LUT.
Runtime: Z1 pack preloaded; Z3 archetype atlases streamed per first tile visit; Z4 interiors + Z5 battery glbs prefetched on Z3 selection (predictive, ≤200 ms budget).
6. HUD & Information Design
All HUD is DOM (crisp text, cheap redraws), anchored over the WebGL canvas; 3D-anchored labels use projection with 8 px offset and hide when occluded behind camera-facing geometry. Panels: --surface-glass, 1 px --hairline border, 8 px radius, 12 px backdrop blur, shadow 0 4px 16px rgba(0,0,0,0.35). 4 pt spacing grid (4/8/12/16/24/32). Min tap target 32 px.
6.1 Z1 wireframe (map strata chrome)
plain
┌──────────────────────────────────────────────────────────────────────────────┐
│ TOP BAR (--surface-glass, 48px)                                              │
│ ◈ batsim │ scenario: ercot_heatwave_aug │ SIM 2025-08-14 16:03:12 ─ 8× ▶⏸ │ │
│                                        WALL 14:22:41 │ lens:[price|soc|outage│
│                                                      |revenue|none]          │
├──────┬───────────────────────────────────────────────────────────┬───────────┤
│ LEFT │                                                           │ RIGHT     │
│ RAIL │                                                           │ INSPECTOR │
│ 48px │            Z1 TEXAS DIORAMA VIEWPORT                      │ (context) │
│ [bld]│                                                           │ zone:     │
│ [insp│   (weather drift · cluster rings breathing · hub chips)   │ LZ_NORTH  │
│ [dsp]│                                                           │ LMP $41.2 │
│ [scn]│                                                           │ load 31GW │
│      │                                                           │ reserves  │
│      │        ┌ hub chip ─ HB_NORTH  $41.20/MWh ▲0.8 ┐           │ 3,412 MW  │
│      │                                                           │ ▸ drill   │
├──────┴───────────────────────────────────────────────────────────┴───────────┤
│ KPI STRIP (32px): FLEET 184 MW / 962 MWh │ aggSOC 63% │ LMP exp $-4.2k/h │  │
│                     4CP [#2 Jun ●] │ revenue cum $128,412.07 (+$412.07)      │
├──────────────────────────────────────────────────────────────────────────────┤
│ BOTTOM TIMELINE (96px): ◀ scrubber ═══●═════ price chart (mono y) ══ ▶       │
│                         markers: ◆scarcity ▼dispatch ◆settle  ▓outage        │
│ TICKER (20px): SCARCITY · HB_HOUSTON $4,381/MWh · ORDC adder active          │
└──────────────────────────────────────────────────────────────────────────────┘
6.2 Z3/Z4 wireframe (neighborhood / house chrome)
plain
┌──────────────────────────────────────────────────────────────────────────────┐
│ TOP BAR (same persistent bar; lens chips collapse to compact row at Z4)      │
├──────┬───────────────────────────────────────────────────────────┬───────────┤
│ RAIL │                                                           │ INSPECTOR │
│      │        Z3 STREET / Z4 DOLLHOUSE VIEWPORT                  │ home 0142 │
│      │                                                           │ PW3 · 13.5│
│      │   (SOC rings · service-line particles · roof lifts on Z4) │ SOC  63 % │
│      │                                                           │ PWR −2.5kW│
│      │        load bars: HVAC▇ EV▄ WH▂ POOL▁ OTH▃               │ PV  +4.1kW│
│      │                                                           │ GRID+1.2kW│
│      │                                                           │ rev $41.07│
│      │                                                           │ [enter Z5]│
├──────┴───────────────────────────────────────────────────────────┴───────────┤
│ KPI STRIP (unchanged — fleet context persists at all strata)                 │
├──────────────────────────────────────────────────────────────────────────────┤
│ BOTTOM TIMELINE (same component; per-home revenue/price overlay when 1 sel.) │
└──────────────────────────────────────────────────────────────────────────────┘
6.3 Component specs
Table
Component   Contents (bindings)   Behavior
Top bar   scenario name (sim.scenario_name); dual clock: SIM sim.time_s formatted CT + WALL clock; speed controls 1/4/8/32/128× (sim.speed_mult); lens segmented control; mode indicator   always visible; SIM clock mono tabular; at >32× speed the clock seconds blur is avoided by updating at 4 Hz
Left rail   mode icons: build / inspect / dispatch / scenario (Part A); tool sub-menus fly out right   48 px rail, icon+tooltip; active mode chip --energy-discharge hairline underline
Right inspector   context: zone (Z1) → cluster (Z2) → home (Z3/Z4) → device (Z5); always a mono telemetry table + one primary action   280 px, collapsible; stale fields dim per §0.1
KPI strip   fleet MW = Σ|home.*.battery.power_kw|/1000; MWh = Σ usable capacity×soc_pct; aggregate SOC %; LMP exposure $/h (Σ power×lmp); realized revenue (settlement.interval.cum_revenue_usd); 4CP gauge (four_cp.*)   32 px strip, 1 s recompute, tick animations per §4.5; exposure negative = earning, shown sage; positive = paying, amber
Bottom timeline   scrubber over sim window; price area chart (ercot.system_lambda_usd_mwh, --hairline grid, amber area); event markers: ◆ scarcity, ▼ dispatch, ▓ outage, ◇ settlement; zoom wheel   96 px; markers clickable → replay ticker entry
Ticker   single-line mono, 20 px, newest left; entries per §4   max 1 line, queue with 8 s dwell
6.4 Typography & spacing scale
Table
Style   Family   Size/line   Weight   Use
Display   Plex Sans   20/24   600   panel titles, scenario name
HUD label   Plex Sans   12/16   500   field labels, chips
Micro label   Plex Sans   10/12   500   uppercase +8% tracking, map annotations
Number L   Plex Mono (tnum)   20/24   500   SOC dial, KPI values
Number M   Plex Mono (tnum)   13/16   400   inspector values, prices
Number S   Plex Mono (tnum)   11/14   400   ticker, timeline axis
7. Audio (Optional, P2 — OFF BY DEFAULT)
Table
Layer   Source   Binding   Mix
Grid hum   filtered brown noise + 60/120 Hz partials   volume ∝ ercot.system load normalized (or Σ zone load_mw), lowpass cutoff ∝ operating_reserves_mw (scarce = brighter, tenser hum)   −30 dB base, capped −20 dB
Event chimes   per §4 table (scarcity two-tone, outage thump, transfer click, ack tick, settlement tink)   event triggers only   −18 to −24 dB, rate-limited
UI foley   panel open/close soft thud   user interaction   −28 dB
Implementation: single WebAudio GainNode master with compressor; hum = looped buffer + biquad automation at 1 Hz (no per-frame work); settings toggle persisted, default off; reduced-motion setting also forces audio off until explicitly enabled. Audio must never be the sole carrier of an alert (pairs with ticker text).
Appendix D-1 — Implementation Checklist (agent-facing)
Import tokens §0.3 as CSS custom properties + a JS TOKENS const feeding three.js material LUT; CI greps for stray hexes.
Build FlowSpline once (§3.2); Z1–Z5 scenes may only instantiate it with §3.1 params and §3.3 caps.
Scene registry: each stratum declares its 3D content table rows as data-bound components with explicit §0.2 field keys; unbound decoration limited to §1.1 pillar 4 allowance.
Event bus: §4 triggers subscribe to Part C streams; dedup keys enforced centrally.
Assets per §5.2 budgets; pipeline §5.4 CI gates green before any scene wiring.
HUD per §6 wireframes; tnum on every number; reduced-motion path §3.4 verified.
CVD simulation screenshot diff (deuteranopia) on Z1 price lens, Z3 SOC rings, Z4 wiring: state distinctions must survive (§0.3 gate).
End of Part D.
