/**
 * Street-level stratum: one procedural neighborhood rendered with React
 * Three Fiber. The scene is fully instanced and reads telemetry straight
 * from the live buffers inside the frame loop, so React re-renders only
 * on low-frequency store changes (selection, layout identity, active).
 */

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  type ComponentRef,
  type RefObject,
} from "react";
import { Canvas, useFrame, type ThreeEvent } from "@react-three/fiber";
import { MapControls } from "@react-three/drei";
import * as THREE from "three";
import { layoutNeighborhood } from "../procgen/placement";
import { dayArc } from "../state/dayArc";
import type { LiveBuffers } from "../state/live";
import { useAppStore } from "../state/store";
import { TOKENS } from "../tokens/tokens";
import { buildNeighborhoodWorld, type NeighborhoodWorld } from "./world";

export interface NeighborhoodViewProps {
  live: LiveBuffers;
  active: boolean;
}

type MapControlsRef = ComponentRef<typeof MapControls>;

const CAMERA_ALTITUDE_M = 250;
const CAMERA_PITCH_RAD = (35 * Math.PI) / 180;
const CAMERA_AZIMUTH_RAD = (25 * Math.PI) / 180;
const CAMERA_HORIZONTAL_M = CAMERA_ALTITUDE_M / Math.tan(CAMERA_PITCH_RAD);
const CAMERA_OFFSET_X = Math.sin(CAMERA_AZIMUTH_RAD) * CAMERA_HORIZONTAL_M;
const CAMERA_OFFSET_Z = Math.cos(CAMERA_AZIMUTH_RAD) * CAMERA_HORIZONTAL_M;

const MIN_POLAR_RAD = (15 * Math.PI) / 180;
const MAX_POLAR_RAD = (60 * Math.PI) / 180;

/** Zooming out past this range hands back to the map stratum. */
const HANDOFF_DISTANCE_M = 1400;
const HANDOFF_RESET_M = 1100;
/** Ignore clicks that were really camera drags. */
const CLICK_DRAG_TOLERANCE_PX = 5;

/**
 * Advances the flow-particle clock every frame and pushes telemetry into
 * GPU attributes only when the live-buffer version has advanced.
 */
function TelemetryDriver({ live, world }: { live: LiveBuffers; world: NeighborhoodWorld }) {
  const lastVersion = useRef(-1);
  useEffect(() => {
    lastVersion.current = -1;
  }, [world]);
  useFrame((state) => {
    world.flowUtime.value = state.clock.elapsedTime;
    if (live.version === lastVersion.current) return;
    lastVersion.current = live.version;
    world.writeTelemetry(live);
  });
  return null;
}

/** Returns to the map stratum once the camera pulls far enough out. */
function StratumHandoff({ controlsRef }: { controlsRef: RefObject<MapControlsRef> }) {
  const handedOff = useRef(false);
  useFrame(({ camera }) => {
    const controls = controlsRef.current;
    if (controls === null) return;
    const distance = camera.position.distanceTo(controls.target);
    if (!handedOff.current && distance > HANDOFF_DISTANCE_M) {
      handedOff.current = true;
      useAppStore.getState().setStratum("map");
    } else if (distance < HANDOFF_RESET_M) {
      handedOff.current = false;
    }
  });
  return null;
}

/**
 * Re-frames the canonical oblique view whenever the neighborhood world
 * is rebuilt for a different zone; without this the camera keeps the
 * orbit it had over the previous zone's street.
 */
function CameraRig({
  world,
  controlsRef,
}: {
  world: NeighborhoodWorld;
  controlsRef: RefObject<MapControlsRef>;
}) {
  useEffect(() => {
    const controls = controlsRef.current;
    if (!controls) return;
    const { center } = world;
    controls.object.position.set(
      center.x + CAMERA_OFFSET_X,
      CAMERA_ALTITUDE_M,
      center.z + CAMERA_OFFSET_Z,
    );
    controls.target.set(center.x, 0, center.z);
    controls.update();
  }, [world, controlsRef]);
  return null;
}

/**
 * Binds the scene's light rig, sky, and window glow to the simulated
 * time of day. Recomputed a few times a second from the live-buffer
 * clock; at high replay speeds the whole neighborhood time-lapses.
 */
function DayArcRig({ live, world }: { live: LiveBuffers; world: NeighborhoodWorld }) {
  const hemiRef = useRef<THREE.HemisphereLight>(null);
  const sunRef = useRef<THREE.DirectionalLight>(null);
  const lastUpdate = useRef(0);
  useFrame(({ scene, clock }) => {
    if (clock.elapsedTime - lastUpdate.current < 0.25) return;
    lastUpdate.current = clock.elapsedTime;
    const arc = dayArc(live.simTimeMs > 0 ? live.simTimeMs : Date.now());
    if (hemiRef.current) hemiRef.current.intensity = arc.hemiIntensity;
    if (sunRef.current) {
      sunRef.current.intensity = arc.sunIntensity;
      sunRef.current.color.set(arc.sunColor);
    }
    if (scene.background instanceof THREE.Color) scene.background.set(arc.skyColor);
    if (scene.fog instanceof THREE.Fog) scene.fog.color.set(arc.skyColor);
    world.setDarkness(arc.darkness);
  });
  return (
    <>
      <hemisphereLight ref={hemiRef} args={[TOKENS.textPrimary, TOKENS.bgDeep, 0.9]} />
      <directionalLight ref={sunRef} color="#F2E8D5" intensity={2.2} position={[240, 320, 160]} />
    </>
  );
}

