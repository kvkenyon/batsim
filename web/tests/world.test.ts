import { beforeEach, describe, expect, it } from "vitest";
import { createLiveBuffers } from "../src/state/live";
import { setRuntime } from "../src/state/runtime";
import { deleteScenario, listScenarios, saveScenario } from "../src/state/scenarios";
import { useAppStore, type HomeMeta } from "../src/state/store";
import type { TelemetryTransport } from "../src/state/transport";
import { addHomeToWorld, removeHomeFromWorld, replaceWorld, type WorldSeed } from "../src/state/world";

/** Scenario persistence only needs the buffers, never a live transport. */
const transportStub: TelemetryTransport = {
  kind: "replay",
  start() {},
  stop() {},
};

interface WindowShim {
  window: { localStorage: unknown };
}

function meta(id: string, zone = "LZ_NORTH"): HomeMeta {
  return {
    id,
    fleetId: null,
    batteryModelId: "tesla.powerwall_3",
    batteryDisplayName: "Tesla Powerwall 3",
    vendor: "Tesla",
    chemistry: "LFP",
    coupling: "DCCoupledHybrid",
    batteryCount: 1,
    usableEnergyKwh: 13.5,
    reserveFloorFrac: 0.2,
    zone,
    archetype: "sfh_family",
    pvPeakKw: null,
    mode: "self-consumption",
  };
}

beforeEach(() => {
  useAppStore.setState({
    homesMeta: {},
    homeOrder: [],
    selectedHomeId: null,
    neighborhoodZone: null,
    neighborhoodHomeIds: [],
    zoneAnchors: { LZ_NORTH: [-97.4, 32.9], LZ_WEST: [-102.1, 31.9] },
  });
});

describe("addHomeToWorld", () => {
  it("appends a slot, positions it, and syncs the store", () => {
    const live = createLiveBuffers(4);
    expect(addHomeToWorld(live, meta("h1"), -97.1, 32.5, { soc: 0.5 })).toBe(true);
    expect(live.count).toBe(1);
    expect(live.slotOf.get("h1")).toBe(0);
    expect(live.lng[0]).toBeCloseTo(-97.1);
    expect(live.soc[0]).toBeCloseTo(0.5);
    // The zone anchor backs the flow overlay's service lines.
    expect(live.anchorLng[0]).toBeCloseTo(-97.4);
    const state = useAppStore.getState();
    expect(state.homeOrder).toEqual(["h1"]);
    expect(state.homesMeta.h1?.zone).toBe("LZ_NORTH");
  });

  it("refuses to overflow the slot arrays", () => {
    const live = createLiveBuffers(1);
    expect(addHomeToWorld(live, meta("h1"), 0, 0, { soc: 0.5 })).toBe(true);
    expect(addHomeToWorld(live, meta("h2"), 0, 0, { soc: 0.5 })).toBe(false);
    expect(live.count).toBe(1);
    expect(useAppStore.getState().homeOrder).toEqual(["h1"]);
  });
});

describe("removeHomeFromWorld", () => {
  it("compacts by swapping the last slot into the gap", () => {
    const live = createLiveBuffers(8);
    for (const id of ["h1", "h2", "h3"]) addHomeToWorld(live, meta(id), 0, 0, { soc: 0.5 });
    live.batteryKw[2] = 3;
    removeHomeFromWorld(live, "h2");
    expect(live.count).toBe(2);
    // h3 moved into h2's slot with its telemetry and position intact.
    expect(live.slotOf.get("h3")).toBe(1);
    expect(live.homeIds[1]).toBe("h3");
    expect(live.batteryKw[1]).toBe(3);
    expect(live.slotOf.has("h2")).toBe(false);
    expect(useAppStore.getState().homeOrder).toEqual(["h1", "h3"]);
  });

  it("closes the inspector when the selected home vanishes", () => {
    const live = createLiveBuffers(4);
    addHomeToWorld(live, meta("h1"), 0, 0, { soc: 0.5 });
    useAppStore.setState({ selectedHomeId: "h1" });
    removeHomeFromWorld(live, "h1");
    expect(useAppStore.getState().selectedHomeId).toBeNull();
  });

  it("ignores unknown ids", () => {
    const live = createLiveBuffers(4);
    addHomeToWorld(live, meta("h1"), 0, 0, { soc: 0.5 });
    removeHomeFromWorld(live, "ghost");
    expect(live.count).toBe(1);
  });
});

describe("replaceWorld", () => {
  it("swaps the fleet and drops stale selection", () => {
    const live = createLiveBuffers(8);
    for (const id of ["a", "b", "c"]) addHomeToWorld(live, meta(id), 1, 1, { soc: 0.5 });
    useAppStore.setState({ selectedHomeId: "a" });
    const seeds: WorldSeed[] = [
      { meta: meta("x", "LZ_WEST"), lng: -102, lat: 31, soc: 0.9 },
      { meta: meta("y", "LZ_NORTH"), lng: -97, lat: 33, soc: 0.1 },
    ];
    expect(replaceWorld(live, seeds)).toBe(2);
    expect(live.count).toBe(2);
    expect(live.slotOf.get("x")).toBe(0);
    expect(live.soc[0]).toBeCloseTo(0.9);
    expect(live.anchorLng[0]).toBeCloseTo(-102.1);
    const state = useAppStore.getState();
    expect(state.homeOrder).toEqual(["x", "y"]);
    expect(state.selectedHomeId).toBeNull();
    expect(state.homesMeta.a).toBeUndefined();
  });
});

describe("fleet scenarios", () => {
  const backing = new Map<string, string>();
  const localStorageShim = {
    getItem: (k: string) => backing.get(k) ?? null,
    setItem: (k: string, v: string) => void backing.set(k, v),
    removeItem: (k: string) => void backing.delete(k),
  };

  beforeEach(() => {
    backing.clear();
    // node has no DOM; scenarios.ts touches only localStorage on window.
    const shim: WindowShim = { window: { localStorage: localStorageShim } };
    Object.assign(globalThis, shim);
  });

  it("round-trips a snapshot of the current world", () => {
    const live = createLiveBuffers(8);
    addHomeToWorld(live, meta("h1"), -97.5, 32.8, { soc: 0.7 });
    setRuntime({ live, transport: transportStub });
    const saved = saveScenario("evening fleet");
    expect(saved.homes).toHaveLength(1);
    expect(saved.homes[0]).toMatchObject({ id: "h1", zone: "LZ_NORTH" });
    expect(saved.homes[0]?.lng).toBeCloseTo(-97.5);
    expect(saved.homes[0]?.soc).toBeCloseTo(0.7);
    expect(listScenarios().map((s) => s.name)).toEqual(["evening fleet"]);
    deleteScenario("evening fleet");
    expect(listScenarios()).toEqual([]);
  });

  it("saving the same name overwrites instead of duplicating", () => {
    const live = createLiveBuffers(8);
    addHomeToWorld(live, meta("h1"), 0, 0, { soc: 0.5 });
    setRuntime({ live, transport: transportStub });
    saveScenario("fleet");
    saveScenario("fleet");
    expect(listScenarios()).toHaveLength(1);
  });

  it("survives corrupt storage", () => {
    backing.set("batsim.fleet-scenarios.v1", "{not json");
    expect(listScenarios()).toEqual([]);
    backing.set("batsim.fleet-scenarios.v1", '{"a":1}');
    expect(listScenarios()).toEqual([]);
  });
});
