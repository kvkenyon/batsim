import { describe, expect, it } from "vitest";
import { hashString, Rng } from "../src/procgen/rng";

describe("seeded rng", () => {
  it("produces identical streams for identical seeds", () => {
    const a = new Rng("fleet:demo");
    const b = new Rng("fleet:demo");
    const seqA = Array.from({ length: 32 }, () => a.next());
    const seqB = Array.from({ length: 32 }, () => b.next());
    expect(seqA).toEqual(seqB);
  });

  it("produces different streams for different seeds", () => {
    const a = new Rng("fleet:a");
    const b = new Rng("fleet:b");
    expect(a.next()).not.toEqual(b.next());
  });

  it("stays inside [0, 1) over long runs", () => {
    const rng = new Rng("bounds");
    for (let i = 0; i < 10000; i++) {
      const v = rng.next();
      expect(v).toBeGreaterThanOrEqual(0);
      expect(v).toBeLessThan(1);
    }
  });

  it("hash is stable and seed-sensitive", () => {
    expect(hashString("home_1")).toEqual(hashString("home_1"));
    expect(hashString("home_1")).not.toEqual(hashString("home_2"));
  });

  it("integer draws cover the range", () => {
    const rng = new Rng("ints");
    const seen = new Set<number>();
    for (let i = 0; i < 500; i++) seen.add(rng.int(7));
    expect(seen.size).toBe(7);
  });
});
