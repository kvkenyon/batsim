import { beforeEach, describe, expect, it, vi } from "vitest";
import { ReplayTransport, type TransportHandlers } from "../src/state/transport";

/**
 * Scrubber contract: seeking a paused tape emits the landed frame so the
 * HUD (lighting, prices, markers) tracks the drag without resuming
 * playback.
 */

function traceLines(): string {
  const rows: string[] = [];
  for (let i = 0; i < 10; i++) {
    rows.push(
      JSON.stringify({
        event: "tick",
        tick: i,
        sim_time: new Date(Date.UTC(2025, 5, 15, 15, i)).toISOString(),
        price_rtm: 40 + i,
        fleets: [],
        homes: [],
      }),
    );
  }
  return rows.join("\n");
}

interface Emitted {
  event: string;
  data: unknown;
}

describe("ReplayTransport scrubbing", () => {
  let emitted: Emitted[];
  const handlers: TransportHandlers = {
    onEvent: (event, data) => {
      emitted.push({ event, data });
    },
    onOpen: () => {},
    onError: () => {},
  };

  beforeEach(() => {
    emitted = [];
    // node has no DOM timers on window; the transport only uses these two.
    const win = {
      setTimeout: () => 1,
      clearTimeout: () => {},
    };
    Object.assign(globalThis, { window: win });
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => ({ ok: true, text: async () => traceLines() }) as Response),
    );
  });

  async function startedTransport(): Promise<ReplayTransport> {
    const transport = new ReplayTransport({ traceUrl: "traces/demo", speed: 60 });
    await transport.start(handlers);
    return transport;
  }

  it("seek while paused emits the first tick at or after the target", async () => {
    const transport = await startedTransport();
    transport.pause();
    const target = Date.UTC(2025, 5, 15, 15, 4, 30); // 04:30 -> tick 5
    expect(transport.seekToSimTime(target)).toBe(true);
    expect(emitted).toHaveLength(1);
    const frame = emitted[0]?.data as { sim_time?: string };
    expect(Date.parse(frame.sim_time ?? "")).toBe(Date.UTC(2025, 5, 15, 15, 5));
  });

  it("seek backwards wraps to the earlier tick", async () => {
    const transport = await startedTransport();
    transport.pause();
    const late = Date.UTC(2025, 5, 15, 15, 8);
    expect(transport.seekToSimTime(late)).toBe(true);
    emitted = [];
    const early = Date.UTC(2025, 5, 15, 15, 2);
    expect(transport.seekToSimTime(early)).toBe(true);
    expect(emitted).toHaveLength(1);
    const frame = emitted[0]?.data as { sim_time?: string };
    expect(Date.parse(frame.sim_time ?? "")).toBe(Date.UTC(2025, 5, 15, 15, 2));
  });

  it("seek backwards from mid-tape lands on the earlier tick", async () => {
    const transport = await startedTransport();
    transport.pause();
    const mid = Date.UTC(2025, 5, 15, 15, 5);
    expect(transport.seekToSimTime(mid)).toBe(true);
    emitted = [];
    const early = Date.UTC(2025, 5, 15, 15, 2);
    expect(transport.seekToSimTime(early)).toBe(true);
    expect(emitted).toHaveLength(1);
    const frame = emitted[0]?.data as { sim_time?: string };
    expect(Date.parse(frame.sim_time ?? "")).toBe(Date.UTC(2025, 5, 15, 15, 2));
  });

  it("seek past the end reports no landing", async () => {
    const transport = await startedTransport();
    transport.pause();
    expect(transport.seekToSimTime(Date.UTC(2026, 0, 1))).toBe(false);
    expect(emitted).toHaveLength(0);
  });
});
