/**
 * Telemetry transports. A transport delivers raw stream events
 * (event name + JSON payload) to the ingest pipeline; it never parses.
 * The live transport reads server-sent events; the replay transport reads
 * a recorded trace file. Both are interchangeable downstream.
 */

export interface TransportHandlers {
  onEvent: (event: string, data: unknown) => void;
  onOpen: () => void;
  onError: (message: string) => void;
}

export interface TelemetryTransport {
  readonly kind: "live" | "replay";
  start(handlers: TransportHandlers): void;
  stop(): void;
}

export interface SseTransportOptions {
  /** Base URL for the API; empty string means same origin (dev proxy). */
  baseUrl: string;
  fleetId?: string;
  homeIds?: string[];
  raw: boolean;
  downsample?: number;
}

/** Live server-sent events from the telemetry stream endpoint. */
export class SseTransport implements TelemetryTransport {
  readonly kind = "live" as const;
  private source: EventSource | null = null;
  private stopped = false;

  constructor(private readonly options: SseTransportOptions) {}

  start(handlers: TransportHandlers): void {
    this.stopped = false;
    void this.open(handlers);
  }

  stop(): void {
    this.stopped = true;
    this.source?.close();
    this.source = null;
  }

  private url(raw: boolean): string {
    const params = new URLSearchParams();
    params.set("fields", raw ? "raw" : "aggregate");
    if (this.options.fleetId) params.set("fleet_id", this.options.fleetId);
    if (this.options.homeIds?.length) params.set("home_ids", this.options.homeIds.join(","));
    if (this.options.downsample && this.options.downsample > 1) {
      params.set("downsample", String(this.options.downsample));
    }
    return `${this.options.baseUrl}/v1/telemetry/stream?${params.toString()}`;
  }

  private async open(handlers: TransportHandlers): Promise<void> {
    let raw = this.options.raw;
    if (raw) {
      // The server caps raw streams; past the cap it answers 422, which
      // EventSource surfaces only as a terminal error. Preflight to drop
      // to fleet aggregates so large fleets still get live rollups.
      try {
        const probe = await fetch(this.url(true), { headers: { accept: "text/event-stream" } });
        if (probe.status === 422) raw = false;
        await probe.body?.cancel().catch(() => undefined);
      } catch {
        // Preflight unreachable: let the EventSource error path report it.
      }
    }
    if (this.stopped) return;
    const source = new EventSource(this.url(raw));
    this.source = source;
    source.onopen = () => handlers.onOpen();
    source.onerror = () => handlers.onError("telemetry stream connection lost");
    for (const event of ["tick", "gap", "dispatch"] as const) {
      source.addEventListener(event, (ev) => {
        try {
          handlers.onEvent(event, JSON.parse((ev as MessageEvent).data as string));
        } catch {
          handlers.onError(`malformed ${event} frame dropped`);
        }
      });
    }
  }
}

export interface TraceManifest {
  format: string;
  scenario: { name: string; seed: number; tick_seconds: number };
  tick_range: [number, number];
  sim_time_range: [string, string];
  homes: number;
}

export interface ReplayTransportOptions {
  /** URL of the trace directory containing manifest.json + telemetry.jsonl. */
  traceUrl: string;
  /** Playback speed relative to recorded sim time. */
  speed?: number;
  /** Loop back to the first frame when the trace ends. */
  loop?: boolean;
}

interface RecordedLine {
  event: string;
  [key: string]: unknown;
}

/** Price level treated as a jump-worthy price event during replay. */
const PRICE_EVENT_USD = 75;

/**
 * Replays a recorded trace through the same parser as live data. Lines
 * are indexed once, then emitted on a wall-clock schedule derived from
 * the sim-time gap between consecutive tick frames. The transport is a
 * full tape deck: pause/resume, live speed changes, and seek jumps to
 * the next recorded price event or fleet dispatch.
 */
export class ReplayTransport implements TelemetryTransport {
  readonly kind = "replay" as const;
  private timer: number | null = null;
  private stopped = false;
  private paused = false;
  private speed: number;
  private lines: RecordedLine[] = [];
  private index = 0;
  private lastSimMs: number | null = null;
  private lastWallMs: number | null = null;
  private handlers: TransportHandlers | null = null;
  /** True when the trace carries at least one dispatch event. */
  hasDispatch = false;

  constructor(private readonly options: ReplayTransportOptions) {
    this.speed = options.speed ?? 60;
  }

  async start(handlers: TransportHandlers): Promise<void> {
    this.stopped = false;
    let text: string;
    try {
      const res = await fetch(`${this.options.traceUrl}/telemetry.jsonl`);
      if (!res.ok) {
        handlers.onError(`trace fetch failed: HTTP ${res.status}`);
        return;
      }
      text = await res.text();
    } catch (err) {
      if (!this.stopped) {
        handlers.onError(
          `trace fetch failed: ${err instanceof Error ? err.message : String(err)}`,
        );
      }
      return;
    }
    if (this.stopped) return;
    const lines: RecordedLine[] = [];
    for (const raw of text.split("\n")) {
      const line = raw.trim();
      if (!line) continue;
      try {
        lines.push(JSON.parse(line) as RecordedLine);
      } catch {
        handlers.onError("malformed trace line dropped");
      }
    }
    if (lines.length === 0) {
      handlers.onError("trace is empty");
      return;
    }
    this.lines = lines;
    this.index = 0;
    this.lastSimMs = null;
    this.lastWallMs = null;
    this.hasDispatch = lines.some((l) => l.event === "dispatch");
    this.handlers = handlers;
    handlers.onOpen();
    this.scheduleNext(0);
  }

