/**
 * Design tokens: the single source of truth for color across DOM and WebGL.
 * Dark operations-console theme: near-black surfaces, cool neutral text,
 * and saturation reserved for energy states (amber discharge, sage
 * charge), the price ramp, and one restrained alert red. Values here feed
 * both CSS custom properties and the three.js material palette, so a
 * change lands everywhere at once.
 */

export const TOKENS = {
  bgBase: "#0B0E11",
  bgDeep: "#07090C",
  surface: "#12161C",
  surfaceRaised: "#1A2029",
  hairline: "#28303A",
  textPrimary: "#E8ECF1",
  textSecondary: "#98A2AE",
  textDim: "#5B6572",

  terrainBase: "#1E252E",
  terrainElev: "#8E97A3",
  terrainWater: "#1B2A30",
  slateLine: "#5C6672",
  groundSlab: "#20262F",
  streetSlab: "#2A323D",

  energyDischarge: "#E2A63D",
  energyDischargeDeep: "#8F6524",
  energyCharge: "#7FAE6B",
  energyChargeDeep: "#47643A",
  energySolar: "#A9C68A",
  energyExport: "#D0923F",

  alert: "#D0533A",
  alertDeep: "#7E2F20",
  warnAmber: "#C98A2E",

  outageDark: "#05070A",
} as const;

/** Five-stop price ramp: negative/cheap through scarcity. */
export const PRICE_RAMP = ["#2E4438", "#5C5A34", "#B98A3A", "#E2A63D", "#D0533A"] as const;

/** Ramp stops in $/MWh corresponding to PRICE_RAMP entries. */
export const PRICE_RAMP_STOPS_USD_MWH = [-10, 25, 75, 250, 1000] as const;

/** Three-stop state-of-charge ramp: depleted (red) through full (green). */
export const SOC_RAMP = ["#C25A45", "#D9A83E", "#7FAE6B"] as const;

function hexToRgb(hex: string): [number, number, number] {
  const n = parseInt(hex.slice(1), 16);
  return [((n >> 16) & 0xff) / 255, ((n >> 8) & 0xff) / 255, (n & 0xff) / 255];
}

function rgbToHex([r, g, b]: [number, number, number]): string {
  const c = (v: number) =>
    Math.round(Math.min(1, Math.max(0, v)) * 255)
      .toString(16)
      .padStart(2, "0");
  return `#${c(r)}${c(g)}${c(b)}`;
}

function lerp(a: number, b: number, t: number): number {
  return a + (b - a) * t;
}

/** Sample a multi-stop color ramp. t is clamped to [0, 1]. */
export function sampleRamp(stops: readonly string[], t: number): string {
  const x = Math.min(1, Math.max(0, t)) * (stops.length - 1);
  const i = Math.min(stops.length - 2, Math.floor(x));
  const a = hexToRgb(stops[i] ?? stops[0] ?? "#000000");
  const b = hexToRgb(stops[i + 1] ?? stops[stops.length - 1] ?? "#ffffff");
  const f = x - i;
  return rgbToHex([lerp(a[0], b[0], f), lerp(a[1], b[1], f), lerp(a[2], b[2], f)]);
}

/** Map a real-time price onto the five-stop price ramp. */
export function priceColor(usdPerMwh: number): string {
  const stops = PRICE_RAMP_STOPS_USD_MWH;
  const first = stops[0] ?? 0;
  const last = stops[stops.length - 1] ?? 1;
  if (usdPerMwh <= first) return PRICE_RAMP[0] ?? "#2E4438";
  if (usdPerMwh >= last) return PRICE_RAMP[PRICE_RAMP.length - 1] ?? "#D0533A";
  for (let i = 0; i < stops.length - 1; i++) {
    const lo = stops[i] ?? first;
    const hi = stops[i + 1] ?? last;
    if (usdPerMwh >= lo && usdPerMwh <= hi) {
      const a = hexToRgb(PRICE_RAMP[i] ?? "#000000");
      const b = hexToRgb(PRICE_RAMP[i + 1] ?? "#ffffff");
      const f = (usdPerMwh - lo) / (hi - lo);
      return rgbToHex([lerp(a[0], b[0], f), lerp(a[1], b[1], f), lerp(a[2], b[2], f)]);
    }
  }
  return PRICE_RAMP[0] ?? "#2E4438";
}

/** Map state of charge (0..1) onto the SOC ramp. */
export function socColor(soc: number): string {
  return sampleRamp(SOC_RAMP, soc);
}

/** Map state of charge (0..1) onto the SOC ramp as linear RGB floats. */
export function socColorRgb(soc: number): [number, number, number] {
  return hexToRgb(socColor(soc));
}

/**
 * Price color for small markers: the ramp lifted toward bone so cheap
 * prices stay legible on the dark basemap. Hue semantics preserved.
 */
export function priceMarkerColor(usdPerMwh: number): string {
  const base = hexToRgb(priceColor(usdPerMwh));
  const lift = hexToRgb("#E8ECF1");
  const t = 0.35;
  return rgbToHex([
    base[0] + (lift[0] - base[0]) * t,
    base[1] + (lift[1] - base[1]) * t,
    base[2] + (lift[2] - base[2]) * t,
  ]);
}
