/**
 * Fleet scenarios: named snapshots of the current world (homes, device
 * models, positions, zones) persisted to local storage. The server's
 * scenario API binds time, prices, and seed to a run - it has no fleet
 * composition to store - so snapshots live in this browser, and the
 * panel says so. Loading replaces the world through the controller:
 * against the live API (delete + recreate) or against the demo tape
 * (local swap, ids preserved so the recording still lines up).
 */

import { getRuntime } from "./runtime";
import { useAppStore } from "./store";

const STORAGE_KEY = "batsim.fleet-scenarios.v1";

/** One home in a saved fleet snapshot. */
export interface SavedScenarioHome {
  id: string;
  modelId: string;
  count: number;
  zone: string;
  archetype: string;
  lng: number;
  lat: number;
  soc: number;
}

export interface SavedScenario {
  version: 1;
  name: string;
  savedAt: string;
  /** World the snapshot was taken in; loading crosses worlds freely. */
  connection: "live" | "demo";
  homes: SavedScenarioHome[];
}

export function listScenarios(): SavedScenario[] {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (raw === null) return [];
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter(
      (s): s is SavedScenario =>
        typeof s === "object" && s !== null &&
        typeof (s as SavedScenario).name === "string" &&
        Array.isArray((s as SavedScenario).homes),
    );
  } catch {
    return [];
  }
}

function writeAll(scenarios: SavedScenario[]): void {
  window.localStorage.setItem(STORAGE_KEY, JSON.stringify(scenarios));
}

/** Snapshot the current world under a name; same name overwrites. */
export function saveScenario(name: string): SavedScenario {
  const { live } = getRuntime();
  const state = useAppStore.getState();
  const homes: SavedScenarioHome[] = [];
  for (const id of state.homeOrder) {
    const meta = state.homesMeta[id];
    const slot = live.slotOf.get(id);
    if (!meta || slot === undefined) continue;
    homes.push({
      id,
      modelId: meta.batteryModelId,
      count: meta.batteryCount,
      zone: meta.zone,
      archetype: meta.archetype,
      lng: live.lng[slot] ?? 0,
      lat: live.lat[slot] ?? 0,
      soc: live.soc[slot] ?? 0.5,
    });
  }
  const scenario: SavedScenario = {
    version: 1,
    name,
    savedAt: new Date().toISOString(),
    connection: state.connection === "live" ? "live" : "demo",
    homes,
  };
  writeAll([...listScenarios().filter((s) => s.name !== name), scenario]);
  return scenario;
}

export function deleteScenario(name: string): void {
  writeAll(listScenarios().filter((s) => s.name !== name));
}
