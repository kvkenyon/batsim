/**
 * Procedural world placement. Homes arrive from the API with an ERCOT load
 * zone but no coordinates, so the UI assigns each home a deterministic
 * position derived from its id: identical fleet, identical map, every run.
 */

import { Rng } from "./rng";

export interface ZoneFeature {
  zone: string;
  label: string;
  anchor: [number, number];
  polygon: [number, number][];
  bbox: [number, number, number, number];
}

interface RawZoneProperties {
  kind: string;
  zone?: string;
  label?: string;
  anchor?: [number, number];
}

/** Parse the zones GeoJSON into an index keyed by ERCOT load zone id. */
export function parseZones(geojson: unknown): Map<string, ZoneFeature> {
  const out = new Map<string, ZoneFeature>();
  const fc = geojson as { features?: Array<{ properties: RawZoneProperties; geometry: { type: string; coordinates: unknown } }> };
  for (const f of fc.features ?? []) {
    if (f.properties.kind !== "zone" || !f.properties.zone) continue;
    if (f.geometry.type !== "Polygon") continue;
    const rings = f.geometry.coordinates as [number, number][][];
    const ring = rings[0];
    if (!ring || ring.length < 4) continue;
    let minX = Infinity, minY = Infinity, maxX = -Infinity, maxY = -Infinity;
    for (const [x, y] of ring) {
      if (x < minX) minX = x;
      if (y < minY) minY = y;
      if (x > maxX) maxX = x;
      if (y > maxY) maxY = y;
    }
    const bbox: [number, number, number, number] = [minX, minY, maxX, maxY];
    const anchor = f.properties.anchor ?? [(minX + maxX) / 2, (minY + maxY) / 2];
    out.set(f.properties.zone, {
      zone: f.properties.zone,
      label: f.properties.label ?? f.properties.zone,
      anchor: pointInPolygon(anchor[0], anchor[1], ring) ? anchor : interiorPoint(ring, bbox),
      polygon: ring,
      bbox,
    });
  }
  return out;
}

/**
 * Deterministic interior point for a ring: the bbox center when it lies
 * inside, otherwise the first hit of a coarse grid scan. Zone anchors in
 * the bundled GeoJSON are hand-drawn and can drift outside their polygon,
 * which would stack every home of the zone on one out-of-zone marker.
 */
function interiorPoint(ring: [number, number][], bbox: [number, number, number, number]): [number, number] {
  const [minX, minY, maxX, maxY] = bbox;
  const cx = (minX + maxX) / 2;
  const cy = (minY + maxY) / 2;
  if (pointInPolygon(cx, cy, ring)) return [cx, cy];
  const steps = 16;
  for (let i = 1; i < steps; i++) {
    for (let j = 1; j < steps; j++) {
      const x = minX + ((maxX - minX) * i) / steps;
      const y = minY + ((maxY - minY) * j) / steps;
      if (pointInPolygon(x, y, ring)) return [x, y];
    }
  }
  return [cx, cy];
}

function pointInPolygon(x: number, y: number, ring: [number, number][]): boolean {
  let inside = false;
  for (let i = 0, j = ring.length - 1; i < ring.length; j = i++) {
    const pi = ring[i] ?? [0, 0];
    const pj = ring[j] ?? [0, 0];
    const intersects =
      pi[1] > y !== pj[1] > y && x < ((pj[0] - pi[0]) * (y - pi[1])) / (pj[1] - pi[1]) + pi[0];
    if (intersects) inside = !inside;
  }
  return inside;
}

/**
 * Deterministic scatter of one home inside its zone polygon. Homes in a
 * zone cluster near the zone anchor so fleets read as subdivisions, not
 * uniform noise: position = anchor + seeded jitter, folded back into the
 * polygon when a draw lands outside.
 */
export function homeLngLat(homeId: string, zone: ZoneFeature): [number, number] {
  const rng = new Rng(`place:${homeId}`);
  const spreadLng = Math.min((zone.bbox[2] - zone.bbox[0]) * 0.28, 0.9);
  const spreadLat = Math.min((zone.bbox[3] - zone.bbox[1]) * 0.28, 0.9);
  for (let attempt = 0; attempt < 24; attempt++) {
    const x = zone.anchor[0] + rng.range(-spreadLng, spreadLng);
    const y = zone.anchor[1] + rng.range(-spreadLat, spreadLat);
    if (pointInPolygon(x, y, zone.polygon)) return [x, y];
  }
  return zone.anchor;
}

