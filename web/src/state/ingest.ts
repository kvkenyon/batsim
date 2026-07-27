/**
 * Frame ingest: parse → write slot buffers → publish a HUD-cadence
 * rollup. Transports deliver raw events here; this is the single ingest
 * path for live and replayed telemetry. Tick frames also feed the
 * grid-event detector, the per-home day accumulators, and the SOC
 * history rings behind the inspector sparkline.
 */

import { createEventDetector, type EventDetector } from "./events";
import { parseStreamFrame, type TickFrame } from "./frames";
import { resetEnergyAccumulators, SOC_HIST_CAP, type LiveBuffers } from "./live";
import { pushEvents, useAppStore } from "./store";

const HUD_COMMIT_MIN_MS = 250;

export interface Ingest {
  handleEvent: (event: string, data: unknown) => void;
}

export function createIngest(live: LiveBuffers): Ingest {
  let lastTick = -1;
  let lastSimMs = -1;
  let lastDayIndex = -1;
  let guardFailures = 0;
  let lastCommitWall = 0;
  const detector: EventDetector = createEventDetector();

  const commitHud = (frame: TickFrame, force: boolean) => {
    const now = performance.now();
    if (!force && now - lastCommitWall < HUD_COMMIT_MIN_MS) return;
    lastCommitWall = now;
    const fleet = frame.fleets[0];
    useAppStore.setState({
      priceRtm: frame.priceRtm,
      simTimeMs: frame.simTimeMs,
      tick: frame.tick,
      fleet: fleet
        ? {
            homes: fleet.homes,
            batteryKw: fleet.battery_power_kw,
            pvKw: fleet.pv_power_kw,
            loadKw: fleet.load_power_kw,
            gridKw: fleet.grid_power_kw,
            socMean: fleet.soc_mean,
          }
        : rollUpFromBuffers(live),
    });
  };

  /**
   * Integrate per-home energy and append SOC history. `dtHours` is the
   * sim-time step since the previous frame; accumulators reset on a sim
   * day rollover or a replay loop restart (tick moving backwards).
   */
  const accumulate = (frame: TickFrame, dtHours: number) => {
    const histReady = live.socHist.length > 0;
    for (const row of frame.homes ?? []) {
      const slot = live.slotOf.get(row.home_id);
      if (slot === undefined) continue;
      const batt = row.battery_power_kw;
      if (batt < 0) live.chargedKwh[slot] = (live.chargedKwh[slot] ?? 0) + -batt * dtHours;
      else if (batt > 0) live.dischargedKwh[slot] = (live.dischargedKwh[slot] ?? 0) + batt * dtHours;
      live.pvKwh[slot] = (live.pvKwh[slot] ?? 0) + row.pv_power_kw * dtHours;
      if (row.grid_power_kw > 0) {
        live.gridImportKwh[slot] = (live.gridImportKwh[slot] ?? 0) + row.grid_power_kw * dtHours;
      }
      if (histReady) live.socHist[slot * SOC_HIST_CAP + live.histPos] = row.soc;
    }
    if (histReady && (frame.homes?.length ?? 0) > 0) {
      live.histPos = (live.histPos + 1) % SOC_HIST_CAP;
      if (live.histLen < SOC_HIST_CAP) live.histLen += 1;
    }
  };

  return {
    handleEvent(event, data) {
      const frame = parseStreamFrame(event, data);
      if (!frame) {
        guardFailures += 1;
        if (guardFailures === 10) {
          useAppStore.setState({ lastError: "telemetry contract mismatch: dropping malformed frames" });
        }
        return;
      }
      if (frame.kind === "gap") {
        return;
      }
      if (frame.kind === "dispatch") {
        pushEvents([detector.onDispatch(frame)]);
        useAppStore.setState((state) => ({
          dispatchWave: state.dispatchWave + 1,
          dispatchStatus: `dispatch · ${frame.targetsApplied} homes acknowledged`,
        }));
        return;
      }
      if (lastTick >= 0 && frame.tick > lastTick + 1) {
        // Tick sequence gap: frames were lost upstream. Rings tolerate
        // holes; the HUD clock simply jumps to the newest frame.
      }
      const dayIndex = Math.floor(frame.simTimeMs / 86_400_000);
      const looped = frame.tick < lastTick;
      if (looped || (lastDayIndex >= 0 && dayIndex !== lastDayIndex)) {
        resetEnergyAccumulators(live);
      }
      const dtHours =
        lastSimMs >= 0 && frame.simTimeMs > lastSimMs ? (frame.simTimeMs - lastSimMs) / 3_600_000 : 0;
      lastTick = frame.tick;
      lastSimMs = frame.simTimeMs;
      lastDayIndex = dayIndex;

      live.priceRtm = frame.priceRtm;
      live.simTimeMs = frame.simTimeMs;
      live.tick = frame.tick;
      if (frame.homes) {
        for (const row of frame.homes) {
          const slot = live.slotOf.get(row.home_id);
          if (slot === undefined) continue;
          live.soc[slot] = row.soc;
          live.batteryKw[slot] = row.battery_power_kw;
          live.pvKw[slot] = row.pv_power_kw;
          live.loadKw[slot] = row.load_power_kw;
          live.gridKw[slot] = row.grid_power_kw;
        }
      }
      accumulate(frame, dtHours);
      live.version += 1;
      pushEvents(detector.onTick(frame));
      commitHud(frame, frame.homes === null);
    },
  };
}

function rollUpFromBuffers(live: LiveBuffers) {
  let battery = 0;
  let pv = 0;
  let load = 0;
  let grid = 0;
  let soc = 0;
  for (let i = 0; i < live.count; i++) {
    battery += live.batteryKw[i] ?? 0;
    pv += live.pvKw[i] ?? 0;
    load += live.loadKw[i] ?? 0;
    grid += live.gridKw[i] ?? 0;
    soc += live.soc[i] ?? 0;
  }
  return {
    homes: live.count,
    batteryKw: battery,
    pvKw: pv,
    loadKw: load,
    gridKw: grid,
    socMean: live.count > 0 ? soc / live.count : 0,
  };
}
