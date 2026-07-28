/**
 * Console transport controls. One facade over the two transports: the
 * live API (pause / resume / speed / run-until / dispatch over HTTP) and
 * the recorded replay (the same gestures mapped onto the tape deck,
 * placements and fleet swaps applied to the local demo world). The HUD
 * talks only to this module; it never branches on connection kind
 * itself.
 */

import {
  type BatsimApi,
  type BatteryModelId,
  type BatterySummary,
  type CreateHomeRequest,
  type DispatchRequest,
  type LoadArchetype,
  type LoadZone,
} from "../api/client";
import { getRuntime } from "./runtime";
import type { SavedScenarioHome } from "./scenarios";
import { useAppStore, type HomeMeta, type ZoneAck } from "./store";
import { ReplayTransport } from "./transport";
import { addHomeToWorld, homeMetaFromDoc, numberAt, removeHomeFromWorld, replaceWorld, type WorldSeed } from "./world";

export type DispatchDirection = "discharge" | "charge" | "idle";

export interface SimController {
  readonly kind: "live" | "replay";
  setPaused(paused: boolean): void;
  setSpeed(multiplier: number): void;
  /** Jump forward to the next noteworthy price moment. */
  jumpToNextPriceEvent(): void;
  /** Fire a fleet-wide charge or discharge command. */
  dispatchFleet(direction: "discharge" | "charge"): void;
  /** Fire a command at one zone's homes; acks land in `store.zoneAck`. */
  dispatchZone(zone: string, direction: DispatchDirection): void;
  /**
   * Place a home of a catalog model at a map position. The caller has
   * already resolved the zone under the point; placement outside every
   * zone is rejected before this is called.
   */
  placeHome(modelId: string, zone: string, lng: number, lat: number): void;
  /** Remove a home from the world (API-side in live mode). */
  removeHome(homeId: string): void;
  /** Replace the fleet with a saved scenario's homes. */
  loadScenario(homes: SavedScenarioHome[]): void;
  /** Seek the replay tape to a sim time; false when not scrubbable. */
  seekTo(simTimeMs: number): boolean;
}

/** Per-home setpoint for dispatch moments, kW. */
const FLEET_DISPATCH_KW = 5;
/** Hold duration for dispatch moments, seconds. */
const FLEET_DISPATCH_DURATION_S = 1800;
/** Poll cadence for live per-target acknowledgement rollups. */
const ACK_POLL_MS = 400;
/** Give up waiting for straggler acknowledgements after this long. */
const ACK_TIMEOUT_MS = 45_000;
/** Battery power (kW) past which a replayed home counts as responding. */
const REPLAY_ACK_KW = 0.5;
/** Default load archetype for homes placed from build mode. */
const PLACED_ARCHETYPE = "sfh_family";
/** Initial state of charge for homes placed from build mode. */
const PLACED_SOC = 0.5;

function commandId(): string {
  return `ui-${Date.now().toString(36)}-${Math.floor(Math.random() * 1e9).toString(36)}`;
}

function setError(err: unknown): void {
  useAppStore.setState({ lastError: err instanceof Error ? err.message : String(err) });
}

/** Zone roster for a dispatch: home ids currently sitting in the zone. */
function zoneHomeIds(zone: string): string[] {
  const state = useAppStore.getState();
  return state.homeOrder.filter((id) => state.homesMeta[id]?.zone === zone);
}

function dispatchAction(direction: DispatchDirection): DispatchRequest["action"] {
  if (direction === "idle") return { type: "set_mode", mode: "self-consumption" };
  return {
    type: direction === "discharge" ? "discharge_to" : "charge_to",
    kw: FLEET_DISPATCH_KW,
    duration_s: FLEET_DISPATCH_DURATION_S,
  };
}

/** Poll a live command until every target reports, updating the ack rollup. */
function watchLiveCommand(api: BatsimApi, command: string, zone: string, direction: DispatchDirection): void {
  const started = Date.now();
  const poll = async (): Promise<void> => {
    const ack = useAppStore.getState().zoneAck;
    if (ack === null || ack.zone !== zone || ack.done) return;
    try {
      const doc = await api.getCommand(command);
      const acked = doc.targets.filter(
        (t) => t.status === "applied" || t.status === "partial",
      ).length;
      const settled = doc.targets.filter((t) => t.status !== null && t.status !== undefined).length;
      const done =
        settled >= doc.targets.length ||
        doc.status === "completed" ||
        doc.status === "completed_with_errors" ||
        doc.status === "cancelled";
      useAppStore.setState({ zoneAck: { zone, direction, acked, expected: doc.targets.length, done } });
      if (!done && Date.now() - started < ACK_TIMEOUT_MS) {
        window.setTimeout(() => void poll(), ACK_POLL_MS);
      } else if (!done) {
        useAppStore.setState((state) => ({
          zoneAck: state.zoneAck ? { ...state.zoneAck, done: true } : null,
        }));
      }
    } catch (err) {
      useAppStore.setState((state) => ({
        zoneAck: state.zoneAck ? { ...state.zoneAck, done: true } : null,
      }));
      setError(err);
    }
  };
  window.setTimeout(() => void poll(), ACK_POLL_MS);
}

