/**
 * HUD-facing application store. Entity metadata, connection status, lens
 * and selection state, the events feed, transport state, and the
 * throttled rollup of the latest tick. The per-slot per-tick numbers live
 * in the live buffers; this store only sees what the HUD renders.
 */

import { create } from "zustand";
import type { GridEvent } from "./events";

export type Lens = "price" | "soc";
export type ConnectionState = "connecting" | "live" | "demo" | "unreachable";
export type Stratum = "map" | "neighborhood";

/** Feed ring depth; older entries drop off the end. */
const EVENT_FEED_CAP = 60;

export interface HomeMeta {
  id: string;
  fleetId: string | null;
  batteryModelId: string;
  batteryDisplayName: string;
  vendor: string;
  chemistry: string;
  coupling: string;
  batteryCount: number;
  usableEnergyKwh: number;
  /** Lowest SOC the hardware allows before reserve protection, 0..1. */
  reserveFloorFrac: number;
  zone: string;
  archetype: string;
  pvPeakKw: number | null;
  mode: string;
}

export interface FleetRollup {
  homes: number;
  batteryKw: number;
  pvKw: number;
  loadKw: number;
  gridKw: number;
  socMean: number;
}

export interface AppState {
  connection: ConnectionState;
  scenarioName: string;
  stratum: Stratum;
  lens: Lens;
  selectedHomeId: string | null;
  homesMeta: Record<string, HomeMeta>;
  homeOrder: string[];
  /** Zone id whose homes form the street-level neighborhood. */
  neighborhoodZone: string | null;
  neighborhoodHomeIds: string[];
  neighborhoodAnchor: [number, number];
  /** Zone under the map crosshair; the dive gestures target it. */
  centerZone: string | null;
  /** Per-zone anchor points and display labels from the zones GeoJSON. */
  zoneAnchors: Record<string, [number, number]>;
  zoneLabels: Record<string, string>;
  priceRtm: number;
  simTimeMs: number;
  tick: number;
  fleet: FleetRollup;
  lastError: string | null;

  /** Transport state mirrored for the HUD. */
  paused: boolean;
  speedMult: number;
  /** Grid-events feed, newest first. */
  events: GridEvent[];
  /** Bumped when a dispatch acknowledgment arrives; drives the map ripple. */
  dispatchWave: number;
  /** One-line status for the dispatch panel (sending, acked, failed). */
  dispatchStatus: string | null;
  /** True when the active transport can seek to a recorded dispatch. */
  canReplayDispatch: boolean;
  /** Last known map zoom, updated on moveend; gates the dive chip. */
  mapZoom: number;

  setLens: (lens: Lens) => void;
  selectHome: (id: string | null) => void;
  setStratum: (stratum: Stratum) => void;
  /**
   * Dive into the street-level neighborhood for a zone. Null means the
   * zone currently under the map crosshair. No-op when the zone has no
   * homes - there is no street to show.
   */
  diveZone: (zone: string | null) => void;
}

export const useAppStore = create<AppState>((set) => ({
  connection: "connecting",
  scenarioName: "",
  stratum: "map",
  lens: "price",
  selectedHomeId: null,
  homesMeta: {},
  homeOrder: [],
  neighborhoodZone: null,
  neighborhoodHomeIds: [],
  neighborhoodAnchor: [-97.4, 32.9],
  centerZone: null,
  zoneAnchors: {},
  zoneLabels: {},
  priceRtm: 0,
  simTimeMs: 0,
  tick: 0,
  fleet: { homes: 0, batteryKw: 0, pvKw: 0, loadKw: 0, gridKw: 0, socMean: 0 },
  lastError: null,

  paused: false,
  speedMult: 60,
  events: [],
  dispatchWave: 0,
  dispatchStatus: null,
  canReplayDispatch: false,
  mapZoom: 5,

  setLens: (lens) => set({ lens }),
  selectHome: (id) => set({ selectedHomeId: id }),
  setStratum: (stratum) => set({ stratum }),
  diveZone: (zone) =>
    set((state) => {
      const target = zone ?? state.centerZone ?? state.neighborhoodZone;
      if (!target) return {};
      const homeIds = state.homeOrder.filter((id) => state.homesMeta[id]?.zone === target);
      if (homeIds.length === 0) return {};
      return {
        neighborhoodZone: target,
        neighborhoodHomeIds: homeIds,
        neighborhoodAnchor: state.zoneAnchors[target] ?? state.neighborhoodAnchor,
        stratum: "neighborhood" as const,
      };
    }),
}));

/** Prepend events to the feed, capped at the ring depth. */
export function pushEvents(incoming: GridEvent[]): void {
  if (incoming.length === 0) return;
  useAppStore.setState((state) => ({
    events: [...incoming.reverse(), ...state.events].slice(0, EVENT_FEED_CAP),
  }));
}
