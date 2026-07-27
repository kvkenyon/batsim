/**
 * Slot-indexed telemetry buffers. Every home owns a stable slot for the
 * session; per-tick ingest writes straight into these arrays, and the
 * render loop reads them without going through React state. The zustand
 * store carries the same data at HUD cadence; these buffers are the
 * high-frequency path.
 */

export interface LiveBuffers {
  count: number;
  /** slot -> home id */
  homeIds: string[];
  /** home id -> slot */
  slotOf: Map<string, number>;
  soc: Float32Array;
  batteryKw: Float32Array;
  pvKw: Float32Array;
  loadKw: Float32Array;
  gridKw: Float32Array;
  /** Assigned map position per slot. */
  lng: Float32Array;
  lat: Float32Array;
  priceRtm: number;
  simTimeMs: number;
  tick: number;
  /** Bumped once per committed tick frame. */
  version: number;
}

export function createLiveBuffers(capacity: number): LiveBuffers {
  return {
    count: 0,
    homeIds: [],
    slotOf: new Map(),
    soc: new Float32Array(capacity),
    batteryKw: new Float32Array(capacity),
    pvKw: new Float32Array(capacity),
    loadKw: new Float32Array(capacity),
    gridKw: new Float32Array(capacity),
    lng: new Float32Array(capacity),
    lat: new Float32Array(capacity),
    priceRtm: 0,
    simTimeMs: 0,
    tick: 0,
    version: 0,
  };
}
