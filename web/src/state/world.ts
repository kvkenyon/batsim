/**
 * World mutation: add, remove, and replace homes in the live buffers and
 * the HUD store. The buffers are dense slot-indexed arrays, so removal
 * compacts by swapping the last slot into the gap and re-keying the id
 * map; the map and neighborhood views rebuild from count + ids, so one
 * version bump repaints every stratum. Every mutation goes through these
 * functions - nothing else writes home membership.
 */

import type { BatterySummary, HomeDoc } from "../api/client";
import { createLiveBuffers, type LiveBuffers } from "./live";
import { useAppStore, type AppState, type HomeMeta } from "./store";

/** Read a number at a nested path, null on any miss. */
export function numberAt(v: unknown, path: string[]): number | null {
  let cur = v;
  for (const key of path) {
    if (typeof cur !== "object" || cur === null) return null;
    cur = (cur as Record<string, unknown>)[key];
  }
  return typeof cur === "number" && Number.isFinite(cur) ? cur : null;
}

/**
 * HUD metadata for one home document: identity from the config, hardware
 * detail from the catalog summary and raw catalog entry. Shared by the
 * boot hydration and by homes created through build mode mid-session.
 */
export function homeMetaFromDoc(
  doc: HomeDoc,
  summaries: Map<string, BatterySummary>,
  details: Record<string, Record<string, unknown>>,
): HomeMeta {
  const modelId = doc.config.battery.model_id;
  const summary = summaries.get(modelId);
  const detail = details[modelId];
  return {
    id: doc.id,
    fleetId: doc.config.fleet_id ?? null,
    batteryModelId: modelId,
    batteryDisplayName: summary?.display_name ?? modelId,
    vendor: summary?.vendor ?? "",
    chemistry: summary?.chemistry ?? "",
    coupling: summary?.coupling ?? "",
    batteryCount: doc.config.battery.count,
    usableEnergyKwh:
      summary?.usable_energy_kwh ?? numberAt(detail, ["usable_energy_kwh", "value"]) ?? numberAt(detail, ["usable_energy_kwh"]) ?? 0,
    reserveFloorFrac: numberAt(detail ?? {}, ["soc_window", "reserve_floor_frac"]) ?? 0,
    zone: doc.config.ercot_load_zone,
    archetype: doc.config.load_archetype,
    pvPeakKw: doc.config.pv_peak_kw ?? null,
    mode: doc.state.mode,
  };
}

/** Initial telemetry state for a freshly placed home. */
export interface PlacedState {
  soc: number;
}

function syncStore(live: LiveBuffers): void {
  const state = useAppStore.getState();
  const patch: Partial<AppState> = {};
  // A home vanishing under an open inspector closes the panel.
  if (state.selectedHomeId !== null && !live.slotOf.has(state.selectedHomeId)) {
    patch.selectedHomeId = null;
  }
  // Keep the walked neighborhood in step with the world while street
  // level is showing; the next dive recomputes from scratch regardless.
  if (state.neighborhoodZone !== null) {
    patch.neighborhoodHomeIds = state.neighborhoodZone
      ? live.homeIds
          .slice(0, live.count)
          .filter((id) => state.homesMeta[id]?.zone === state.neighborhoodZone)
      : state.neighborhoodHomeIds;
  }
  useAppStore.setState(patch);
}

/**
 * Append a home at an explicit position. Returns false when the slot
 * arrays are full; the caller reports the rejection and leaves the API
 * home to appear on the next boot.
 */
export function addHomeToWorld(
  live: LiveBuffers,
  meta: HomeMeta,
  lng: number,
  lat: number,
  initial: PlacedState,
): boolean {
  if (live.count >= live.soc.length) return false;
  const slot = live.count;
  live.count += 1;
  live.homeIds[slot] = meta.id;
  live.slotOf.set(meta.id, slot);
  live.lng[slot] = lng;
  live.lat[slot] = lat;
  const anchor = useAppStore.getState().zoneAnchors[meta.zone] ?? [lng, lat];
  live.anchorLng[slot] = anchor[0];
  live.anchorLat[slot] = anchor[1];
  live.soc[slot] = initial.soc;
  live.batteryKw[slot] = 0;
  live.pvKw[slot] = 0;
  live.loadKw[slot] = 0;
  live.gridKw[slot] = 0;
  live.version += 1;
  useAppStore.setState((state) => ({
    homesMeta: { ...state.homesMeta, [meta.id]: meta },
    homeOrder: [...state.homeOrder, meta.id],
  }));
  syncStore(live);
  return true;
}