export interface StreetParcel {
  homeId: string;
  /** Local east/north meters relative to the neighborhood anchor. */
  x: number;
  z: number;
  /** House yaw in radians (faces its street). */
  yaw: number;
}

export interface NeighborhoodLayout {
  parcels: StreetParcel[];
  /** Street centerline segments in local meters, for road rendering. */
  streets: Array<{ ax: number; az: number; bx: number; bz: number }>;
  /** Overall extents, for camera framing. */
  extentMeters: number;
}

const LOT_WIDTH_M = 22;
const LOT_DEPTH_M = 34;
const STREET_GAP_M = 14;
const LOTS_PER_ROW = 14;

/**
 * Lay homes out on a seeded street grid: one main east-west avenue with
 * rows of lots on either side, a side street every two rows. Lot order
 * follows sorted home ids, so a given fleet always produces the same
 * street. Deterministic per (seed, home set).
 */
export function layoutNeighborhood(homeIds: string[], seed: string): NeighborhoodLayout {
  const sorted = [...homeIds].sort();
  const rng = new Rng(`neighborhood:${seed}`);
  const parcels: StreetParcel[] = [];
  const streets: Array<{ ax: number; az: number; bx: number; bz: number }> = [];

  const rowPitch = LOT_DEPTH_M * 2 + STREET_GAP_M;
  const rowCount = Math.max(1, Math.ceil(sorted.length / (LOTS_PER_ROW * 2)));
  const half = (LOTS_PER_ROW * LOT_WIDTH_M) / 2;

  for (let row = 0; row < rowCount; row++) {
    const streetZ = row * rowPitch;
    streets.push({ ax: -half - 10, az: streetZ, bx: half + 10, bz: streetZ });
  }
  const sideStreetX = half + 10 + STREET_GAP_M;
  const lastAvenueZ = (rowCount - 1) * rowPitch;
  for (const x of [-sideStreetX, sideStreetX]) {
    streets.push({ ax: x, az: -STREET_GAP_M, bx: x, bz: lastAvenueZ });
  }

  for (let i = 0; i < sorted.length; i++) {
    const homeId = sorted[i] ?? "";
    const row = Math.floor(i / (LOTS_PER_ROW * 2));
    const inRow = i % (LOTS_PER_ROW * 2);
    const northSide = inRow >= LOTS_PER_ROW;
    const lot = inRow % LOTS_PER_ROW;
    const streetZ = row * rowPitch;
    const x = (lot - (LOTS_PER_ROW - 1) / 2) * LOT_WIDTH_M + rng.range(-2.5, 2.5);
    const side = northSide ? 1 : -1;
    const z = streetZ + side * (STREET_GAP_M / 2 + LOT_DEPTH_M / 2) + rng.range(-2, 2);
    parcels.push({ homeId, x, z, yaw: northSide ? Math.PI : 0 });
  }

  const extentMeters = Math.max(LOTS_PER_ROW * LOT_WIDTH_M + 40, rowCount * rowPitch + 60);
  return { parcels, streets, extentMeters };
}

const EARTH_RADIUS_M = 6378137;

/** WGS84 to web mercator meters. */
export function lngLatToMercator(lng: number, lat: number): [number, number] {
  const lam = (lng * Math.PI) / 180;
  const phi = (lat * Math.PI) / 180;
  return [EARTH_RADIUS_M * lam, EARTH_RADIUS_M * Math.log(Math.tan(Math.PI / 4 + phi / 2))];
}

/**
 * Local tangent-plane offset in true meters between two lng/lat points,
 * correcting for mercator scale distortion at the anchor latitude.
 */
export function lngLatOffsetMeters(
  lng: number,
  lat: number,
  anchorLng: number,
  anchorLat: number,
): [number, number] {
  const [mx, my] = lngLatToMercator(lng, lat);
  const [ax, ay] = lngLatToMercator(anchorLng, anchorLat);
  const k = 1 / Math.cos((anchorLat * Math.PI) / 180);
  return [(mx - ax) / k, (my - ay) / k];
}
