/**
 * Scene-graph assembly for the street-level neighborhood. Everything is
 * instanced: one draw call for houses, one for SOC rings, one for poles,
 * one for the merged street strips, one for service lines, one for flow
 * particles. Telemetry reaches the GPU through a handful of dynamic
 * attributes that are rewritten only when the live-buffer version bumps,
 * never through React state.
 */

import * as THREE from "three";
import type { NeighborhoodLayout, StreetParcel } from "../procgen/placement";
import { Rng } from "../procgen/rng";
import type { LiveBuffers } from "../state/live";
import { useAppStore } from "../state/store";
import { TOKENS, socColorRgb } from "../tokens/tokens";
import {
  BADGE_ALTITUDE_M,
  HOUSE_DEPTH_M,
  HOUSE_WALL_H_M,
  POLE_ATTACH_H_M,
  POLE_SPACING_M,
  RING_ALTITUDE_M,
  RING_INNER_M,
  RING_OUTER_M,
  RING_THETA_SEGMENTS,
  WINDOW_Y_M,
  createBadgeGeometry,
  createHouseGeometry,
  createPoleGeometry,
  createStreetsGeometry,
  createWindowGeometry,
} from "./geometry";
import { createFlowMaterial, createSocRingMaterial, createWindowMaterial } from "./telemetryShaders";

/** Flow dots travelling along each home's service line. */
export const PARTICLES_PER_HOME = 6;

const GROUND_EXTENT_FACTOR = 2.6;

/**
 * SOC ramp stops sampled from the shared ramp. The ring shader outputs
 * these verbatim, so they stay as sRGB floats straight from socColorRgb.
 */
const SOC_STOP_LOW = socColorRgb(0);
const SOC_STOP_MID = socColorRgb(0.5);
const SOC_STOP_HIGH = socColorRgb(1);

const _matrix = new THREE.Matrix4();
const _position = new THREE.Vector3();
const _scale = new THREE.Vector3(1, 1, 1);
const _quaternion = new THREE.Quaternion();
const _euler = new THREE.Euler();
const _color = new THREE.Color();
const _colorBase = new THREE.Color();
const _flatQuaternion = new THREE.Quaternion().setFromEuler(new THREE.Euler(-Math.PI / 2, 0, 0));

/** Badge state colors as sRGB floats (written verbatim to instanceColor). */
const BADGE_DISCHARGE_RGB: [number, number, number] = [0xe2 / 255, 0xa6 / 255, 0x3d / 255];
const BADGE_CHARGE_RGB: [number, number, number] = [0x7f / 255, 0xae / 255, 0x6b / 255];
const BADGE_IDLE_RGB: [number, number, number] = [0x5c / 255, 0x66 / 255, 0x72 / 255];
const BADGE_RESERVE_RGB: [number, number, number] = [0xd0 / 255, 0x53 / 255, 0x3a / 255];

export interface NeighborhoodWorld {
  /** Static scenery and overlays without event handlers. */
  group: THREE.Group;
  /** Interactive house instances; raycast target for selection. */
  houseMesh: THREE.InstancedMesh;
  /** Interactive ground plane; clicking it clears the selection. */
  groundMesh: THREE.Mesh;
  center: { x: number; z: number };
  extentMeters: number;
  parcels: StreetParcel[];
  indexOfHome: Map<string, number>;
  /** Seconds uniform feeding the flow-particle shader. */
  flowUtime: { value: number };
  /** Drive the window-glow uniform from the day arc (0 day, 1 night). */
  setDarkness: (darkness: number) => void;
  /**
   * Push the latest live buffers into the dynamic GPU attributes. Runs
   * allocation-free; call only when the buffer version has changed.
   */
  writeTelemetry(live: LiveBuffers): void;
  dispose(): void;
}

