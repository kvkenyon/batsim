/**
 * Frame ingest: parse → write slot buffers → publish a HUD-cadence
 * rollup. Transports deliver raw events here; this is the single ingest
 * path for live and replayed telemetry.
 */

import { parseStreamFrame, type TickFrame } from "./frames";
import type { LiveBuffers } from "./live";
import { useAppStore } from "./store";

const HUD_COMMIT_MIN_MS = 250;

export interface Ingest {
  handleEvent: (event: string, data: unknown) => void;
}

export function createIngest(live: LiveBuffers): Ingest {
  let lastTick = -1;
  let guardFailures = 0;
  let lastCommitWall = 0;

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
        return;
      }
      if (lastTick >= 0 && frame.tick > lastTick + 1) {
        // Tick sequence gap: frames were lost upstream. Rings tolerate
        // holes; the HUD clock simply jumps to the newest frame.
      }
      lastTick = frame.tick;

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
      live.version += 1;
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
