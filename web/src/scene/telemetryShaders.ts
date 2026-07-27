/**
 * Telemetry-bound shader materials for the neighborhood overlays. These
 * accents are unlit and write token sRGB values verbatim so the ring and
 * particle colors match the HUD palette exactly; lit surfaces (houses,
 * ground) go through the standard tone-mapped pipeline instead.
 */

import * as THREE from "three";
import { TOKENS } from "../tokens/tokens";

/**
 * Per-instance SOC gauge. Geometry is a flat ring in local XY (instanced
 * rotation lays it flat above each house). The arc fills clockwise from
 * the top by the socFrac attribute; fragments past the fill are discarded
 * so the dim track ring underneath stays visible.
 */
const SOC_RING_VERTEX = /* glsl */ `
attribute float socFrac;

varying float vSocFrac;
varying vec3 vColor;
varying vec2 vLocal;

void main() {
  vSocFrac = socFrac;
  vLocal = position.xy;
  #ifdef USE_INSTANCING_COLOR
    vColor = instanceColor;
  #else
    vColor = vec3(1.0, 0.85, 0.55);
  #endif
  vec4 localPosition = vec4(position, 1.0);
  #ifdef USE_INSTANCING
    localPosition = instanceMatrix * localPosition;
  #endif
  gl_Position = projectionMatrix * modelViewMatrix * localPosition;
}
`;

const SOC_RING_FRAGMENT = /* glsl */ `
varying float vSocFrac;
varying vec3 vColor;
varying vec2 vLocal;

const float PI = 3.141592653589793;
const float TAU = 6.283185307179586;

void main() {
  float angle = atan(vLocal.y, vLocal.x);
  float filled = mod(PI * 0.5 - angle, TAU) / TAU;
  if (filled > vSocFrac) discard;
  gl_FragColor = vec4(vColor, 1.0);
}
`;

/**
 * Service-line flow dots. Each particle owns a position on one home's
 * service line; direction and color derive from the home's gridKw and
 * batteryKw attributes, speed scales with |gridKw|. Exporting flows from
 * house to pole, importing from pole to house.
 */
const FLOW_VERTEX = /* glsl */ `
attribute vec3 lineStart;
attribute vec3 lineEnd;
attribute float phaseOffset;
attribute float gridKw;
attribute float batteryKw;

uniform float uTime;
uniform float uSizePx;
uniform vec3 uExportColor;
uniform vec3 uChargeColor;
uniform vec3 uIdleColor;

varying vec3 vColor;

void main() {
  vec3 color = uIdleColor;
  float dir = 1.0;
  if (gridKw < -0.05) {
    color = uExportColor;
  } else if (gridKw > 0.05) {
    dir = -1.0;
    if (batteryKw < -0.05) color = uChargeColor;
  }
  vColor = color;

  float speed = clamp(abs(gridKw) / 5.0, 0.2, 3.0) * 1.5;
  float len = max(distance(lineStart, lineEnd), 0.001);
  float travel = mod(phaseOffset * len + uTime * speed * dir, len);
  vec3 pos = mix(lineStart, lineEnd, travel / len);
  vec4 mvPosition = modelViewMatrix * vec4(pos, 1.0);
  gl_PointSize = clamp(uSizePx * (160.0 / -mvPosition.z), 1.5, 9.0);
  gl_Position = projectionMatrix * mvPosition;
}
`;

const FLOW_FRAGMENT = /* glsl */ `
varying vec3 vColor;

void main() {
  float radius = length(gl_PointCoord - vec2(0.5));
  if (radius > 0.5) discard;
  float alpha = smoothstep(0.5, 0.3, radius) * 0.9;
  gl_FragColor = vec4(vColor, alpha);
}
`;

export function createSocRingMaterial(): THREE.ShaderMaterial {
  return new THREE.ShaderMaterial({
    vertexShader: SOC_RING_VERTEX,
    fragmentShader: SOC_RING_FRAGMENT,
  });
}

/**
 * Window glow: one quad per house on the street-facing wall. A per-home
 * load attribute gates how many window panes are lit; a global darkness
 * uniform fades the whole effect in at dusk and out at dawn, so the
 * street lights up exactly when the simulated sun goes down.
 */
const WINDOW_VERTEX = /* glsl */ `
attribute float glow;

varying float vGlow;
varying vec2 vUv;

void main() {
  vGlow = glow;
  vUv = uv;
  vec4 localPosition = vec4(position, 1.0);
  #ifdef USE_INSTANCING
    localPosition = instanceMatrix * localPosition;
  #endif
  gl_Position = projectionMatrix * modelViewMatrix * localPosition;
}
`;

const WINDOW_FRAGMENT = /* glsl */ `
uniform float uDarkness;
uniform vec3 uColor;

varying float vGlow;
varying vec2 vUv;

void main() {
  // Two panes per quad; pane brightness jitters with the home's load.
  float pane = step(0.08, vUv.x) * step(vUv.x, 0.44) + step(0.56, vUv.x) * step(vUv.x, 0.92);
  float vertical = step(0.15, vUv.y) * step(vUv.y, 0.85);
  float mask = pane * vertical;
  if (mask < 0.5) discard;
  float lit = uDarkness * clamp(vGlow, 0.0, 1.0);
  if (lit < 0.03) discard;
  gl_FragColor = vec4(uColor, lit * 0.85);
}
`;

export interface WindowMaterialHandle {
  material: THREE.ShaderMaterial;
  /** 0 = day (windows dark), 1 = night (windows fully lit by load). */
  uDarkness: { value: number };
}

export function createWindowMaterial(): WindowMaterialHandle {
  const uDarkness = { value: 0 };
  const material = new THREE.ShaderMaterial({
    vertexShader: WINDOW_VERTEX,
    fragmentShader: WINDOW_FRAGMENT,
    uniforms: {
      uDarkness,
      uColor: { value: new THREE.Vector3(...hexToSrgbFloats("#E8C07A")) },
    },
    transparent: true,
    depthWrite: false,
    side: THREE.DoubleSide,
  });
  return { material, uDarkness };
}

export interface FlowMaterialHandle {
  material: THREE.ShaderMaterial;
  /** Seconds; advanced every rendered frame so dots animate smoothly. */
  uTime: { value: number };
}

/** Mirrors the token hex decoding so overlays use identical sRGB floats. */
function hexToSrgbFloats(hex: string): [number, number, number] {
  const n = parseInt(hex.slice(1), 16);
  return [((n >> 16) & 0xff) / 255, ((n >> 8) & 0xff) / 255, (n & 0xff) / 255];
}

export function createFlowMaterial(): FlowMaterialHandle {
  const uTime = { value: 0 };
  const material = new THREE.ShaderMaterial({
    vertexShader: FLOW_VERTEX,
    fragmentShader: FLOW_FRAGMENT,
    uniforms: {
      uTime,
      uSizePx: { value: 7.0 },
      uExportColor: { value: new THREE.Vector3(...hexToSrgbFloats(TOKENS.energyExport)) },
      uChargeColor: { value: new THREE.Vector3(...hexToSrgbFloats(TOKENS.energyCharge)) },
      uIdleColor: { value: new THREE.Vector3(...hexToSrgbFloats(TOKENS.slateLine)) },
    },
    transparent: true,
    depthWrite: false,
  });
  return { material, uTime };
}