  stop(): void {
    this.stopped = true;
    if (this.timer !== null) {
      window.clearTimeout(this.timer);
      this.timer = null;
    }
  }

  pause(): void {
    this.paused = true;
    if (this.timer !== null) {
      window.clearTimeout(this.timer);
      this.timer = null;
    }
  }

  resume(): void {
    if (!this.paused || this.stopped || this.lines.length === 0) return;
    this.paused = false;
    this.lastWallMs = null;
    this.scheduleNext(0);
  }

  setSpeed(speed: number): void {
    if (speed > 0) this.speed = speed;
  }

  /** Seek to just before the next recorded dispatch command. */
  jumpToNextDispatch(): boolean {
    const found = this.seekForward((line) => line.event === "dispatch");
    return found;
  }

  /** Seek to the first tick at or after a sim time (epoch ms). */
  seekToSimTime(targetMs: number): boolean {
    // Scrubbing lands exactly on the target; the context back-up that
    // jump-to-event seeks use would leave the HUD minutes off the drag.
    const found = this.seekForward(
      (line) => {
        if (line.event !== "tick") return false;
        const sim = typeof line.sim_time === "string" ? Date.parse(line.sim_time) : NaN;
        return Number.isFinite(sim) && sim >= targetMs;
      },
      0,
    );
    // A paused tape shows nothing on seek; emit the landed frame once so
    // the scrubber's lighting, prices, and markers track the drag.
    if (found && this.paused) this.emitCurrent();
    return found;
  }

  /** Seek to just before the next tick that crosses into high price. */
  jumpToNextPriceEvent(): boolean {
    let prev = this.currentPrice();
    return this.seekForward((line) => {
      if (line.event !== "tick") return false;
      const price = typeof line.price_rtm === "number" ? line.price_rtm : NaN;
      const crosses = Number.isFinite(price) && prev < PRICE_EVENT_USD && price >= PRICE_EVENT_USD;
      if (Number.isFinite(price)) prev = price;
      return crosses;
    });
  }

  private currentPrice(): number {
    for (let i = this.index - 1; i >= 0; i--) {
      const line = this.lines[i];
      if (line?.event === "tick" && typeof line.price_rtm === "number") {
        return line.price_rtm;
      }
    }
    return 0;
  }

  /**
   * Scan forward (wrapping once) for a line matching `pred`, then back
   * up `back` ticks so a jump lands with context before the event.
   */
  private seekForward(pred: (line: RecordedLine) => boolean, back = 3): boolean {
    const n = this.lines.length;
    if (n === 0) return false;
    for (let step = 1; step <= n; step++) {
      const i = (this.index + step) % n;
      const line = this.lines[i];
      if (line && pred(line)) {
        let landing = i;
        for (let k = 0; k < back; k++) {
          const candidate = (landing - 1 + n) % n;
          if (this.lines[candidate]?.event !== "tick") break;
          landing = candidate;
        }
        this.index = landing;
        this.lastSimMs = null;
        this.lastWallMs = null;
        if (!this.paused) this.scheduleNext(0);
        return true;
      }
    }
    return false;
  }

  /** Emit the frame under the cursor once, advancing the cursor. */
  private emitCurrent(): void {
    if (this.stopped || !this.handlers) return;
    const line = this.lines[this.index];
    if (!line) return;
    this.index += 1;
    const { event, ...payload } = line;
    this.handlers.onEvent(event, payload);
  }

  private scheduleNext(delayMs: number): void {
    if (this.stopped || this.paused) return;
    if (this.timer !== null) window.clearTimeout(this.timer);
    this.timer = window.setTimeout(() => this.emitNext(), delayMs);
  }

  private emitNext(): void {
    if (this.stopped || this.paused) return;
    const handlers = this.handlers;
    if (!handlers) return;
    const loop = this.options.loop ?? true;
    if (this.index >= this.lines.length) {
      if (!loop) return;
      this.index = 0;
      this.lastSimMs = null;
      this.lastWallMs = null;
    }
    const line = this.lines[this.index];
    this.index += 1;
    if (!line) return;
    const { event, ...payload } = line;
    handlers.onEvent(event, payload);

    const simMs = typeof payload.sim_time === "string" ? Date.parse(payload.sim_time) : NaN;
    const now = performance.now();
    let delayMs = 1000 / 30;
    if (Number.isFinite(simMs) && this.lastSimMs !== null && this.lastWallMs !== null) {
      const simDelta = Math.max(0, simMs - this.lastSimMs);
      delayMs = Math.min(5000, Math.max(1, simDelta / this.speed));
    }
    if (Number.isFinite(simMs)) {
      this.lastSimMs = simMs;
      this.lastWallMs = now;
    }
    this.scheduleNext(delayMs);
  }
}
