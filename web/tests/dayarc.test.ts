import { describe, expect, it } from "vitest";
import { centralHour, dayArc } from "../src/state/dayArc";

/** Ms epoch for a Central-Time wall clock reading on a summer date (CDT, UTC-5). */
function ct(date: string, hhmm: string): number {
  return Date.parse(`${date}T${hhmm}:00-05:00`);
}

describe("centralHour", () => {
  it("converts UTC to the Central wall clock", () => {
    expect(centralHour(Date.parse("2025-06-15T17:00:00Z"))).toBeCloseTo(12);
    expect(centralHour(Date.parse("2025-06-15T00:30:00Z"))).toBeCloseTo(19.5);
  });
});

describe("dayArc", () => {
  it("is fully bright at noon", () => {
    const arc = dayArc(ct("2025-06-15", "12:00"));
    expect(arc.phase).toBe("day");
    expect(arc.darkness).toBe(0);
    expect(arc.veilOpacity).toBe(0);
    expect(arc.sunIntensity).toBeGreaterThan(2);
  });

  it("is fully dark in the small hours", () => {
    const arc = dayArc(ct("2025-06-15", "02:00"));
    expect(arc.phase).toBe("night");
    expect(arc.darkness).toBe(1);
    expect(arc.veilOpacity).toBeGreaterThan(0.4);
    expect(arc.hemiIntensity).toBeLessThan(0.2);
  });

  it("warms up through dusk", () => {
    const arc = dayArc(ct("2025-06-15", "19:30"));
    expect(arc.phase).toBe("dusk");
    expect(arc.warmth).toBeGreaterThan(0);
    expect(arc.darkness).toBeGreaterThan(0.2);
    expect(arc.darkness).toBeLessThan(1);
  });

  it("ramps darkness down across dawn", () => {
    const early = dayArc(ct("2025-06-15", "05:30"));
    const late = dayArc(ct("2025-06-15", "07:00"));
    expect(early.darkness).toBeGreaterThan(late.darkness);
    expect(late.darkness).toBeGreaterThan(0);
  });

  it("blends continuously at window edges", () => {
    // No jumps: adjacent minutes never differ by more than a small step.
    for (const start of ["04:55", "07:25", "16:25", "18:55", "20:55"]) {
      const a = dayArc(ct("2025-06-15", start));
      const b = dayArc(ct("2025-06-15", start) + 60_000);
      expect(Math.abs(a.darkness - b.darkness)).toBeLessThan(0.05);
    }
  });
});