/** Remove a home, compacting the slot arrays. No-op for unknown ids. */
export function removeHomeFromWorld(live: LiveBuffers, homeId: string): void {
  const slot = live.slotOf.get(homeId);
  if (slot === undefined) return;
  const last = live.count - 1;
  if (slot !== last) {
    const movedId = live.homeIds[last];
    if (movedId !== undefined) {
      live.homeIds[slot] = movedId;
      live.slotOf.set(movedId, slot);
    }
    live.soc[slot] = live.soc[last] ?? 0;
    live.batteryKw[slot] = live.batteryKw[last] ?? 0;
    live.pvKw[slot] = live.pvKw[last] ?? 0;
    live.loadKw[slot] = live.loadKw[last] ?? 0;
    live.gridKw[slot] = live.gridKw[last] ?? 0;
    live.lng[slot] = live.lng[last] ?? 0;
    live.lat[slot] = live.lat[last] ?? 0;
    live.anchorLng[slot] = live.anchorLng[last] ?? 0;
    live.anchorLat[slot] = live.anchorLat[last] ?? 0;
    live.chargedKwh[slot] = live.chargedKwh[last] ?? 0;
    live.dischargedKwh[slot] = live.dischargedKwh[last] ?? 0;
    live.pvKwh[slot] = live.pvKwh[last] ?? 0;
    live.gridImportKwh[slot] = live.gridImportKwh[last] ?? 0;
    if (live.socHist.length > 0) {
      const rowBytes = live.socHist.length / live.soc.length;
      live.socHist.copyWithin(slot * rowBytes, last * rowBytes, (last + 1) * rowBytes);
    }
  }
  live.homeIds.length = last;
  live.slotOf.delete(homeId);
  live.count = last;
  live.version += 1;
  useAppStore.setState((state) => {
    const homesMeta = { ...state.homesMeta };
    delete homesMeta[homeId];
    return {
      homesMeta,
      homeOrder: state.homeOrder.filter((id) => id !== homeId),
    };
  });
  syncStore(live);
}

/** One home in a world replacement (scenario load). */
export interface WorldSeed {
  meta: HomeMeta;
  lng: number;
  lat: number;
  soc: number;
}

/**
 * Swap the whole fleet. Slots are rebuilt from scratch; telemetry frames
 * for ids no longer present are dropped by the ingest guard until the
 * stream catches up (live) or the next replay loop (demo).
 */
export function replaceWorld(live: LiveBuffers, seeds: WorldSeed[]): number {
  const fresh = createLiveBuffers(live.soc.length);
  Object.assign(live, fresh);
  let placed = 0;
  const metas: Record<string, HomeMeta> = {};
  const order: string[] = [];
  for (const seed of seeds) {
    if (live.count >= live.soc.length) break;
    const slot = live.count;
    live.count += 1;
    live.homeIds[slot] = seed.meta.id;
    live.slotOf.set(seed.meta.id, slot);
    live.lng[slot] = seed.lng;
    live.lat[slot] = seed.lat;
    live.soc[slot] = seed.soc;
    metas[seed.meta.id] = seed.meta;
    order.push(seed.meta.id);
    placed += 1;
  }
  // Anchors come from the store (zone geojson), not the seed list.
  const anchors = useAppStore.getState().zoneAnchors;
  for (let slot = 0; slot < live.count; slot++) {
    const meta = metas[live.homeIds[slot] ?? ""];
    const anchor = (meta && anchors[meta.zone]) ?? [live.lng[slot] ?? 0, live.lat[slot] ?? 0];
    live.anchorLng[slot] = anchor[0];
    live.anchorLat[slot] = anchor[1];
  }
  live.version += 1;
  useAppStore.setState({
    homesMeta: metas,
    homeOrder: order,
    selectedHomeId: null,
  });
  syncStore(live);
  return placed;
}
