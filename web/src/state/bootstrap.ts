/**
 * Boot sequence: probe a live server, otherwise fall back to the bundled
 * recorded trace. Both paths hydrate identical entity metadata, assign
 * deterministic home positions, and start a telemetry transport over the
 * shared ingest pipeline.
 */

import { createBatsimApi, type BatsimApi, type BatterySummary, type HomeDoc } from "../api/client";
import { homeLngLat, parseZones, type ZoneFeature } from "../procgen/placement";
import { createLiveController, createReplayController, setController, type SimController } from "./controls";
import { createIngest } from "./ingest";
import { createLiveBuffers, type LiveBuffers } from "./live";
import { setRuntime } from "./runtime";
import { useAppStore, type CatalogBattery, type HomeMeta } from "./store";
import { homeMetaFromDoc } from "./world";
import { ReplayTransport, SseTransport, type TelemetryTransport, type TraceManifest } from "./transport";

export interface BootResult {
  live: LiveBuffers;
  transport: TelemetryTransport;
}

interface EntitiesBundle {
  homes: HomeDoc[];
  batteries: BatterySummary[];
  batteryDetails: Record<string, Record<string, unknown>>;
  scenarioName: string;
}

const DEMO_TRACE_URL = "traces/demo";

async function loadLiveEntities(api: BatsimApi): Promise<EntitiesBundle> {
  const [homes, batteries, status] = await Promise.all([
    api.listHomes(),
    api.listBatteries(),
    api.simStatus().catch(() => null),
  ]);
  const modelIds = [...new Set(homes.map((h) => h.config.battery.model_id))];
  const detailPairs = await Promise.all(
    modelIds.map(async (id) => [id, await api.batteryDetail(id)] as const),
  );
  const batteryDetails: Record<string, Record<string, unknown>> = {};
  for (const [id, detail] of detailPairs) {
    if (detail) batteryDetails[id] = detail;
  }
  return {
    homes,
    batteries,
    batteryDetails,
    scenarioName: status?.active_scenario ?? "live world",
  };
}

async function loadDemoEntities(traceUrl: string): Promise<EntitiesBundle> {
  const res = await fetch(`${traceUrl}/entities.json`);
  if (!res.ok) throw new Error(`demo trace entities missing: HTTP ${res.status}`);
  const bundle = (await res.json()) as EntitiesBundle & { scenarioName?: string };
  return {
    homes: bundle.homes ?? [],
    batteries: bundle.batteries ?? [],
    batteryDetails: bundle.batteryDetails ?? {},
    scenarioName: bundle.scenarioName ?? "recorded demo",
  };
}

function buildHomeMeta(bundle: EntitiesBundle): Record<string, HomeMeta> {
  const summaryByModel = new Map(bundle.batteries.map((b) => [b.model_id, b]));
  const metas: Record<string, HomeMeta> = {};
  for (const home of bundle.homes) {
    metas[home.id] = homeMetaFromDoc(home, summaryByModel, bundle.batteryDetails);
  }
  return metas;
}

function assignPositions(
  homes: HomeDoc[],
  zones: Map<string, ZoneFeature>,
  live: LiveBuffers,
): void {
  const fallback: [number, number] = [-97.4, 32.9];
  homes.forEach((home, slot) => {
    const zone = zones.get(home.config.ercot_load_zone);
    const [lng, lat] = zone ? homeLngLat(home.id, zone) : fallback;
    live.homeIds[slot] = home.id;
    live.slotOf.set(home.id, slot);
    live.lng[slot] = lng;
    live.lat[slot] = lat;
    live.anchorLng[slot] = zone?.anchor[0] ?? fallback[0];
    live.anchorLat[slot] = zone?.anchor[1] ?? fallback[1];
    live.soc[slot] = home.state.soc;
    live.batteryKw[slot] = home.state.battery_power_kw;
    live.pvKw[slot] = home.state.pv_power_kw;
    live.loadKw[slot] = home.state.load_power_kw;
    live.gridKw[slot] = home.state.grid_power_kw;
  });
  live.count = homes.length;
}

function pickNeighborhood(
  homes: HomeDoc[],
  zones: Map<string, ZoneFeature>,
): { zone: string; homeIds: string[]; anchor: [number, number] } {
  const byZone = new Map<string, string[]>();
  for (const home of homes) {
    const list = byZone.get(home.config.ercot_load_zone) ?? [];
    list.push(home.id);
    byZone.set(home.config.ercot_load_zone, list);
  }
  let best: [string, string[]] | null = null;
  for (const entry of byZone) {
    if (!best || entry[1].length > best[1].length) best = entry;
  }
  if (!best) return { zone: "", homeIds: [], anchor: [-97.4, 32.9] };
  const zone = zones.get(best[0]);
  return { zone: best[0], homeIds: best[1], anchor: zone?.anchor ?? [-97.4, 32.9] };
}

