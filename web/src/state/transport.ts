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

  constructor(private readonly options: SseTransportOptions) {}

  start(handlers: TransportHandlers): void {
    const params = new URLSearchParams();
    params.set("fields", this.options.raw ? "raw" : "aggregate");
    if (this.options.fleetId) params.set("fleet_id", this.options.fleetId);
    if (this.options.homeIds?.length) params.set("home_ids", this.options.homeIds.join(","));
    if (this.options.downsample && this.options.downsample > 1) {
      params.set("downsample", String(this.options.downsample));
    }
    const url = `${this.options.baseUrl}/v1/telemetry/stream?${params.toString()}`;
    const source = new EventSource(url);
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

  stop(): void {
    this.source?.close();
    this.source = null;
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

/**
 * Replays a recorded trace through the same parser as live data. Lines
 * are indexed once, then emitted on a wall-clock schedule derived from
 * the sim-time gap between consecutive tick frames.
 */
export class ReplayTransport implements TelemetryTransport {
  readonly kind = "replay" as const;
  private timer: number | null = null;
  private stopped = false;

  constructor(private readonly options: ReplayTransportOptions) {}

  async start(handlers: TransportHandlers): Promise<void> {
    this.stopped = false;
    const res = await fetch(`${this.options.traceUrl}/telemetry.jsonl`);
    if (!res.ok) {
      handlers.onError(`trace fetch failed: HTTP ${res.status}`);
      return;
    }
    const text = await res.text();
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
    handlers.onOpen();

    const speed = this.options.speed ?? 60;
    const loop = this.options.loop ?? true;
    let index = 0;
    let lastSimMs: number | null = null;
    let lastWallMs: number | null = null;

    const emitNext = () => {
      if (this.stopped) return;
      if (index >= lines.length) {
        if (!loop) return;
        index = 0;
        lastSimMs = null;
        lastWallMs = null;
      }
      const line = lines[index];
      index += 1;
      if (!line) return;
      const { event, ...payload } = line;
      handlers.onEvent(event, payload);

      const simMs = typeof payload.sim_time === "string" ? Date.parse(payload.sim_time) : NaN;
      const now = performance.now();
      let delayMs = 1000 / 30;
      if (Number.isFinite(simMs) && lastSimMs !== null && lastWallMs !== null) {
        const simDelta = Math.max(0, simMs - lastSimMs);
        delayMs = Math.min(5000, Math.max(1, simDelta / speed));
      }
      if (Number.isFinite(simMs)) {
        lastSimMs = simMs;
        lastWallMs = now;
      }
      this.timer = window.setTimeout(emitNext, delayMs);
    };
    emitNext();
  }

  stop(): void {
    this.stopped = true;
    if (this.timer !== null) {
      window.clearTimeout(this.timer);
      this.timer = null;
    }
  }
}
