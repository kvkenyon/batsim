import { describe, expect, it } from "vitest";
import { createEventDetector } from "../src/state/events";
import type { DispatchFrame, TickFrame } from "../src/state/frames";

function tick(over: Partial<TickFrame> & { simTimeMs: number }): TickFrame {
  return {
    kind: "tick",
    tick: 1,
    priceRtm: 30,
    fleets: [
      {
        fleet_id: "flt_1",
        homes: 64,
        battery_power_kw: 0,
        pv_power_kw: 0,
        load_power_kw: 100,
        grid_power_kw: 100,
        soc_mean: 0.5,
      },
    ],
    homes: null,
    ...over,
  };
}

const T0 = Date.parse("2025-06-15T12:00:00Z"); // 07:00 CT
const HOUR = 3_600_000;

describe("event detector", () => {
  it("flags a price spike as critical once, not on every tick", () => {
    const det = createEventDetector();
    det.onTick(tick({ simTimeMs: T0, priceRtm: 30 }));
    const spike = det.onTick(tick({ simTimeMs: T0 + HOUR, priceRtm: 900 }));
    expect(spike.some((e) => e.kind === "price" && e.severity === "critical")).toBe(true);
    expect(spike.some((e) => e.kind === "scarcity")).toBe(true);
    // Holding at the same price repeats nothing.
    const hold = det.onTick(tick({ simTimeMs: T0 + HOUR + 60_000, priceRtm: 910 }));
    expect(hold.filter((e) => e.kind === "price" || e.kind === "scarcity")).toHaveLength(0);
  });

  it("announces a negative-price charging window", () => {
    const det = createEventDetector();
    det.onTick(tick({ simTimeMs: T0, priceRtm: 25 }));
    const events = det.onTick(tick({ simTimeMs: T0 + HOUR, priceRtm: -4 }));
    expect(events.some((e) => e.kind === "price" && e.message.includes("negative"))).toBe(true);
  });

  it("tracks fleet charge and discharge swings with scaled thresholds", () => {
    const det = createEventDetector();
    const fleet = (kw: number) => [
      {
        fleet_id: "flt_1",
        homes: 64,
        battery_power_kw: kw,
        pv_power_kw: 0,
        load_power_kw: 100,
        grid_power_kw: 100,
        soc_mean: 0.5,
      },
    ];
    det.onTick(tick({ simTimeMs: T0, fleets: fleet(0) }));
    const dis = det.onTick(tick({ simTimeMs: T0 + HOUR, fleets: fleet(280) }));
    expect(dis.some((e) => e.kind === "fleet" && e.message.includes("discharging"))).toBe(true);
    const idle = det.onTick(tick({ simTimeMs: T0 + 2 * HOUR, fleets: fleet(2) }));
    expect(idle.some((e) => e.message.includes("idle"))).toBe(true);
  });

  it("marks scenario time at fixed Central hours", () => {
    const det = createEventDetector();
    const events = det.onTick(tick({ simTimeMs: T0 })); // 07:00 CT, not a marker hour
    expect(events.filter((e) => e.kind === "time")).toHaveLength(0);
    const noon = det.onTick(tick({ simTimeMs: Date.parse("2025-06-15T17:00:00Z") })); // 12:00 CT
    expect(noon.some((e) => e.kind === "time" && e.message.includes("12:00"))).toBe(true);
  });

  it("turns dispatch frames into feed entries with rejection counts", () => {
    const det = createEventDetector();
    det.onTick(tick({ simTimeMs: T0 }));
    const frame: DispatchFrame = {
      kind: "dispatch",
      commandId: "cmd_1",
      targetsApplied: 50,
      targetsRejected: 14,
    };
    const event = det.onDispatch(frame);
    expect(event.kind).toBe("dispatch");
    expect(event.severity).toBe("watch");
    expect(event.message).toContain("50");
    expect(event.message).toContain("14 rejected");
    expect(event.simTimeMs).toBe(T0);
  });

  it("marks the solar ramp once per day", () => {
    const det = createEventDetector();
    const fleet = (pv: number) => [
      {
        fleet_id: "flt_1",
        homes: 64,
        battery_power_kw: 0,
        pv_power_kw: pv,
        load_power_kw: 100,
        grid_power_kw: 100,
        soc_mean: 0.5,
      },
    ];
    det.onTick(tick({ simTimeMs: T0, fleets: fleet(0) }));
    const ramp = det.onTick(tick({ simTimeMs: T0 + HOUR, fleets: fleet(120) }));
    expect(ramp.some((e) => e.kind === "pv")).toBe(true);
    const again = det.onTick(tick({ simTimeMs: T0 + 2 * HOUR, fleets: fleet(200) }));
    expect(again.filter((e) => e.kind === "pv")).toHaveLength(0);
  });
});