/**
 * Count a zone's homes as they respond in the recorded fleet dispatch:
 * each home crosses the commanded direction's power threshold with the
 * per-device latency the recorder captured, so the rollup climbs home by
 * home exactly as it would against a live fleet.
 */
function watchReplayAcks(zone: string, direction: DispatchDirection): void {
  const started = Date.now();
  const poll = (): void => {
    const ack = useAppStore.getState().zoneAck;
    if (ack === null || ack.zone !== zone || ack.done) return;
    const { live } = getRuntime();
    let acked = 0;
    for (const id of zoneHomeIds(zone)) {
      const slot = live.slotOf.get(id);
      if (slot === undefined) continue;
      const kw = live.batteryKw[slot] ?? 0;
      const responding =
        direction === "discharge" ? kw > REPLAY_ACK_KW : direction === "charge" ? kw < -REPLAY_ACK_KW : Math.abs(kw) <= REPLAY_ACK_KW;
      if (responding) acked += 1;
    }
    const timedOut = Date.now() - started > ACK_TIMEOUT_MS;
    const done = acked >= ack.expected || timedOut;
    useAppStore.setState({ zoneAck: { ...ack, acked, done } });
    if (!done) window.setTimeout(poll, ACK_POLL_MS);
  };
  window.setTimeout(poll, ACK_POLL_MS);
}

/** Catalog summaries keyed by model, in the shape meta construction expects. */
function catalogSummaries(): Map<string, BatterySummary> {
  return new Map(
    useAppStore.getState().catalog.map((b) => [
      b.modelId,
      {
        model_id: b.modelId,
        vendor: b.vendor,
        display_name: b.displayName,
        chemistry: b.chemistry,
        coupling: b.coupling,
        usable_energy_kwh: b.usableEnergyKwh,
        continuous_charge_power_kw: 0,
        continuous_discharge_power_kw: 0,
      },
    ]),
  );
}

/** Metadata for a locally placed demo-world home (no API behind it). */
function localHomeMeta(
  fields: { id: string; modelId: string; zone: string; archetype: string; count: number },
): HomeMeta | null {
  const state = useAppStore.getState();
  const entry = state.catalog.find((b) => b.modelId === fields.modelId);
  if (!entry) return null;
  return {
    id: fields.id,
    fleetId: null,
    batteryModelId: entry.modelId,
    batteryDisplayName: entry.displayName,
    vendor: entry.vendor,
    chemistry: entry.chemistry,
    coupling: entry.coupling,
    batteryCount: fields.count,
    usableEnergyKwh: entry.usableEnergyKwh,
    reserveFloorFrac:
      numberAt(state.batteryDetails[entry.modelId] ?? {}, ["soc_window", "reserve_floor_frac"]) ?? 0,
    zone: fields.zone,
    archetype: fields.archetype,
    pvPeakKw: null,
    mode: "self-consumption",
  };
}

let localHomeSequence = 0;

