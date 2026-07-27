/**
 * Map flow overlay: a 2D canvas layered over the MapLibre canvas that
 * draws what the vector layers cannot - moving things. Service-line
 * particles run from each home toward its zone anchor (direction = the
 * home's real energy direction, rate ∝ |grid kW|), and dispatch commands
 * play as an expanding ripple with per-home acknowledgment pops on a
 * jittered per-home delay, the way a real cloud fleet acks.
 *
 * Everything here reads the live buffers directly inside a rAF loop;
 * nothing goes through React. Particle work is viewport-culled and
 * capped, and switches off entirely below a zoom where the dots are a
 * few pixels apart.
 */

import type { Map as MaplibreMap } from "maplibre-gl";
import type { LiveBuffers } from "../state/live";
import { TOKENS } from "../tokens/tokens";

/** Below this zoom the map shows dots only; particles would be noise. */
const FLOW_MIN_ZOOM = 9;
/** Most homes animated at once, ranked by |grid kW|. */
const MAX_FLOW_HOMES = 1500;
const PARTICLES_PER_HOME = 3;
/** Most homes ack-flashed during a dispatch wave. */
const MAX_WAVE_HOMES = 4000;
const WAVE_DURATION_S = 2.6;
const ACK_JITTER_S = 1.4;

interface Rgb {
  r: number;
  g: number;
  b: number;
}

function rgb(hex: string): Rgb {
  const n = parseInt(hex.slice(1), 16);
  return { r: (n >> 16) & 0xff, g: (n >> 8) & 0xff, b: n & 0xff };
}

const EXPORT_RGB = rgb(TOKENS.energyExport);
const CHARGE_RGB = rgb(TOKENS.energyCharge);
const IMPORT_RGB = rgb(TOKENS.slateLine);
const WAVE_RGB = rgb(TOKENS.energyDischarge);

/** Small stable hash → [0, 1); drives per-home phases and ack jitter. */
function hash01(id: string): number {
  let h = 2166136261;
  for (let i = 0; i < id.length; i++) {
    h ^= id.charCodeAt(i);
    h = Math.imul(h, 16777619);
  }
  return ((h >>> 0) % 10000) / 10000;
}

export class MapFlowOverlay {
  private canvas: HTMLCanvasElement;
  private ctx: CanvasRenderingContext2D;
  private map: MaplibreMap | null = null;
  private live: LiveBuffers | null = null;
  private raf = 0;
  private running = false;
  private waveStartS: number | null = null;
  private readonly onMove = () => {
    this.positionsDirty = true;
  };
  private positionsDirty = true;
  private projected = new Float32Array(0);

  constructor(private readonly container: HTMLElement) {
    this.canvas = document.createElement("canvas");
    this.canvas.style.position = "absolute";
    this.canvas.style.inset = "0";
    this.canvas.style.pointerEvents = "none";
    this.canvas.style.zIndex = "4";
    const ctx = this.canvas.getContext("2d");
    if (!ctx) throw new Error("flow overlay: 2d context unavailable");
    this.ctx = ctx;
    container.appendChild(this.canvas);
  }

  attach(map: MaplibreMap, live: LiveBuffers): void {
    this.map = map;
    this.live = live;
    map.on("move", this.onMove);
    map.on("resize", this.onMove);
    this.resize();
  }

  detach(): void {
    this.stop();
    this.map?.off("move", this.onMove);
    this.map?.off("resize", this.onMove);
    this.map = null;
    this.live = null;
    this.canvas.remove();
  }

  start(): void {
    if (this.running) return;
    this.running = true;
    const loop = () => {
      if (!this.running) return;
      this.draw();
      this.raf = requestAnimationFrame(loop);
    };
    this.raf = requestAnimationFrame(loop);
  }

  stop(): void {
    this.running = false;
    cancelAnimationFrame(this.raf);
  }

  /** Begin the dispatch ripple + per-home ack pops. */
  triggerDispatchWave(): void {
    this.waveStartS = performance.now() / 1000;
  }

  private resize(): void {
    const dpr = Math.min(2, window.devicePixelRatio || 1);
    const w = this.container.clientWidth;
    const h = this.container.clientHeight;
    this.canvas.width = Math.max(1, Math.round(w * dpr));
    this.canvas.height = Math.max(1, Math.round(h * dpr));
    this.canvas.style.width = `${w}px`;
    this.canvas.style.height = `${h}px`;
    this.ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    this.positionsDirty = true;
  }

  /** Reproject every home + anchor into screen space (post-move only). */
  private reproject(): void {
    const map = this.map;
    const live = this.live;
    if (!map || !live) return;
    const n = live.count;
    if (this.projected.length < n * 4) this.projected = new Float32Array(n * 4);
    const bounds = map.getBounds();
    const west = bounds.getWest();
    const east = bounds.getEast();
    const south = bounds.getSouth();
    const north = bounds.getNorth();
    const padLng = (east - west) * 0.05;
    const padLat = (north - south) * 0.05;
    for (let i = 0; i < n; i++) {
      const lng = live.lng[i] ?? 0;
      const lat = live.lat[i] ?? 0;
      const o = i * 4;
      if (
        lng < west - padLng ||
        lng > east + padLng ||
        lat < south - padLat ||
        lat > north + padLat
      ) {
        this.projected[o] = NaN;
        continue;
      }
      const p = map.project([lng, lat]);
      const a = map.project([live.anchorLng[i] ?? lng, live.anchorLat[i] ?? lat]);
      this.projected[o] = p.x;
      this.projected[o + 1] = p.y;
      this.projected[o + 2] = a.x;
      this.projected[o + 3] = a.y;
    }
    this.positionsDirty = false;
  }

