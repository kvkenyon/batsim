import { describe, expect, it } from "vitest";
import {
  homeLngLat,
  layoutNeighborhood,
  lngLatOffsetMeters,
  lngLatToMercator,
  parseZones,
} from "../src/procgen/placement";

const GEOJSON = {
  type: "FeatureCollection",
  features: [
    {
      type: "Feature",
      properties: { kind: "zone", zone: "LZ_NORTH", label: "North", anchor: [-97.4, 32.9] },
      geometry: {
        type: "Polygon",
        coordinates: [[
          [-99.0, 31.6], [-96.3, 31.6], [-96.3, 33.8], [-99.0, 33.8], [-99.0, 31.6],
        ]],
      },
    },
    {
      type: "Feature",
      properties: { kind: "state", name: "Texas" },
      geometry: {
        type: "Polygon",
        coordinates: [[
          [-106.0, 26.0], [-93.0, 26.0], [-93.0, 36.5], [-106.0, 36.5], [-106.0, 26.0],
        ]],
      },
    },
  ],
};

describe("parseZones", () => {
  it("indexes zone features and skips non-zone features", () => {
    const zones = parseZones(GEOJSON);
    expect(zones.size).toBe(1);
    const north = zones.get("LZ_NORTH");
    expect(north?.anchor).toEqual([-97.4, 32.9]);
    expect(north?.bbox).toEqual([-99.0, 31.6, -96.3, 33.8]);
  });
});

describe("homeLngLat", () => {
  it("is deterministic per home id", () => {
    const zones = parseZones(GEOJSON);
    const north = zones.get("LZ_NORTH");
    if (!north) throw new Error("zone missing");
    expect(homeLngLat("home_a", north)).toEqual(homeLngLat("home_a", north));
  });

  it("places homes inside the zone polygon", () => {
    const zones = parseZones(GEOJSON);
    const north = zones.get("LZ_NORTH");
    if (!north) throw new Error("zone missing");
    for (let i = 0; i < 50; i++) {
      const [lng, lat] = homeLngLat(`home_${i}`, north);
      expect(lng).toBeGreaterThanOrEqual(north.bbox[0]);
      expect(lng).toBeLessThanOrEqual(north.bbox[2]);
      expect(lat).toBeGreaterThanOrEqual(north.bbox[1]);
      expect(lat).toBeLessThanOrEqual(north.bbox[3]);
    }
  });
});

describe("layoutNeighborhood", () => {
  const ids = Array.from({ length: 40 }, (_, i) => `home_${String(i).padStart(3, "0")}`);

  it("assigns one parcel per home, deterministically", () => {
    const a = layoutNeighborhood(ids, "seed-1");
    const b = layoutNeighborhood(ids, "seed-1");
    expect(a.parcels.length).toBe(40);
    expect(a.parcels).toEqual(b.parcels);
    const idsCovered = new Set(a.parcels.map((p) => p.homeId));
    expect(idsCovered.size).toBe(40);
  });

  it("produces a different layout for a different home set", () => {
    const a = layoutNeighborhood(ids, "seed-1");
    const b = layoutNeighborhood(ids.slice(0, 20), "seed-1");
    expect(a.parcels.length).not.toEqual(b.parcels.length);
  });

  it("parcels do not overlap", () => {
    const { parcels } = layoutNeighborhood(ids, "seed-1");
    for (let i = 0; i < parcels.length; i++) {
      for (let j = i + 1; j < parcels.length; j++) {
        const a = parcels[i];
        const b = parcels[j];
        if (!a || !b) continue;
        const dx = a.x - b.x;
        const dz = a.z - b.z;
        expect(Math.hypot(dx, dz)).toBeGreaterThan(5);
      }
    }
  });
});

describe("mercator math", () => {
  it("round-trips offset distances at texas latitudes", () => {
    // ~1 degree of latitude near 33N is about 111 km.
    const [, northMeters] = lngLatOffsetMeters(-97.4, 33.9, -97.4, 32.9);
    expect(northMeters).toBeGreaterThan(105_000);
    expect(northMeters).toBeLessThan(117_000);
  });

  it("corrects for mercator scale distortion", () => {
    // Without the 1/cos(lat) correction, east-west distances at 33N
    // would read about 19% long.
    const [rawX] = lngLatToMercator(-96.4, 32.9);
    const [anchorX] = lngLatToMercator(-97.4, 32.9);
    const [corrected] = lngLatOffsetMeters(-96.4, 32.9, -97.4, 32.9);
    const rawDelta = rawX - anchorX;
    expect(rawDelta / corrected).toBeGreaterThan(1.15);
    expect(rawDelta / corrected).toBeLessThan(1.25);
  });
});