export function createLiveController(api: BatsimApi, fleetId: string | null): SimController {
  const set = useAppStore.setState;
  return {
    kind: "live",
    setPaused(paused) {
      set({ paused });
      void (paused ? api.simPause() : api.simResume()).catch(setError);
    },
    setSpeed(multiplier) {
      set({ speedMult: multiplier });
      void api.setSpeed(multiplier).catch(setError);
    },
    jumpToNextPriceEvent() {
      // The API advances a paused sim to a wall time; the next 5-minute
      // settlement boundary is the next moment a price can change.
      void (async () => {
        try {
          const { sim_time } = await api.simStatus();
          const now = Date.parse(sim_time);
          const next = Math.ceil((now + 1) / 300_000) * 300_000;
          await api.simPause();
          await api.runUntil(new Date(next).toISOString());
          if (!useAppStore.getState().paused) await api.simResume();
        } catch (err) {
          setError(err);
        }
      })();
    },
    dispatchFleet(direction) {
      if (!fleetId) {
        set({ dispatchStatus: "no fleet to dispatch" });
        return;
      }
      set({ dispatchStatus: `${direction} ${FLEET_DISPATCH_KW} kW · sending…` });
      void api
        .dispatchFleet(fleetId, {
          command_id: commandId(),
          action: {
            type: direction === "discharge" ? "discharge_to" : "charge_to",
            kw: FLEET_DISPATCH_KW,
            duration_s: FLEET_DISPATCH_DURATION_S,
          },
        })
        .then((res) => {
          set({ dispatchStatus: `${direction} · ${res.targets} homes · command accepted` });
        })
        .catch((err: unknown) => {
          set({ dispatchStatus: err instanceof Error ? err.message : String(err) });
        });
    },
    dispatchZone(zone, direction) {
      const homeIds = zoneHomeIds(zone);
      if (homeIds.length === 0) {
        set({ zoneAck: null, dispatchStatus: `no homes in ${zone}` });
        return;
      }
      const ack: ZoneAck = { zone, direction, acked: 0, expected: homeIds.length, done: false };
      set({ zoneAck: ack, dispatchStatus: `${direction} ${zone} · sending…` });
      void api
        .dispatch({
          command_id: commandId(),
          target: { home_ids: homeIds },
          action: dispatchAction(direction),
        })
        .then((res) => {
          set({ dispatchStatus: `${direction} ${zone} · ${res.targets} homes targeted` });
          watchLiveCommand(api, res.command_id, zone, direction);
        })
        .catch((err: unknown) => {
          set({ zoneAck: null, dispatchStatus: err instanceof Error ? err.message : String(err) });
        });
    },
    placeHome(modelId, zone, lng, lat) {
      const state = useAppStore.getState();
      if (!state.catalog.some((b) => b.modelId === modelId)) {
        set({ buildStatus: `unknown model ${modelId}` });
        return;
      }
      set({ buildStatus: "creating home…" });
      const request: CreateHomeRequest = {
        // Model and zone strings come from the registry catalog and the
        // zone geojson, so they always satisfy the narrowed enums.
        battery: { model_id: modelId as BatteryModelId, count: 1 },
        load: { archetype: PLACED_ARCHETYPE },
        location: { ercot_load_zone: zone as LoadZone },
        fleet_id: fleetId,
        initial_soc: PLACED_SOC,
      };
      void api
        .createHome(request)
        .then((doc) => {
          const meta = homeMetaFromDoc(doc, catalogSummaries(), useAppStore.getState().batteryDetails);
          const placed = addHomeToWorld(getRuntime().live, meta, lng, lat, {
            soc: doc.state.soc,
          });
          set({
            buildStatus: placed
              ? `${meta.batteryDisplayName} placed in ${zone}`
              : "world is full; home exists via API but is not shown",
          });
        })
        .catch((err: unknown) => {
          set({ buildStatus: err instanceof Error ? err.message : String(err) });
        });
    },
    removeHome(homeId) {
      // Home deletion requires a paused sim; pause, delete, and restore.
      set({ buildStatus: "removing home…" });
      void (async () => {
        try {
          const wasRunning = (await api.simStatus()).state === "running";
          if (wasRunning) await api.simPause();
          try {
            await api.deleteHome(homeId);
          } finally {
            if (wasRunning) await api.simResume();
          }
          removeHomeFromWorld(getRuntime().live, homeId);
          set({ buildStatus: "home removed" });
        } catch (err) {
          set({ buildStatus: err instanceof Error ? err.message : String(err) });
        }
      })();
    },
    loadScenario(homes) {
      set({ buildStatus: `loading scenario · replacing ${useAppStore.getState().homeOrder.length} homes…` });
      void (async () => {
        // The whole swap is structural: pause the sim for the duration.
        const wasRunning = (await api.simStatus()).state === "running";
        if (wasRunning) await api.simPause();
        try {
          const current = [...useAppStore.getState().homeOrder];
          for (const id of current) {
            await api.deleteHome(id).catch(() => undefined);
          }
          const seeds: WorldSeed[] = [];
          let failed = 0;
          for (const home of homes) {
            try {
              const doc = await api.createHome({
                battery: { model_id: home.modelId as BatteryModelId, count: home.count },
                load: { archetype: home.archetype as LoadArchetype },
                location: { ercot_load_zone: home.zone as LoadZone },
                fleet_id: fleetId,
                initial_soc: home.soc,
              });
              seeds.push({
                meta: homeMetaFromDoc(doc, catalogSummaries(), useAppStore.getState().batteryDetails),
                lng: home.lng,
                lat: home.lat,
                soc: doc.state.soc,
              });
            } catch {
              failed += 1;
            }
          }
          const placed = replaceWorld(getRuntime().live, seeds);
          set({
            buildStatus:
              failed > 0
                ? `scenario loaded · ${placed} homes · ${failed} rejected by the API`
                : `scenario loaded · ${placed} homes`,
          });
        } finally {
          if (wasRunning) await api.simResume();
        }
      })();
    },
    seekTo() {
      // A live simulation cannot rewind; the scrubber says so itself.
      return false;
    },
  };
}

