/**
 * HUD-facing application store. Entity metadata, connection status, lens
 * and selection state, and the throttled rollup of the latest tick. The
 * per-slot per-tick numbers live in the live buffers; this store only
 * sees what the HUD renders.
 */

import { create } from "zustand";

export type Lens = "price" | "soc";
export type ConnectionState = "connecting" | "live" | "demo" | "unreachable";
export type Stratum = "map" | "neighborhood";

export interface HomeMeta {
  id: string;
  fleetId: string | null;
  batteryModelId: string;
  batteryDisplayName: string;
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
  priceRtm: number;
  simTimeMs: number;
  tick: number;
  fleet: FleetRollup;
  lastError: string | null;

  setLens: (lens: Lens) => void;
  selectHome: (id: string | null) => void;
  setStratum: (stratum: Stratum) => void;
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
  priceRtm: 0,
  simTimeMs: 0,
  tick: 0,
  fleet: { homes: 0, batteryKw: 0, pvKw: 0, loadKw: 0, gridKw: 0, socMean: 0 },
  lastError: null,

  setLens: (lens) => set({ lens }),
  selectHome: (id) => set({ selectedHomeId: id }),
  setStratum: (stratum) => set({ stratum }),
}));