/** Ground-level amber ring marking the selected home's parcel. */
function SelectionRing({ world }: { world: NeighborhoodWorld }) {
  const selectedHomeId = useAppStore((state) => state.selectedHomeId);
  const index = selectedHomeId === null ? undefined : world.indexOfHome.get(selectedHomeId);
  const parcel = index === undefined ? undefined : world.parcels[index];
  if (!parcel) return null;
  return (
    <mesh position={[parcel.x, 0.4, parcel.z]} rotation={[-Math.PI / 2, 0, 0]}>
      <ringGeometry args={[6.4, 7.6, 40]} />
      <meshBasicMaterial
        color={TOKENS.energyDischarge}
        transparent
        opacity={0.85}
        side={THREE.DoubleSide}
        depthWrite={false}
      />
    </mesh>
  );
}

export default function NeighborhoodView({ live, active }: NeighborhoodViewProps) {
  const neighborhoodHomeIds = useAppStore((state) => state.neighborhoodHomeIds);
  const neighborhoodAnchor = useAppStore((state) => state.neighborhoodAnchor);

  const layout = useMemo(
    () =>
      layoutNeighborhood(
        neighborhoodHomeIds,
        `${neighborhoodAnchor[0]},${neighborhoodAnchor[1]}`,
      ),
    [neighborhoodHomeIds, neighborhoodAnchor],
  );
  const world = useMemo(
    () => (layout.parcels.length > 0 ? buildNeighborhoodWorld(layout) : null),
    [layout],
  );
  useEffect(() => () => world?.dispose(), [world]);
  useEffect(
    () => () => {
      document.body.style.cursor = "";
    },
    [],
  );

  const controlsRef = useRef<MapControlsRef>(null);

  const extent = layout.extentMeters;
  const fogNear = Math.max(extent * 1.4, 500);
  const fogFar = Math.max(extent * 3.2, 1600);
  const center = world ? world.center : { x: 0, z: 0 };

  const handleHouseClick = useCallback(
    (event: ThreeEvent<MouseEvent>) => {
      if (event.delta > CLICK_DRAG_TOLERANCE_PX || world === null) return;
      event.stopPropagation();
      const instanceId = event.instanceId;
      if (instanceId === undefined) return;
      const parcel = world.parcels[instanceId];
      if (parcel) useAppStore.getState().selectHome(parcel.homeId);
    },
    [world],
  );

  const handleGroundClick = useCallback((event: ThreeEvent<MouseEvent>) => {
    if (event.delta > CLICK_DRAG_TOLERANCE_PX) return;
    useAppStore.getState().selectHome(null);
  }, []);

  const handleHouseHover = useCallback((event: ThreeEvent<PointerEvent>) => {
    event.stopPropagation();
    document.body.style.cursor = "pointer";
  }, []);

  const handleHouseOut = useCallback(() => {
    document.body.style.cursor = "";
  }, []);

  return (
    <Canvas
      frameloop={active ? "always" : "never"}
      dpr={[1, 2]}
      camera={{
        fov: 30,
        near: 5,
        far: fogFar * 2,
        position: [center.x + CAMERA_OFFSET_X, CAMERA_ALTITUDE_M, center.z + CAMERA_OFFSET_Z],
      }}
    >
      <color attach="background" args={[TOKENS.bgBase]} />
      <fog attach="fog" args={[TOKENS.bgDeep, fogNear, fogFar]} />
      {world !== null ? (
        <>
          <DayArcRig live={live} world={world} />
          <primitive object={world.groundMesh} onClick={handleGroundClick} />
          <primitive
            object={world.houseMesh}
            onClick={handleHouseClick}
            onPointerMove={handleHouseHover}
            onPointerOut={handleHouseOut}
          />
          <primitive object={world.group} />
          <TelemetryDriver world={world} live={live} />
          <SelectionRing world={world} />
        </>
      ) : (
        <mesh position={[0, 5, 0]}>
          <boxGeometry args={[10, 10, 10]} />
          <meshStandardMaterial color={TOKENS.surfaceRaised} wireframe />
        </mesh>
      )}
      <MapControls
        ref={controlsRef}
        makeDefault
        target={[center.x, 0, center.z]}
        enableDamping
        dampingFactor={0.08}
        minDistance={30}
        maxDistance={1700}
        minPolarAngle={MIN_POLAR_RAD}
        maxPolarAngle={MAX_POLAR_RAD}
      />
      {world !== null && <CameraRig world={world} controlsRef={controlsRef} />}
      <StratumHandoff controlsRef={controlsRef} />
    </Canvas>
  );
}
