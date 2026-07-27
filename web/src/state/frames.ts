/**
 * Telemetry wire contracts and the shared frame parser. Live SSE and the
 * recorded demo trace both feed this exact parser, so the offline demo
 * exercises the same ingest path as a running server.
 */

export interface HomeTickRowWire {
  home_id: string;
  soc: number;
  battery_power_kw: number;
  pv_power_kw: number;
  load_power_kw: number;
  grid_power_kw: number;
}

export interface FleetTickWire {
  fleet_id: string;
  homes: number;
  battery_power_kw: number;
  pv_power_kw: number;
  load_power_kw: number;
  grid_power_kw: number;
  soc_mean: number;
}

export interface TickFrame {
  kind: "tick";
  tick: number;
  simTimeMs: number;
  priceRtm: number;
  fleets: FleetTickWire[];
  homes: HomeTickRowWire[] | null;
}

export interface GapFrame {
  kind: "gap";
  reason: string;
  detail?: string;
}

export interface DispatchFrame {
  kind: "dispatch";
  commandId: string;
  targetsApplied: number;
  targetsRejected: number;
}

export type StreamFrame = TickFrame | GapFrame | DispatchFrame;

function asNumber(v: unknown): number | null {
  return typeof v === "number" && Number.isFinite(v) ? v : null;
}

function asString(v: unknown): string | null {
  return typeof v === "string" ? v : null;
}

function parseHomeRow(v: unknown): HomeTickRowWire | null {
  if (typeof v !== "object" || v === null) return null;
  const o = v as Record<string, unknown>;
  const homeId = asString(o.home_id);
  const soc = asNumber(o.soc);
  const batt = asNumber(o.battery_power_kw);
  const pv = asNumber(o.pv_power_kw);
  const load = asNumber(o.load_power_kw);
  const grid = asNumber(o.grid_power_kw);
  if (homeId === null || soc === null || batt === null || pv === null || load === null || grid === null) {
    return null;
  }
  return { home_id: homeId, soc, battery_power_kw: batt, pv_power_kw: pv, load_power_kw: load, grid_power_kw: grid };
}

function parseFleet(v: unknown): FleetTickWire | null {
  if (typeof v !== "object" || v === null) return null;
  const o = v as Record<string, unknown>;
  const fleetId = asString(o.fleet_id);
  const homes = asNumber(o.homes);
  const socMean = asNumber(o.soc_mean);
  if (fleetId === null || homes === null || socMean === null) return null;
  return {
    fleet_id: fleetId,
    homes,
    soc_mean: socMean,
    battery_power_kw: asNumber(o.battery_power_kw) ?? 0,
    pv_power_kw: asNumber(o.pv_power_kw) ?? 0,
    load_power_kw: asNumber(o.load_power_kw) ?? 0,
    grid_power_kw: asNumber(o.grid_power_kw) ?? 0,
  };
}

/**
 * Parse one stream event payload. Returns null on contract violations;
 * callers drop the frame and count it, never throw in the ingest loop.
 */
export function parseStreamFrame(event: string, data: unknown): StreamFrame | null {
  if (event === "tick") {
    if (typeof data !== "object" || data === null) return null;
    const o = data as Record<string, unknown>;
    const tick = asNumber(o.tick);
    const simTime = asString(o.sim_time);
    const priceRtm = asNumber(o.price_rtm);
    if (tick === null || simTime === null || priceRtm === null) return null;
    const simTimeMs = Date.parse(simTime);
    if (!Number.isFinite(simTimeMs)) return null;
    const fleets = Array.isArray(o.fleets)
      ? (o.fleets.map(parseFleet).filter(Boolean) as FleetTickWire[])
      : [];
    const homes = Array.isArray(o.homes)
      ? (o.homes.map(parseHomeRow).filter(Boolean) as HomeTickRowWire[])
      : null;
    return { kind: "tick", tick, simTimeMs, priceRtm, fleets, homes };
  }
  if (event === "gap") {
    const o = (typeof data === "object" && data !== null ? data : {}) as Record<string, unknown>;
    return {
      kind: "gap",
      reason: asString(o.reason) ?? "unknown",
      detail: asString(o.detail) ?? undefined,
    };
  }
  if (event === "dispatch") {
    const o = (typeof data === "object" && data !== null ? data : {}) as Record<string, unknown>;
    const commandId = asString(o.command_id);
    if (commandId === null) return null;
    return {
      kind: "dispatch",
      commandId,
      targetsApplied: asNumber(o.targets_applied) ?? 0,
      targetsRejected: asNumber(o.targets_rejected) ?? 0,
    };
  }
  return null;
}