let bootPromise: Promise<BootResult> | null = null;

/**
 * Idempotent: React StrictMode mounts the app effect twice in dev, and a
 * second run would leak a transport and interleave two ingest pipelines
 * into the shared store. The first in-flight boot wins.
 */
export function bootstrap(options: { forceDemo?: boolean; apiBase?: string } = {}): Promise<BootResult> {
  bootPromise ??= runBootstrap(options);
  return bootPromise;
}

async function runBootstrap(options: { forceDemo?: boolean; apiBase?: string }): Promise<BootResult> {
  const store = useAppStore;
  const apiBase = options.apiBase ?? "";
  const api = createBatsimApi(apiBase);

  const zonesRes = await fetch("geo/texas-zones.json");
  if (!zonesRes.ok) throw new Error(`zone geojson fetch failed: HTTP ${zonesRes.status}`);
  const zones = parseZones(await zonesRes.json());
  if (zones.size === 0) throw new Error("zone geojson contained no load zones");

  let connection: "live" | "demo" = "live";
  let bundle: EntitiesBundle;
  const isLive = !options.forceDemo && (await api.health());
  if (isLive) {
    bundle = await loadLiveEntities(api);
  } else {
    connection = "demo";
    bundle = await loadDemoEntities(DEMO_TRACE_URL);
  }

  const live = createLiveBuffers(Math.max(1024, bundle.homes.length * 2));
  assignPositions(bundle.homes, zones, live);
  const neighborhood = pickNeighborhood(bundle.homes, zones);

  // Demo replay also publishes its recorded time bounds for the scrubber.
  let traceRangeMs: [number, number] | null = null;
  if (!isLive) {
    try {
      const res = await fetch(`${DEMO_TRACE_URL}/manifest.json`);
      if (res.ok) {
        const manifest = (await res.json()) as TraceManifest;
        const start = Date.parse(manifest.sim_time_range[0]);
        const end = Date.parse(manifest.sim_time_range[1]);
        if (Number.isFinite(start) && Number.isFinite(end) && end > start) {
          traceRangeMs = [start, end];
        }
      }
    } catch {
      // A missing manifest only costs the scrubber its bounds; replay runs.
    }
  }

  store.setState({
    connection,
    scenarioName: bundle.scenarioName,
    homesMeta: buildHomeMeta(bundle),
    homeOrder: bundle.homes.map((h) => h.id),
    neighborhoodZone: neighborhood.zone,
    neighborhoodHomeIds: neighborhood.homeIds,
    neighborhoodAnchor: neighborhood.anchor,
    centerZone: neighborhood.zone,
    zoneAnchors: Object.fromEntries([...zones.values()].map((z) => [z.zone, z.anchor])),
    zoneLabels: Object.fromEntries([...zones.values()].map((z) => [z.zone, z.label])),
    catalog: bundle.batteries.map(
      (b): CatalogBattery => ({
        modelId: b.model_id,
        displayName: b.display_name,
        vendor: b.vendor,
        chemistry: b.chemistry,
        coupling: b.coupling,
        usableEnergyKwh: b.usable_energy_kwh,
      }),
    ),
    batteryDetails: bundle.batteryDetails,
    traceRangeMs,
  });

  const ingest = createIngest(live);
  let transport: TelemetryTransport;
  let controller: SimController;
  if (isLive) {
    transport = new SseTransport({ baseUrl: apiBase, raw: true });
    controller = createLiveController(api, bundle.homes[0]?.config.fleet_id ?? null);
  } else {
    const replay = new ReplayTransport({ traceUrl: DEMO_TRACE_URL, speed: 60, loop: true });
    transport = replay;
    controller = createReplayController(replay);
  }
  void transport.start({
    onEvent: ingest.handleEvent,
    onOpen: () =>
      store.setState({
        lastError: null,
        canReplayDispatch:
          transport instanceof ReplayTransport ? transport.hasDispatch : false,
      }),
    onError: (message) => store.setState({ lastError: message }),
  });
  setController(controller);

  const result = { live, transport };
  setRuntime(result);
  return result;
}
