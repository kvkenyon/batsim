import { describe, expect, it } from "vitest";
import { parseStreamFrame } from "../src/state/frames";
import { priceColor, sampleRamp, socColor, PRICE_RAMP, SOC_RAMP } from "../src/tokens/tokens";

describe("parseStreamFrame", () => {
  const tickPayload = {
    sim_time: "2026-08-14T21:03:12Z",
    tick: 512,
    price_rtm: 41.2,
    fleets: [
      {
        fleet_id: "flt_1",
        homes: 2,
        battery_power_kw: -3.5,
        pv_power_kw: 8.1,
        load_power_kw: 4.2,
        grid_power_kw: -0.4,
        soc_mean: 0.62,
      },
    ],
    homes: [
      {
        home_id: "home_1",
        soc: 0.62,
        battery_power_kw: -1.8,
        pv_power_kw: 4.1,
        load_power_kw: 2.1,
        grid_power_kw: -0.2,
      },
      {
        home_id: "home_2",
        soc: 0.6,
        battery_power_kw: -1.7,
        pv_power_kw: 4.0,
        load_power_kw: 2.1,
        grid_power_kw: -0.2,
      },
    ],
  };

  it("parses a well-formed tick frame", () => {
    const frame = parseStreamFrame("tick", tickPayload);
    if (frame?.kind !== "tick") throw new Error("expected tick frame");
    expect(frame.tick).toBe(512);
    expect(frame.priceRtm).toBeCloseTo(41.2);
    expect(frame.simTimeMs).toBe(Date.parse("2026-08-14T21:03:12Z"));
    expect(frame.fleets).toHaveLength(1);
    expect(frame.homes).toHaveLength(2);
  });

  it("parses a tick frame without per-home rows", () => {
    const { homes: _omit, ...rest } = tickPayload;
    const frame = parseStreamFrame("tick", rest);
    if (frame?.kind !== "tick") throw new Error("expected tick frame");
    expect(frame.homes).toBeNull();
    expect(frame.fleets).toHaveLength(1);
  });

  it("rejects malformed payloads instead of throwing", () => {
    expect(parseStreamFrame("tick", null)).toBeNull();
    expect(parseStreamFrame("tick", { tick: "nope" })).toBeNull();
    expect(parseStreamFrame("tick", { sim_time: "not-a-date", tick: 1, price_rtm: 1 })).toBeNull();
    expect(parseStreamFrame("tick", 42)).toBeNull();
  });

  it("drops malformed home rows but keeps the frame", () => {
    const payload = {
      ...tickPayload,
      homes: [tickPayload.homes[0], { home_id: "broken" }],
    };
    const frame = parseStreamFrame("tick", payload);
    if (frame?.kind !== "tick") throw new Error("expected tick frame");
    expect(frame.homes).toHaveLength(1);
  });

  it("parses gap and dispatch events", () => {
    const gap = parseStreamFrame("gap", { reason: "raw_home_rows_suspended" });
    expect(gap).toEqual({ kind: "gap", reason: "raw_home_rows_suspended", detail: undefined });
    const dispatch = parseStreamFrame("dispatch", {
      command_id: "cmd_1",
      targets_applied: 10,
      targets_rejected: 1,
    });
    expect(dispatch).toEqual({ kind: "dispatch", commandId: "cmd_1", targetsApplied: 10, targetsRejected: 1 });
  });

  it("returns null for unknown event names", () => {
    expect(parseStreamFrame("mystery", {})).toBeNull();
  });
});

describe("color ramps", () => {
  it("samples ramp endpoints exactly", () => {
    expect(sampleRamp(SOC_RAMP, 0).toLowerCase()).toBe((SOC_RAMP[0] ?? "").toLowerCase());
    expect(sampleRamp(SOC_RAMP, 1).toLowerCase()).toBe(
      (SOC_RAMP[SOC_RAMP.length - 1] ?? "").toLowerCase(),
    );
  });

  it("clamps out-of-range input", () => {
    expect(sampleRamp(SOC_RAMP, -1)).toBe(sampleRamp(SOC_RAMP, 0));
    expect(sampleRamp(SOC_RAMP, 2)).toBe(sampleRamp(SOC_RAMP, 1));
  });

  it("maps extreme prices to ramp ends", () => {
    expect(priceColor(-100)).toBe(PRICE_RAMP[0]);
    expect(priceColor(5000)).toBe(PRICE_RAMP[PRICE_RAMP.length - 1]);
  });

  it("maps mid-range prices between stops", () => {
    expect(priceColor(50)).not.toBe(priceColor(300));
  });

  it("soc color moves from amber to sage as charge rises", () => {
    expect(socColor(0)).toBe(sampleRamp(SOC_RAMP, 0));
    expect(socColor(1)).toBe(sampleRamp(SOC_RAMP, 1));
    expect(socColor(0.2)).not.toBe(socColor(0.8));
  });
});