export function buildNeighborhoodWorld(layout: NeighborhoodLayout): NeighborhoodWorld {
  const parcels = layout.parcels;
  const n = parcels.length;
  const disposables: Array<{ dispose(): void }> = [];
  const track = <T extends { dispose(): void }>(resource: T): T => {
    disposables.push(resource);
    return resource;
  };

  let minX = Infinity;
  let maxX = -Infinity;
  let minZ = Infinity;
  let maxZ = -Infinity;
  for (const parcel of parcels) {
    minX = Math.min(minX, parcel.x);
    maxX = Math.max(maxX, parcel.x);
    minZ = Math.min(minZ, parcel.z);
    maxZ = Math.max(maxZ, parcel.z);
  }
  const center =
    parcels.length === 0
      ? { x: 0, z: 0 }
      : { x: (minX + maxX) / 2, z: (minZ + maxZ) / 2 };

  const indexOfHome = new Map<string, number>();

  // Houses: one instanced mesh, transforms and bone-tint colors baked once.
  const houseGeometry = track(createHouseGeometry());
  const houseMaterial = track(
    new THREE.MeshStandardMaterial({ vertexColors: true, roughness: 0.95, metalness: 0 }),
  );
  const houseMesh = track(new THREE.InstancedMesh(houseGeometry, houseMaterial, n));
  houseMesh.name = "houses";
  houseMesh.frustumCulled = false;
  houseMesh.matrixAutoUpdate = false;

  // SOC rings: a shader-driven arc plus a static dim track underneath.
  const ringGeometry = track(
    new THREE.RingGeometry(RING_INNER_M, RING_OUTER_M, RING_THETA_SEGMENTS),
  );
  const socFracAttr = new THREE.InstancedBufferAttribute(new Float32Array(n), 1);
  socFracAttr.setUsage(THREE.DynamicDrawUsage);
  ringGeometry.setAttribute("socFrac", socFracAttr);

  const ringArc = track(new THREE.InstancedMesh(ringGeometry, track(createSocRingMaterial()), n));
  ringArc.name = "soc-rings";
  ringArc.frustumCulled = false;
  ringArc.matrixAutoUpdate = false;
  const ringColorAttr = new THREE.InstancedBufferAttribute(new Float32Array(n * 3), 3);
  ringColorAttr.setUsage(THREE.DynamicDrawUsage);
  ringArc.instanceColor = ringColorAttr;

  const ringTrack = track(
    new THREE.InstancedMesh(
      ringGeometry,
      track(
        new THREE.MeshBasicMaterial({
          color: TOKENS.slateLine,
          transparent: true,
          opacity: 0.45,
          depthWrite: false,
        }),
      ),
      n,
    ),
  );
  ringTrack.name = "soc-ring-tracks";
  ringTrack.frustumCulled = false;
  ringTrack.matrixAutoUpdate = false;

  // State badges: one instanced triangle per home above the SOC ring.
  // Color carries the state (sage charge, amber discharge, red reserve
  // breach, slate idle); orientation carries charge up / discharge down.
  const badgeMesh = track(
    new THREE.InstancedMesh(
      track(createBadgeGeometry()),
      track(new THREE.MeshBasicMaterial({ side: THREE.DoubleSide })),
      n,
    ),
  );
  badgeMesh.name = "state-badges";
  badgeMesh.frustumCulled = false;
  badgeMesh.matrixAutoUpdate = false;
  const badgeColorAttr = new THREE.InstancedBufferAttribute(new Float32Array(n * 3), 3);
  badgeColorAttr.setUsage(THREE.DynamicDrawUsage);
  badgeMesh.instanceColor = badgeColorAttr;

  // Window glow: one shader quad on each street-facing wall.
  const windowGeometry = track(createWindowGeometry());
  const glowAttr = new THREE.InstancedBufferAttribute(new Float32Array(n), 1);
  glowAttr.setUsage(THREE.DynamicDrawUsage);
  windowGeometry.setAttribute("glow", glowAttr);
  const windows = createWindowMaterial();
  track(windows.material);
  const windowMesh = track(new THREE.InstancedMesh(windowGeometry, windows.material, n));
  windowMesh.name = "window-glow";
  windowMesh.frustumCulled = false;
  windowMesh.matrixAutoUpdate = false;

  // Reserve floors are static per home; read them once at build time.
  const homesMeta = useAppStore.getState().homesMeta;
  const reserveFloor = new Float32Array(n);

  for (let i = 0; i < n; i++) {
    const parcel = parcels[i];
    if (!parcel) continue;
    indexOfHome.set(parcel.homeId, i);
    reserveFloor[i] = homesMeta[parcel.homeId]?.reserveFloorFrac ?? 0;

    const rng = new Rng(`house:${parcel.homeId}`);
    const footprint = rng.range(0.88, 1.12);
    const height = rng.range(0.85, 1.18);
    _euler.set(0, parcel.yaw, 0);
    _quaternion.setFromEuler(_euler);
    _position.set(parcel.x, 0, parcel.z);
    _scale.set(footprint, height, footprint);
    _matrix.compose(_position, _quaternion, _scale);
    houseMesh.setMatrixAt(i, _matrix);

    _colorBase.set(TOKENS.terrainElev).lerp(_color.set(TOKENS.textPrimary), 0.3);
    _color.copy(_colorBase);
    _color.offsetHSL(rng.range(-0.015, 0.015), rng.range(-0.04, 0.04), rng.range(-0.055, 0.045));
    houseMesh.setColorAt(i, _color);

    _position.set(parcel.x, RING_ALTITUDE_M, parcel.z);
    _scale.set(1, 1, 1);
    _matrix.compose(_position, _flatQuaternion, _scale);
    ringArc.setMatrixAt(i, _matrix);

    // Badge floats upright over the ring; direction flips per telemetry.
    _euler.set(0, parcel.yaw, 0);
    _quaternion.setFromEuler(_euler);
    _position.set(parcel.x, BADGE_ALTITUDE_M, parcel.z);
    _matrix.compose(_position, _quaternion, _scale);
    badgeMesh.setMatrixAt(i, _matrix);

    // Window quad hugs the street-facing wall, normal outward.
    const frontX = parcel.x + (HOUSE_DEPTH_M / 2 + 0.08) * Math.sin(parcel.yaw);
    const frontZ = parcel.z + (HOUSE_DEPTH_M / 2 + 0.08) * Math.cos(parcel.yaw);
    _euler.set(0, parcel.yaw, 0);
    _quaternion.setFromEuler(_euler);
    _position.set(frontX, WINDOW_Y_M, frontZ);
    _matrix.compose(_position, _quaternion, _scale);
    windowMesh.setMatrixAt(i, _matrix);
  }
  houseMesh.instanceMatrix.needsUpdate = true;
  if (houseMesh.instanceColor) houseMesh.instanceColor.needsUpdate = true;
  ringArc.instanceMatrix.needsUpdate = true;
  badgeMesh.instanceMatrix.needsUpdate = true;
  windowMesh.instanceMatrix.needsUpdate = true;

  // Track ring shares the arc's instance transforms, nudged down slightly
  // so the two never z-fight.
  ringTrack.instanceMatrix = ringArc.instanceMatrix;
  ringTrack.position.y = -0.06;
  ringTrack.updateMatrix();

  // Ground plane.
  const groundGeometry = track(
    new THREE.PlaneGeometry(layout.extentMeters * GROUND_EXTENT_FACTOR, layout.extentMeters * GROUND_EXTENT_FACTOR),
  );
  groundGeometry.rotateX(-Math.PI / 2);
  const groundMesh = new THREE.Mesh(
    groundGeometry,
    track(new THREE.MeshStandardMaterial({ color: TOKENS.groundSlab, roughness: 1, metalness: 0 })),
  );
  groundMesh.name = "ground";
  groundMesh.position.set(center.x, -0.15, center.z);
  groundMesh.updateMatrix();
  groundMesh.matrixAutoUpdate = false;

    // Streets: one merged mesh, a shade lighter than the packed ground.
    const streetsGeometry = createStreetsGeometry(layout.streets);
    let streetsMesh: THREE.Mesh | null = null;
    if (streetsGeometry) {
      track(streetsGeometry);
      const streetColor = new THREE.Color(TOKENS.streetSlab);
    streetsMesh = new THREE.Mesh(
      streetsGeometry,
      track(new THREE.MeshStandardMaterial({ color: streetColor, roughness: 1, metalness: 0 })),
    );
    streetsMesh.name = "streets";
    streetsMesh.matrixAutoUpdate = false;
  }

  // Service poles along each street row (segments with constant z).
  interface RowPoles {
    z: number;
    xs: number[];
  }
  const rowsPoles: RowPoles[] = [];
  for (const street of layout.streets) {
    if (street.az !== street.bz) continue;
    const length = street.bx - street.ax;
    const count = Math.max(1, Math.round(length / POLE_SPACING_M));
    const xs: number[] = [];
    for (let k = 0; k < count; k++) xs.push(street.ax + ((k + 0.5) / count) * length);
    rowsPoles.push({ z: street.az, xs });
  }
  if (rowsPoles.length === 0) rowsPoles.push({ z: 0, xs: [0] });

  const poleCount = rowsPoles.reduce((sum, row) => sum + row.xs.length, 0);
  const poleMesh = track(
    new THREE.InstancedMesh(
      track(createPoleGeometry()),
      track(new THREE.MeshStandardMaterial({ color: TOKENS.hairline, roughness: 0.9, metalness: 0 })),
      poleCount,
    ),
  );
  poleMesh.name = "poles";
  poleMesh.frustumCulled = false;
  poleMesh.matrixAutoUpdate = false;
  let poleIndex = 0;
  for (const row of rowsPoles) {
    for (const x of row.xs) {
      _position.set(x, 0, row.z);
      _scale.set(1, 1, 1);
      _matrix.compose(_position, _quaternion.identity(), _scale);
      poleMesh.setMatrixAt(poleIndex, _matrix);
      poleIndex++;
    }
  }
  poleMesh.instanceMatrix.needsUpdate = true;

  // Service lines plus the flow-particle system riding along them.
  const linePositions = new Float32Array(n * 6);
  const particleCount = n * PARTICLES_PER_HOME;
  const pointPosition = new Float32Array(particleCount * 3);
  const pointStart = new Float32Array(particleCount * 3);
  const pointEnd = new Float32Array(particleCount * 3);
  const pointPhase = new Float32Array(particleCount);
  const pointGridKw = new Float32Array(particleCount);
  const pointBatteryKw = new Float32Array(particleCount);

  for (let i = 0; i < n; i++) {
    const parcel = parcels[i];
    if (!parcel) continue;
    const eaveLocalZ = HOUSE_DEPTH_M / 2 + 0.35;
    const eaveX = parcel.x + eaveLocalZ * Math.sin(parcel.yaw);
    const eaveZ = parcel.z + eaveLocalZ * Math.cos(parcel.yaw);
    const eaveY = HOUSE_WALL_H_M * 0.92;

    const firstRow = rowsPoles[0] ?? { z: 0, xs: [0] };
    let row = firstRow;
    let bestDz = Math.abs(row.z - parcel.z);
    for (const candidate of rowsPoles) {
      const distance = Math.abs(candidate.z - parcel.z);
      if (distance < bestDz) {
        row = candidate;
        bestDz = distance;
      }
    }
    let poleX = row.xs[0] ?? 0;
    let bestDx = Math.abs(poleX - parcel.x);
    for (const x of row.xs) {
      const distance = Math.abs(x - parcel.x);
      if (distance < bestDx) {
        poleX = x;
        bestDx = distance;
      }
    }

    const lineOffset = i * 6;
    linePositions[lineOffset] = eaveX;
    linePositions[lineOffset + 1] = eaveY;
    linePositions[lineOffset + 2] = eaveZ;
    linePositions[lineOffset + 3] = poleX;
    linePositions[lineOffset + 4] = POLE_ATTACH_H_M;
    linePositions[lineOffset + 5] = row.z;

    for (let k = 0; k < PARTICLES_PER_HOME; k++) {
      const p = i * PARTICLES_PER_HOME + k;
      const p3 = p * 3;
      pointPosition[p3] = eaveX;
      pointPosition[p3 + 1] = eaveY;
      pointPosition[p3 + 2] = eaveZ;
      pointStart[p3] = eaveX;
      pointStart[p3 + 1] = eaveY;
      pointStart[p3 + 2] = eaveZ;
      pointEnd[p3] = poleX;
      pointEnd[p3 + 1] = POLE_ATTACH_H_M;
      pointEnd[p3 + 2] = row.z;
      pointPhase[p] = k / PARTICLES_PER_HOME;
    }
  }

  const lineGeometry = track(new THREE.BufferGeometry());
  lineGeometry.setAttribute("position", new THREE.BufferAttribute(linePositions, 3));
  const lines = new THREE.LineSegments(
    lineGeometry,
    track(
      new THREE.LineBasicMaterial({ color: TOKENS.slateLine, transparent: true, opacity: 0.5 }),
    ),
  );
  lines.name = "service-lines";

  const pointsGeometry = track(new THREE.BufferGeometry());
  pointsGeometry.setAttribute("position", new THREE.BufferAttribute(pointPosition, 3));
  pointsGeometry.setAttribute("lineStart", new THREE.BufferAttribute(pointStart, 3));
  pointsGeometry.setAttribute("lineEnd", new THREE.BufferAttribute(pointEnd, 3));
  pointsGeometry.setAttribute("phaseOffset", new THREE.BufferAttribute(pointPhase, 1));
  const gridKwAttr = new THREE.BufferAttribute(pointGridKw, 1);
  gridKwAttr.setUsage(THREE.DynamicDrawUsage);
  const batteryKwAttr = new THREE.BufferAttribute(pointBatteryKw, 1);
  batteryKwAttr.setUsage(THREE.DynamicDrawUsage);
  pointsGeometry.setAttribute("gridKw", gridKwAttr);
  pointsGeometry.setAttribute("batteryKw", batteryKwAttr);

  const flow = createFlowMaterial();
  track(flow.material);
  const points = new THREE.Points(pointsGeometry, flow.material);
  points.name = "service-flow";
  points.frustumCulled = false;

  const group = new THREE.Group();
  group.name = "neighborhood-static";
  group.add(ringTrack);
  group.add(ringArc);
  group.add(badgeMesh);
  group.add(windowMesh);
  group.add(poleMesh);
  if (streetsMesh) group.add(streetsMesh);
  group.add(lines);
  group.add(points);

  // Live-buffer slots resolve lazily so the world never depends on ingest
  // ordering; unresolved homes are skipped until their slot appears.
  const slots = new Int32Array(n).fill(-1);

  const writeTelemetry = (live: LiveBuffers): void => {
    const soc = live.soc;
    const gridKw = live.gridKw;
    const batteryKw = live.batteryKw;
    const loadKw = live.loadKw;
    const fracArray = socFracAttr.array as Float32Array;
    const colorArray = ringColorAttr.array as Float32Array;
    const gridArray = gridKwAttr.array as Float32Array;
    const batteryArray = batteryKwAttr.array as Float32Array;
    const badgeArray = badgeColorAttr.array as Float32Array;
    const glowArray = glowAttr.array as Float32Array;
    for (let i = 0; i < n; i++) {
      let slot = slots[i] ?? -1;
      if (slot < 0) {
        const parcel = parcels[i];
        const resolved = parcel === undefined ? undefined : live.slotOf.get(parcel.homeId);
        if (resolved === undefined) continue;
        slots[i] = resolved;
        slot = resolved;
      }
      const rawSoc = soc[slot] ?? 0;
      const t = rawSoc < 0 ? 0 : rawSoc > 1 ? 1 : rawSoc;
      fracArray[i] = t;
      let r: number;
      let g: number;
      let b: number;
      if (t <= 0.5) {
        const u = t * 2;
        r = SOC_STOP_LOW[0] + (SOC_STOP_MID[0] - SOC_STOP_LOW[0]) * u;
        g = SOC_STOP_LOW[1] + (SOC_STOP_MID[1] - SOC_STOP_LOW[1]) * u;
        b = SOC_STOP_LOW[2] + (SOC_STOP_MID[2] - SOC_STOP_LOW[2]) * u;
      } else {
        const u = t * 2 - 1;
        r = SOC_STOP_MID[0] + (SOC_STOP_HIGH[0] - SOC_STOP_MID[0]) * u;
        g = SOC_STOP_MID[1] + (SOC_STOP_HIGH[1] - SOC_STOP_MID[1]) * u;
        b = SOC_STOP_MID[2] + (SOC_STOP_HIGH[2] - SOC_STOP_MID[2]) * u;
      }
      const c = i * 3;
      // Match socColorRgb exactly: its ramp passes through an 8-bit hex
      // string, so interior samples quantize to 1/255 steps.
      colorArray[c] = Math.round(r * 255) / 255;
      colorArray[c + 1] = Math.round(g * 255) / 255;
      colorArray[c + 2] = Math.round(b * 255) / 255;
      const gridValue = gridKw[slot] ?? 0;
      const batteryValue = batteryKw[slot] ?? 0;
      const base = i * PARTICLES_PER_HOME;
      for (let k = 0; k < PARTICLES_PER_HOME; k++) {
        gridArray[base + k] = gridValue;
        batteryArray[base + k] = batteryValue;
      }

      // State badge: reserve breach wins, then flow direction, then idle.
      const parcel = parcels[i];
      if (!parcel) continue;
      const floor = reserveFloor[i] ?? 0;
      let badgeColor: [number, number, number];
      let roll = 0;
      let badgeScale = 1;
      if (floor > 0 && t <= floor + 0.005) {
        badgeColor = BADGE_RESERVE_RGB;
      } else if (batteryValue > 0.05) {
        badgeColor = BADGE_DISCHARGE_RGB;
        roll = Math.PI;
      } else if (batteryValue < -0.05) {
        badgeColor = BADGE_CHARGE_RGB;
      } else {
        badgeColor = BADGE_IDLE_RGB;
        badgeScale = 0.55;
      }
      badgeArray[c] = badgeColor[0];
      badgeArray[c + 1] = badgeColor[1];
      badgeArray[c + 2] = badgeColor[2];
      // Spin about the badge's own face (Z) first, then yaw to the street.
      _euler.set(0, parcel.yaw, roll, "ZYX");
      _quaternion.setFromEuler(_euler);
      _position.set(parcel.x, BADGE_ALTITUDE_M, parcel.z);
      _scale.set(badgeScale, badgeScale, badgeScale);
      _matrix.compose(_position, _quaternion, _scale);
      badgeMesh.setMatrixAt(i, _matrix);

      // Windows brighten with the home's own load.
      glowArray[i] = Math.min(1, Math.max(0, (loadKw[slot] ?? 0) / 3));
    }
    socFracAttr.needsUpdate = true;
    ringColorAttr.needsUpdate = true;
    gridKwAttr.needsUpdate = true;
    batteryKwAttr.needsUpdate = true;
    badgeColorAttr.needsUpdate = true;
    badgeMesh.instanceMatrix.needsUpdate = true;
    glowAttr.needsUpdate = true;
  };

  let disposed = false;

  return {
    group,
    houseMesh,
    groundMesh,
    center,
    extentMeters: layout.extentMeters,
    parcels,
    indexOfHome,
    flowUtime: flow.uTime,
    setDarkness: (darkness: number) => {
      windows.uDarkness.value = darkness;
    },
    writeTelemetry,
    dispose: () => {
      if (disposed) return;
      disposed = true;
      for (const resource of disposables) resource.dispose();
    },
  };
}
