/**
 * Slot-indexed telemetry buffers. Every home owns a stable slot for the
 * session; per-tick ingest writes straight into these arrays, and the
 * render loop reads them without going through React state. The zustand
 * store carries the same data at HUD cadence; these buffers are the
 * high-frequency path.
 */

/** Sparkline history depth: 24 h of 60-second ticks. */
export const SOC_HIST_CAP = 1440;
/** Above this fleet size per-home history is skipped to bound memory. */
const HIST_MAX_HOMES = 2048;

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
  /** Service anchor (zone centroid) per slot; flow lines point here. */
  anchorLng: Float32Array;
  anchorLat: Float32Array;
  /** Per-home energy accumulators for the current sim day (kWh). */
  chargedKwh: Float32Array;
  dischargedKwh: Float32Array;
  pvKwh: Float32Array;
  gridImportKwh: Float32Array;
  /**
   * Per-home SOC ring history, slot-major rows of SOC_HIST_CAP samples;
   * empty when the fleet exceeds HIST_MAX_HOMES.
   */
  socHist: Float32Array;
  histLen: number;
  histPos: number;
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
    anchorLng: new Float32Array(capacity),
    anchorLat: new Float32Array(capacity),
    chargedKwh: new Float32Array(capacity),
    dischargedKwh: new Float32Array(capacity),
    pvKwh: new Float32Array(capacity),
    gridImportKwh: new Float32Array(capacity),
    socHist: capacity <= HIST_MAX_HOMES ? new Float32Array(capacity * SOC_HIST_CAP) : new Float32Array(0),
    histLen: 0,
    histPos: 0,
    priceRtm: 0,
    simTimeMs: 0,
    tick: 0,
    version: 0,
  };
}

/** Zero the per-day accumulators (day rollover or replay loop restart). */
export function resetEnergyAccumulators(live: LiveBuffers): void {
  live.chargedKwh.fill(0);
  live.dischargedKwh.fill(0);
  live.pvKwh.fill(0);
  live.gridImportKwh.fill(0);
  live.histLen = 0;
  live.histPos = 0;
}