  private draw(): void {
    const map = this.map;
    const live = this.live;
    if (!map || !live) return;
    if (this.container.clientWidth !== this.canvas.clientWidth) this.resize();
    const ctx = this.ctx;
    const w = this.container.clientWidth;
    const h = this.container.clientHeight;
    ctx.clearRect(0, 0, w, h);
    const t = performance.now() / 1000;
    const n = live.count;
    if (n === 0) return;
    if (this.positionsDirty) this.reproject();

    const waveAge = this.waveStartS === null ? Infinity : t - this.waveStartS;
    const waveActive = waveAge < WAVE_DURATION_S;

    // Dispatch ripple: an expanding hairline ring from the fleet centroid.
    if (waveActive) {
      let cx = 0;
      let cy = 0;
      let c = 0;
      for (let i = 0; i < n; i++) {
        const o = i * 4;
        const x = this.projected[o];
        if (x === undefined || Number.isNaN(x)) continue;
        cx += x;
        cy += this.projected[o + 1] ?? 0;
        c++;
      }
      if (c > 0) {
        cx /= c;
        cy /= c;
        const prog = waveAge / WAVE_DURATION_S;
        const radius = 40 + prog * Math.max(w, h) * 0.9;
        const alpha = 0.4 * (1 - prog);
        ctx.strokeStyle = `rgba(${WAVE_RGB.r},${WAVE_RGB.g},${WAVE_RGB.b},${alpha.toFixed(3)})`;
        ctx.lineWidth = 1.5;
        ctx.beginPath();
        ctx.arc(cx, cy, radius, 0, Math.PI * 2);
        ctx.stroke();
      }
    }

    const zoom = map.getZoom();
    const drawFlows = zoom >= FLOW_MIN_ZOOM;

    // Rank homes by |grid kW| when the fleet exceeds the animation cap.
    let order: number[] | null = null;
    const cap = waveActive ? MAX_WAVE_HOMES : MAX_FLOW_HOMES;
    if (n > cap) {
      order = Array.from({ length: n }, (_, i) => i);
      order.sort((a, b) => Math.abs(live.gridKw[b] ?? 0) - Math.abs(live.gridKw[a] ?? 0));
      order.length = cap;
    }

    const drawHome = (i: number) => {
      const o = i * 4;
      const x = this.projected[o];
      if (x === undefined || Number.isNaN(x)) return;
      const y = this.projected[o + 1] ?? 0;
      const gridKw = live.gridKw[i] ?? 0;

      // Ack pop: a bright flash that lands on a per-home jittered delay.
      if (waveActive && this.waveStartS !== null) {
        const id = live.homeIds[i] ?? "";
        const delay = hash01(id) * ACK_JITTER_S;
        const age = waveAge - delay;
        if (age > 0 && age < 0.9) {
          const alpha = 0.85 * (1 - age / 0.9);
          const r = 3 + age * 6;
          ctx.fillStyle = `rgba(${WAVE_RGB.r},${WAVE_RGB.g},${WAVE_RGB.b},${alpha.toFixed(3)})`;
          ctx.beginPath();
          ctx.arc(x, y, r, 0, Math.PI * 2);
          ctx.fill();
        }
      }

      if (!drawFlows || Math.abs(gridKw) < 0.15) return;
      const ax = this.projected[o + 2] ?? x;
      const ay = this.projected[o + 3] ?? y;
      let dx = ax - x;
      let dy = ay - y;
      const dist = Math.hypot(dx, dy);
      if (dist < 2) return;
      // The service drop reads as a short leader toward the anchor.
      const lead = Math.min(dist, 34);
      dx /= dist;
      dy /= dist;
      const exporting = gridKw < 0;
      const charging = !exporting && (live.batteryKw[i] ?? 0) < -0.05;
      const c = exporting ? EXPORT_RGB : charging ? CHARGE_RGB : IMPORT_RGB;
      // Exporting flows run home → anchor; importing flows anchor → home.
      const dir = exporting ? 1 : -1;
      const speed = Math.min(3, Math.max(0.25, Math.abs(gridKw) / 5)) * 22;
      const id = live.homeIds[i] ?? "";
      const phase = hash01(id);
      for (let k = 0; k < PARTICLES_PER_HOME; k++) {
        const frac = ((phase + k / PARTICLES_PER_HOME + (t * speed * dir) / lead) % 1 + 1) % 1;
        const px = x + dx * lead * frac;
        const py = y + dy * lead * frac;
        ctx.fillStyle = `rgba(${c.r},${c.g},${c.b},0.95)`;
        ctx.fillRect(px - 1.5, py - 1.5, 3, 3);
      }
    };

    if (order) for (const i of order) drawHome(i);
    else for (let i = 0; i < n; i++) drawHome(i);
  }
}
