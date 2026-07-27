/**
 * Design tokens: the single source of truth for color across DOM and WebGL.
 * Warm-neutral world; only energy states, prices, and rare alerts carry
 * saturation. Values here feed both CSS custom properties and the three.js
 * material palette, so a change lands everywhere at once.
 */

export const TOKENS = {
  bgBase: "#201C17",
  bgDeep: "#171410",
  surface: "#2B261F",
  surfaceRaised: "#383227",
  hairline: "#4D4536",
  textPrimary: "#EFE7D8",
  textSecondary: "#B0A591",
  textDim: "#7C7462",

  terrainBase: "#8B8371",
  terrainElev: "#A79B82",
  terrainWater: "#5E6B66",
  slateLine: "#6E7478",

  energyDischarge: "#DFA33C",
  energyDischargeDeep: "#9C6B21",
  energyCharge: "#86A96B",
  energyChargeDeep: "#4F6B3C",
  energySolar: "#A8BE85",
  energyExport: "#C7903F",

  alert: "#C4452F",
  alertDeep: "#7E2B1D",
  warnAmber: "#C77F2E",

  outageDark: "#0E0C09",
} as const;

/** Five-stop price ramp: negative/cheap through scarcity. */
export const PRICE_RAMP = ["#3A352C", "#6E6247", "#B98A3A", "#DFA33C", "#C4452F"] as const;

/** Ramp stops in $/MWh corresponding to PRICE_RAMP entries. */
export const PRICE_RAMP_STOPS_USD_MWH = [-10, 25, 75, 250, 1000] as const;

/** Three-stop state-of-charge ramp: low to full. */
export const SOC_RAMP = ["#DFA33C", "#A8A06B", "#86A96B"] as const;

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
  if (usdPerMwh <= first) return PRICE_RAMP[0] ?? "#3A352C";
  if (usdPerMwh >= last) return PRICE_RAMP[PRICE_RAMP.length - 1] ?? "#C4452F";
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
  return PRICE_RAMP[0] ?? "#3A352C";
}

/** Map state of charge (0..1) onto the SOC ramp. */
export function socColor(soc: number): string {
  return sampleRamp(SOC_RAMP, soc);
}

/** Map state of charge (0..1) onto the SOC ramp as linear RGB floats. */
export function socColorRgb(soc: number): [number, number, number] {
  return hexToRgb(socColor(soc));
}