export function createReplayController(transport: ReplayTransport): SimController {
  const set = useAppStore.setState;
  return {
    kind: "replay",
    setPaused(paused) {
      set({ paused });
      if (paused) transport.pause();
      else transport.resume();
    },
    setSpeed(multiplier) {
      set({ speedMult: multiplier });
      transport.setSpeed(multiplier);
    },
    jumpToNextPriceEvent() {
      if (!transport.jumpToNextPriceEvent()) {
        set({ dispatchStatus: "no price event ahead in the recording" });
      }
    },
    dispatchFleet(_direction) {
      // The recording already contains a real fleet dispatch with true
      // per-home execution latency; seek to it and watch the fleet move.
      if (transport.jumpToNextDispatch()) {
        set({
          paused: false,
          dispatchStatus: "replaying recorded fleet dispatch · watch the fleet respond",
        });
        transport.resume();
      } else {
        set({ dispatchStatus: "recording holds no fleet dispatch" });
      }
    },
    dispatchZone(zone, direction) {
      const expected = zoneHomeIds(zone).length;
      if (expected === 0) {
        set({ zoneAck: null, dispatchStatus: `no homes in ${zone}` });
        return;
      }
      // The tape holds one real fleet discharge; per-zone charge and idle
      // have nothing recorded to replay, and pretending otherwise would
      // show a discharge labeled as something it is not.
      if (direction !== "discharge") {
        set({ zoneAck: null, dispatchStatus: `recording holds no ${direction} command` });
        return;
      }
      if (!transport.jumpToNextDispatch()) {
        set({ zoneAck: null, dispatchStatus: "recording holds no fleet dispatch" });
        return;
      }
      set({
        paused: false,
        zoneAck: { zone, direction, acked: 0, expected, done: false },
        dispatchStatus: `replaying recorded dispatch · ${zone} responding`,
      });
      transport.resume();
      watchReplayAcks(zone, direction);
    },
    placeHome(modelId, zone, lng, lat) {
      localHomeSequence += 1;
      const meta = localHomeMeta({
        id: `local-${Date.now().toString(36)}-${localHomeSequence}`,
        modelId,
        zone,
        archetype: PLACED_ARCHETYPE,
        count: 1,
      });
      if (!meta) {
        set({ buildStatus: `unknown model ${modelId}` });
        return;
      }
      const placed = addHomeToWorld(getRuntime().live, meta, lng, lat, { soc: PLACED_SOC });
      set({
        buildStatus: placed
          ? `${meta.batteryDisplayName} placed in ${zone} · demo world, not persisted`
          : "demo world is full",
      });
    },
    removeHome(homeId) {
      removeHomeFromWorld(getRuntime().live, homeId);
      set({ buildStatus: "home removed from the demo world" });
    },
    loadScenario(homes) {
      // Local swap; saved ids are preserved so the recorded tape still
      // lines up with the homes it was captured from.
      const seeds: WorldSeed[] = [];
      let skipped = 0;
      for (const home of homes) {
        const meta = localHomeMeta({
          id: home.id,
          modelId: home.modelId,
          zone: home.zone,
          archetype: home.archetype,
          count: home.count,
        });
        if (!meta) {
          skipped += 1;
          continue;
        }
        seeds.push({ meta, lng: home.lng, lat: home.lat, soc: home.soc });
      }
      const placed = replaceWorld(getRuntime().live, seeds);
      set({
        buildStatus:
          skipped > 0
            ? `scenario loaded · ${placed} homes · ${skipped} skipped (unknown model)`
            : `scenario loaded · ${placed} homes · demo world`,
      });
    },
    seekTo(simTimeMs) {
      return transport.seekToSimTime(simTimeMs);
    },
  };
}

let controller: SimController | null = null;

export function setController(next: SimController): void {
  controller = next;
}

export function getController(): SimController {
  if (!controller) throw new Error("sim controller used before bootstrap completed");
  return controller;
}
