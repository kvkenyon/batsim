/**
 * Static geometry builders for the street-level neighborhood. Everything
 * here is built once per layout and shared across instances; per-frame
 * work never touches these. Coordinates are local meters: x east, z
 * south, y up.
 */

import * as THREE from "three";
import { mergeGeometries } from "three/examples/jsm/utils/BufferGeometryUtils.js";

export const HOUSE_WIDTH_M = 9;
export const HOUSE_DEPTH_M = 11;
export const HOUSE_WALL_H_M = 5;
export const HOUSE_ROOF_H_M = 2.6;

/** Ring hovers just above the roof peak of a nominal house. */
export const RING_ALTITUDE_M = HOUSE_WALL_H_M + HOUSE_ROOF_H_M + 1.8;
export const RING_INNER_M = 3.2;
export const RING_OUTER_M = 5.0;
export const RING_THETA_SEGMENTS = 24;

export const STREET_WIDTH_M = 10;

export const POLE_HEIGHT_M = 7.2;
export const POLE_ATTACH_H_M = 6.6;
/** Roughly one service pole every four lots along a street row. */
export const POLE_SPACING_M = 88;

/** Roof faces bake a slightly darker warm tint via vertex color. */
const ROOF_TINT: [number, number, number] = [0.74, 0.68, 0.6];

function paintSolidColor(geometry: THREE.BufferGeometry, rgb: [number, number, number]): void {
  const count = geometry.getAttribute("position").count;
  const colors = new Float32Array(count * 3);
  for (let i = 0; i < count; i++) {
    colors[i * 3] = rgb[0];
    colors[i * 3 + 1] = rgb[1];
    colors[i * 3 + 2] = rgb[2];
  }
  geometry.setAttribute("color", new THREE.BufferAttribute(colors, 3));
}

/**
 * House massing: one box (walls) plus a triangular prism (roof) merged
 * into a single non-indexed geometry, about 20 triangles. Vertex colors
 * carry the wall/roof split; per-instance color multiplies on top.
 */
export function createHouseGeometry(): THREE.BufferGeometry {
  const box = new THREE.BoxGeometry(HOUSE_WIDTH_M, HOUSE_WALL_H_M, HOUSE_DEPTH_M).toNonIndexed();
  box.translate(0, HOUSE_WALL_H_M / 2, 0);
  paintSolidColor(box, [1, 1, 1]);

  const hw = HOUSE_WIDTH_M / 2;
  const hd = HOUSE_DEPTH_M / 2;
  const h = HOUSE_WALL_H_M;
  const peak = HOUSE_WALL_H_M + HOUSE_ROOF_H_M;

  const a = [-hw, h, -hd];
  const b = [-hw, h, hd];
  const c = [0, peak, hd];
  const d = [0, peak, -hd];
  const e = [hw, h, -hd];
  const f = [hw, h, hd];

  // Wound counter-clockwise seen from outside: two slopes plus two end caps.
  const corners = [a, b, c, a, c, d, e, c, f, e, d, c, b, f, c, a, d, e];
  const vertexCount = corners.length;
  const positions = new Float32Array(vertexCount * 3);
  for (let i = 0; i < vertexCount; i++) {
    const corner = corners[i] ?? [0, 0, 0];
    positions[i * 3] = corner[0] ?? 0;
    positions[i * 3 + 1] = corner[1] ?? 0;
    positions[i * 3 + 2] = corner[2] ?? 0;
  }

  const prism = new THREE.BufferGeometry();
  prism.setAttribute("position", new THREE.BufferAttribute(positions, 3));
  prism.setAttribute("normal", new THREE.BufferAttribute(new Float32Array(vertexCount * 3), 3));
  prism.setAttribute("uv", new THREE.BufferAttribute(new Float32Array(vertexCount * 2), 2));
  paintSolidColor(prism, ROOF_TINT);

  const merged = mergeGeometries([box, prism], false);
  merged.computeVertexNormals();
  return merged;
}

/** Service pole: a small tapered cylinder with its base at y = 0. */
export function createPoleGeometry(): THREE.CylinderGeometry {
  const geometry = new THREE.CylinderGeometry(0.14, 0.2, POLE_HEIGHT_M, 8);
  geometry.translate(0, POLE_HEIGHT_M / 2, 0);
  return geometry;
}

export const BADGE_ALTITUDE_M = RING_ALTITUDE_M + 2.8;
export const BADGE_SIZE_M = 1.1;

/**
 * Per-home state badge: a flat triangle (charge = up, discharge = down,
 * idle = diamond, reserve breach = red triangle). Flat in local XY; the
 * instance transform billboards it upright and rotates for direction.
 */
export function createBadgeGeometry(): THREE.BufferGeometry {
  const s = BADGE_SIZE_M;
  const positions = new Float32Array([0, s, 0, -s * 0.87, -s * 0.5, 0, s * 0.87, -s * 0.5, 0]);
  const geometry = new THREE.BufferGeometry();
  geometry.setAttribute("position", new THREE.BufferAttribute(positions, 3));
  geometry.setAttribute("normal", new THREE.BufferAttribute(new Float32Array([0, 0, 1, 0, 0, 1, 0, 0, 1]), 3));
  return geometry;
}

/** Window-glow quad on the street-facing wall of one house. */
export const WINDOW_WIDTH_M = HOUSE_WIDTH_M * 0.72;
export const WINDOW_HEIGHT_M = HOUSE_WALL_H_M * 0.5;
export const WINDOW_Y_M = HOUSE_WALL_H_M * 0.45;

export function createWindowGeometry(): THREE.PlaneGeometry {
  return new THREE.PlaneGeometry(WINDOW_WIDTH_M, WINDOW_HEIGHT_M);
}

export interface StreetSegment {
  ax: number;
  az: number;
  bx: number;
  bz: number;
}

/**
 * Merge every street centerline into one geometry of flat strips so the
 * whole road network renders in a single draw call.
 */
export function createStreetsGeometry(
  streets: StreetSegment[],
  width: number = STREET_WIDTH_M,
): THREE.BufferGeometry | null {
  const parts: THREE.BufferGeometry[] = [];
  for (const street of streets) {
    const dx = street.bx - street.ax;
    const dz = street.bz - street.az;
    const length = Math.hypot(dx, dz);
    if (length < 0.01) continue;
    const strip = new THREE.PlaneGeometry(length, width);
    strip.rotateX(-Math.PI / 2);
    strip.rotateY(Math.atan2(-dz, dx));
    strip.translate((street.ax + street.bx) / 2, 0, (street.az + street.bz) / 2);
    parts.push(strip);
  }
  if (parts.length === 0) return null;
  return mergeGeometries(parts, false);
}
