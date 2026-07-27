/**
 * Day arc: maps simulated time to lighting. One pure function of the sim
 * clock feeds both the map's day veil and the neighborhood's light rig,
 * so the whole console darkens, warms at dusk, and brightens at noon in
 * lockstep with the virtual clock.
 */

import { TOKENS } from "../tokens/tokens";

export type DayPhase = "night" | "dawn" | "day" | "golden" | "dusk";

export interface DayArc {
  phase: DayPhase;
  /** 0 at bright noon, 1 in deep night. */
  darkness: number;
  /** Warm dusk/dawn tint strength, 0..1. */
  warmth: number;
  /** Opacity of the map veil (premultiplied from darkness). */
  veilOpacity: number;
  /** CSS color of the map veil. */
  veilColor: string;
  /** Neighborhood light rig. */
  hemiIntensity: number;
  sunIntensity: number;
  sunColor: string;
  skyColor: string;
}

const ctHourFormatter = new Intl.DateTimeFormat("en-US", {
  timeZone: "America/Chicago",
  hour: "2-digit",
  minute: "2-digit",
  hourCycle: "h23",
});

/** Decimal hour of the sim clock in Central Time. */
export function centralHour(simTimeMs: number): number {
  const parts = ctHourFormatter.format(new Date(simTimeMs));
  const [h, m] = parts.split(":");
  return (Number(h) % 24) + Number(m) / 60;
}

function smoothstep(edge0: number, edge1: number, x: number): number {
  const t = Math.min(1, Math.max(0, (x - edge0) / (edge1 - edge0)));
  return t * t * (3 - 2 * t);
}

function hexLerp(a: string, b: string, t: number): string {
  const pa = parseInt(a.slice(1), 16);
  const pb = parseInt(b.slice(1), 16);
  const c = (sa: number, sb: number) => Math.round(sa + (sb - sa) * t);
  const r = c((pa >> 16) & 0xff, (pb >> 16) & 0xff);
  const g = c((pa >> 8) & 0xff, (pb >> 8) & 0xff);
  const bl = c(pa & 0xff, pb & 0xff);
  return `#${r.toString(16).padStart(2, "0")}${g.toString(16).padStart(2, "0")}${bl.toString(16).padStart(2, "0")}`;
}

const SUN_DAY = "#F2E8D5";
const SUN_GOLDEN = "#E8C07A";
const SUN_DUSK = "#D98E4A";
const SUN_NIGHT = "#8E9298";
const SKY_DAY = "#151B23";
const SKY_NIGHT = TOKENS.bgDeep;
const VEIL_NIGHT = "#04060A";
const VEIL_DUSK = "#3A2208";

/**
 * Lighting for one instant of simulated time. Phase windows (Central):
 * dawn 05:00-07:30, day 07:30-16:30, golden 16:30-19:00, dusk
 * 19:00-21:00, night otherwise. Blends are smooth across the windows.
 */
export function dayArc(simTimeMs: number): DayArc {
  const h = centralHour(simTimeMs);

  let darkness: number;
  let warmth: number;
  let phase: DayPhase;
  if (h >= 7.5 && h < 16.5) {
    phase = "day";
    darkness = 0;
    warmth = 0;
  } else if (h >= 16.5 && h < 19) {
    phase = "golden";
    darkness = 0.22 * smoothstep(16.5, 19, h);
    warmth = smoothstep(16.5, 17.8, h);
  } else if (h >= 19 && h < 21) {
    phase = "dusk";
    darkness = 0.22 + 0.78 * smoothstep(19, 21, h);
    warmth = 1 - smoothstep(19.4, 21, h);
  } else if (h >= 5 && h < 7.5) {
    phase = "dawn";
    darkness = 1 - smoothstep(5, 7.5, h);
    warmth = smoothstep(5, 6.2, h) * (1 - smoothstep(6.6, 7.5, h));
  } else {
    phase = "night";
    darkness = 1;
    warmth = 0;
  }

  const sunColor =
    darkness >= 0.98
      ? SUN_NIGHT
      : warmth > 0.55
        ? hexLerp(SUN_GOLDEN, SUN_DUSK, (warmth - 0.55) / 0.45)
        : hexLerp(SUN_DAY, SUN_GOLDEN, warmth / 0.55);

  return {
    phase,
    darkness,
    warmth,
    veilOpacity: 0.52 * darkness,
    veilColor: hexLerp(VEIL_NIGHT, VEIL_DUSK, warmth * (1 - darkness) * 0.9),
    hemiIntensity: 0.16 + 1.0 * (1 - darkness),
    sunIntensity: 0.25 + 2.0 * (1 - darkness),
    sunColor,
    skyColor: hexLerp(SKY_DAY, SKY_NIGHT, darkness),
  };
}
