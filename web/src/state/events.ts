/**
 * Grid-event detection. Watches the telemetry stream for meaningful
 * transitions - price crossings, scarcity territory, solar ramps, fleet
 * charge/discharge swings, dispatch acknowledgments, scenario time
 * markers - and turns them into timestamped feed entries. Detection is
 * stateful and deduplicated so flapping telemetry cannot strobe the
 * feed. Pure aside from the sim timestamps carried on the frames.
 */

import type { DispatchFrame, TickFrame } from "./frames";

export type EventSeverity = "info" | "watch" | "critical";
export type EventKind = "price" | "scarcity" | "pv" | "fleet" | "dispatch" | "time";

export interface GridEvent {
  id: number;
  simTimeMs: number;
  kind: EventKind;
  severity: EventSeverity;
  message: string;
}

const PRICE_WATCH_USD = 75;
const PRICE_SPIKE_USD = 250;
const PRICE_SCARCITY_USD = 500;
const PRICE_SETTLED_USD = 40;
/** Sim-time cooldowns so a hovering value does not retrigger an entry. */
const PRICE_COOLDOWN_MS = 15 * 60_000;
const FLEET_COOLDOWN_MS = 5 * 60_000;
const TIME_MARKER_EVERY_H = 4;

export interface EventDetector {
  onTick: (frame: TickFrame) => GridEvent[];
  onDispatch: (frame: DispatchFrame) => GridEvent;
}

export function createEventDetector(): EventDetector {
  let nextId = 1;
  let lastPriceBand = 0;
  let lastPriceEventMs = -Infinity;
  let lastScarcityMs = -Infinity;
  let fleetState: "charging" | "discharging" | "idle" | null = null;
  let lastFleetEventMs = -Infinity;
  let pvDailyPeakKw = 0;
  let pvRampMarkedDay = -1;
  let lastTimeMarkerHour = -1;
  let lastDay = -1;
  let lastSimMs = 0;

  const make = (
    simTimeMs: number,
    kind: EventKind,
    severity: EventSeverity,
    message: string,
  ): GridEvent => ({ id: nextId++, simTimeMs, kind, severity, message });

  const priceBand = (price: number): number =>
    price >= PRICE_SPIKE_USD ? 3 : price >= PRICE_WATCH_USD ? 2 : price <= 0 ? -1 : price <= PRICE_SETTLED_USD ? 0 : 1;

  return {
    onTick(frame) {
      const out: GridEvent[] = [];
      const now = frame.simTimeMs;
      lastSimMs = now;
      const dayIndex = Math.floor(now / 86_400_000);
      if (dayIndex !== lastDay) {
        lastDay = dayIndex;
        pvDailyPeakKw = 0;
        pvRampMarkedDay = -1;
      }

      // Price band transitions.
      const band = priceBand(frame.priceRtm);
      if (band !== lastPriceBand && now - lastPriceEventMs >= PRICE_COOLDOWN_MS) {
        if (band === 3) {
          out.push(make(now, "price", "critical", `price spike · $${frame.priceRtm.toFixed(0)}/MWh`));
          lastPriceEventMs = now;
        } else if (band === 2 && lastPriceBand < 2) {
          out.push(make(now, "price", "watch", `price rising · $${frame.priceRtm.toFixed(0)}/MWh`));
          lastPriceEventMs = now;
        } else if (band === -1 && lastPriceBand > -1) {
          out.push(make(now, "price", "watch", `negative prices · $${frame.priceRtm.toFixed(0)}/MWh charging window`));
          lastPriceEventMs = now;
        } else if (band === 0 && lastPriceBand >= 2) {
          out.push(make(now, "price", "info", `price settled · $${frame.priceRtm.toFixed(0)}/MWh`));
          lastPriceEventMs = now;
        }
      }
      if (frame.priceRtm >= PRICE_SCARCITY_USD && now - lastScarcityMs >= PRICE_COOLDOWN_MS) {
        out.push(make(now, "scarcity", "critical", `scarcity watch · $${frame.priceRtm.toFixed(0)}/MWh`));
        lastScarcityMs = now;
      }
      lastPriceBand = band;

      const fleet = frame.fleets[0];
      if (fleet) {
        // Solar ramp: the day's first crossing of a fleet-scaled
        // output floor, once the day has shown real generation.
        if (fleet.pv_power_kw > pvDailyPeakKw) pvDailyPeakKw = fleet.pv_power_kw;
        if (
          pvRampMarkedDay !== dayIndex &&
          pvDailyPeakKw > Math.max(20, fleet.homes * 0.5) &&
          fleet.pv_power_kw > Math.max(10, fleet.homes * 0.25)
        ) {
          pvRampMarkedDay = dayIndex;
          out.push(make(now, "pv", "info", `solar ramp · fleet pv ${(fleet.pv_power_kw / 1000).toFixed(2)} MW`));
        }

        // Fleet battery swings, threshold scaled with fleet size.
        const batt = fleet.battery_power_kw;
        const threshold = Math.max(8, fleet.homes * 0.4);
        const state = batt > threshold ? "discharging" : batt < -threshold ? "charging" : "idle";
        if (fleetState !== null && state !== fleetState && now - lastFleetEventMs >= FLEET_COOLDOWN_MS) {
          if (state === "discharging") {
            out.push(make(now, "fleet", "watch", `fleet discharging · ${(batt / 1000).toFixed(2)} MW`));
            lastFleetEventMs = now;
          } else if (state === "charging") {
            out.push(make(now, "fleet", "info", `fleet charging · ${(Math.abs(batt) / 1000).toFixed(2)} MW`));
            lastFleetEventMs = now;
          } else if (fleetState !== "idle") {
            out.push(make(now, "fleet", "info", "fleet returned to idle"));
            lastFleetEventMs = now;
          }
        }
        fleetState = state;
      }

      // Scenario time markers at fixed Central hours.
      const ctHour = new Date(now).toLocaleString("en-US", {
        timeZone: "America/Chicago",
        hour: "2-digit",
        hourCycle: "h23",
      });
      const hour = Number(ctHour) % 24;
      if (hour % TIME_MARKER_EVERY_H === 0 && hour !== lastTimeMarkerHour) {
        lastTimeMarkerHour = hour;
        out.push(make(now, "time", "info", `scenario time ${String(hour).padStart(2, "0")}:00 CT`));
      } else if (hour % TIME_MARKER_EVERY_H !== 0) {
        lastTimeMarkerHour = -1;
      }

      return out;
    },

    onDispatch(frame) {
      const rejected =
        frame.targetsRejected > 0 ? ` · ${frame.targetsRejected} rejected` : "";
      return make(
        lastSimMs,
        "dispatch",
        frame.targetsRejected > 0 ? "watch" : "info",
        `dispatch ack · ${frame.targetsApplied} homes${rejected}`,
      );
    },
  };
}
