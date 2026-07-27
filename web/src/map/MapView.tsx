/**
 * Map stratum: the whole-fleet view. A tile-less MapLibre map renders the
 * Texas state polygon and load-zone boundaries from bundled GeoJSON, plus
 * one circle marker per home. Marker color follows the active lens; live
 * telemetry is read imperatively from the live buffers at a low cadence and
 * pushed into the GeoJSON source, never through React state.
 */

import type { Feature, FeatureCollection, MultiPolygon, Point, Polygon } from "geojson";
import maplibregl, {
  type CircleLayerSpecification,
  type FillLayerSpecification,
  type FilterSpecification,
  type LineLayerSpecification,
  type LngLatBoundsLike,
  type Map as MaplibreMap,
  type StyleSpecification,
  type SymbolLayerSpecification,
} from "maplibre-gl";
import "maplibre-gl/dist/maplibre-gl.css";
import { useEffect, useRef } from "react";

import type { LiveBuffers } from "../state/live";
import { useAppStore, type Lens } from "../state/store";
import { TOKENS, priceColor, socColor } from "../tokens/tokens";

export interface MapViewProps {
  live: LiveBuffers;
  active: boolean;
}

const ZONES_URL = "geo/texas-zones.json";
/** Glyph PBFs for the zone-label symbol layer, vendored for offline use. */
const GLYPHS_URL = "fonts/{fontstack}/{range}.pbf";
const LABEL_FONT = ["open-sans-semibold"];

const TEXAS_BOUNDS: LngLatBoundsLike = [
  [-106.7, 25.9],
  [-93.5, 36.6],
];
const PAN_MARGIN_DEG = 5;
const PAN_BOUNDS: LngLatBoundsLike = [
  [-106.7 - PAN_MARGIN_DEG, 25.9 - PAN_MARGIN_DEG],
  [-93.5 + PAN_MARGIN_DEG, 36.6 + PAN_MARGIN_DEG],
];
/** Zoom at or above which the map hands off to the street-level stratum. */
const NEIGHBORHOOD_ZOOM = 13.4;
/** Center proximity to the neighborhood anchor required for the handoff. */
const NEIGHBORHOOD_RADIUS_DEG = 0.6;
/** Telemetry push cadence for marker and zone tints. */
const TELEMETRY_PUSH_MS = 500;

const SOURCE_STATE = "texas-state";
const SOURCE_ZONES = "texas-zones";
const SOURCE_ZONE_LABELS = "texas-zone-labels";
const SOURCE_HOMES = "homes";

const LAYER_STATE_FILL = "state-fill";
const LAYER_STATE_OUTLINE = "state-outline";
const LAYER_ZONES_FILL = "zones-fill";
const LAYER_ZONES_OUTLINE = "zones-outline";
const LAYER_ZONES_LABEL = "zones-label";
const LAYER_HOMES = "homes";
const LAYER_HOMES_SELECTED = "homes-selected";

type ZoneGeometry = Polygon | MultiPolygon;

const EMPTY_FC: FeatureCollection = { type: "FeatureCollection", features: [] };

/** One point feature per home, colored for the given lens. */
function buildHomesCollection(live: LiveBuffers, lens: Lens): FeatureCollection<Point> {
  const priceHex = priceColor(live.priceRtm);
  const features: Feature<Point>[] = [];
  for (let i = 0; i < live.count; i++) {
    const id = live.homeIds[i];
    const lng = live.lng[i];
    const lat = live.lat[i];
    if (id === undefined || lng === undefined || lat === undefined) continue;
    features.push({
      type: "Feature",
      properties: { id, color: lens === "soc" ? socColor(live.soc[i] ?? 0) : priceHex },
      geometry: { type: "Point", coordinates: [lng, lat] },
    });
  }
  return { type: "FeatureCollection", features };
}

/** Split the zones bundle into state outline, zone polygons, and label points. */
function splitZones(fc: FeatureCollection): {
  state: FeatureCollection<ZoneGeometry>;
  zones: FeatureCollection<ZoneGeometry>;
  labels: FeatureCollection<Point>;
} {
  const state: Feature<ZoneGeometry>[] = [];
  const zones: Feature<ZoneGeometry>[] = [];
  const labels: Feature<Point>[] = [];
  for (const feature of fc.features) {
    const props = feature.properties ?? {};
    const geometry = feature.geometry;
    if (geometry.type !== "Polygon" && geometry.type !== "MultiPolygon") continue;
    const polygonFeature = feature as Feature<ZoneGeometry>;
    if (props.kind === "state") {
      state.push(polygonFeature);
    } else if (props.kind === "zone") {
      zones.push(polygonFeature);
      const anchor: unknown = props.anchor;
      const label: unknown = props.label;
      const zone: unknown = props.zone;
      if (
        Array.isArray(anchor) &&
        typeof anchor[0] === "number" &&
        typeof anchor[1] === "number" &&
        typeof label === "string"
      ) {
        labels.push({
          type: "Feature",
          properties: { label, zone: typeof zone === "string" ? zone : "" },
          geometry: { type: "Point", coordinates: [anchor[0], anchor[1]] },
        });
      }
    }
  }
  return {
    state: { type: "FeatureCollection", features: state },
    zones: { type: "FeatureCollection", features: zones },
    labels: { type: "FeatureCollection", features: labels },
  };
}

export default function MapView({ live, active }: MapViewProps) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const mapRef = useRef<MaplibreMap | null>(null);
  const activeRef = useRef(active);
  activeRef.current = active;

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    let cancelled = false;
    const abort = new AbortController();
    const lensRef = { current: useAppStore.getState().lens };
    const lastVersionRef = { current: -1 };
    const lastCountRef = { current: -1 };
    let layersReady = false;

    const style: StyleSpecification = {
      version: 8,
      name: "batsim-offline",
      glyphs: GLYPHS_URL,
      sources: {},
      layers: [
        {
          id: "background",
          type: "background",
          paint: { "background-color": TOKENS.bgBase },
        },
      ],
    };
    const map: MaplibreMap = new maplibregl.Map({
      container,
      style,
      bounds: TEXAS_BOUNDS,
      fitBoundsOptions: { padding: 32 },
      maxBounds: PAN_BOUNDS,
      minZoom: 4,
      maxZoom: 15.5,
      attributionControl: false,
      cooperativeGestures: false,
    });
    mapRef.current = map;
    // Ops/e2e handle for driving the map from the console.
    (window as unknown as { __batsimMap?: MaplibreMap }).__batsimMap = map;
    map.addControl(new maplibregl.NavigationControl({ showCompass: false }), "top-right");

    /** Zone tint for the active lens: price-colored under price, neutral under soc. */
    const applyZoneTint = (lens: Lens) => {
      if (!map.getLayer(LAYER_ZONES_FILL)) return;
      if (lens === "price") {
        map.setPaintProperty(LAYER_ZONES_FILL, "fill-color", priceColor(live.priceRtm));
        map.setPaintProperty(LAYER_ZONES_FILL, "fill-opacity", 0.35);
      } else {
        map.setPaintProperty(LAYER_ZONES_FILL, "fill-color", TOKENS.terrainBase);
        map.setPaintProperty(LAYER_ZONES_FILL, "fill-opacity", 0.12);
      }
    };

    /** Push the latest committed telemetry frame into the map. */
    const pushTelemetry = (force: boolean) => {
      if (!layersReady) return;
      const versionChanged = live.version !== lastVersionRef.current;
      const countChanged = live.count !== lastCountRef.current;
      if (!force && !versionChanged && !countChanged) return;
      lastVersionRef.current = live.version;
      lastCountRef.current = live.count;
      if (lensRef.current === "soc" || countChanged) {
        (map.getSource(SOURCE_HOMES) as maplibregl.GeoJSONSource | undefined)?.setData(
          buildHomesCollection(live, lensRef.current),
        );
      }
      if (lensRef.current === "price") {
        map.setPaintProperty(LAYER_HOMES, "circle-color", priceColor(live.priceRtm));
      }
      applyZoneTint(lensRef.current);
    };

    const wireInteractions = () => {
      map.on("click", LAYER_HOMES, (e) => {
        e.preventDefault();
        const id: unknown = e.features?.[0]?.properties?.id;
        useAppStore.getState().selectHome(typeof id === "string" ? id : null);
      });
      map.on("click", (e) => {
        if (!e.defaultPrevented) useAppStore.getState().selectHome(null);
      });
      map.on("mouseenter", LAYER_HOMES, () => {
        map.getCanvas().style.cursor = "pointer";
      });
      map.on("mouseleave", LAYER_HOMES, () => {
        map.getCanvas().style.cursor = "";
      });
      // Handoff is armed while zoomed out below the threshold and consumed
      // when it fires; re-entering the map stratum at a high zoom does not
      // immediately bounce back to the street level.
      let handoffArmed = map.getZoom() < NEIGHBORHOOD_ZOOM;
      map.on("moveend", () => {
        if (map.getZoom() < NEIGHBORHOOD_ZOOM) {
          handoffArmed = true;
          return;
        }
        if (!handoffArmed || !activeRef.current) return;
        const state = useAppStore.getState();
        if (state.stratum !== "map") return;
        const center = map.getCenter();
        const [anchorLng, anchorLat] = state.neighborhoodAnchor;
        if (
          Math.abs(center.lng - anchorLng) <= NEIGHBORHOOD_RADIUS_DEG &&
          Math.abs(center.lat - anchorLat) <= NEIGHBORHOOD_RADIUS_DEG
        ) {
          handoffArmed = false;
          state.setStratum("neighborhood");
        }
      });
    };

    const addLayers = (zonesFc: FeatureCollection) => {
      const { state, zones, labels } = splitZones(zonesFc);
      const lens = lensRef.current;

      map.addSource(SOURCE_STATE, { type: "geojson", data: state });
      map.addSource(SOURCE_ZONES, { type: "geojson", data: zones });
      map.addSource(SOURCE_ZONE_LABELS, { type: "geojson", data: labels });
      map.addSource(SOURCE_HOMES, { type: "geojson", data: buildHomesCollection(live, lens) });

      const stateFill: FillLayerSpecification = {
        id: LAYER_STATE_FILL,
        type: "fill",
        source: SOURCE_STATE,
        paint: { "fill-color": TOKENS.terrainBase, "fill-opacity": 0.16 },
      };
      const stateOutline: LineLayerSpecification = {
        id: LAYER_STATE_OUTLINE,
        type: "line",
        source: SOURCE_STATE,
        paint: { "line-color": TOKENS.hairline, "line-width": 1 },
      };
      const zonesFill: FillLayerSpecification = {
        id: LAYER_ZONES_FILL,
        type: "fill",
        source: SOURCE_ZONES,
        paint:
          lens === "price"
            ? { "fill-color": priceColor(live.priceRtm), "fill-opacity": 0.35 }
            : { "fill-color": TOKENS.terrainBase, "fill-opacity": 0.12 },
      };
      const zonesOutline: LineLayerSpecification = {
        id: LAYER_ZONES_OUTLINE,
        type: "line",
        source: SOURCE_ZONES,
        paint: { "line-color": TOKENS.hairline, "line-width": 0.75, "line-opacity": 0.8 },
      };
      const zonesLabel: SymbolLayerSpecification = {
        id: LAYER_ZONES_LABEL,
        type: "symbol",
        source: SOURCE_ZONE_LABELS,
        layout: {
          "text-field": ["get", "label"],
          "text-font": LABEL_FONT,
          "text-size": 11,
          "text-allow-overlap": false,
        },
        paint: {
          "text-color": TOKENS.textSecondary,
          "text-halo-color": TOKENS.bgBase,
          "text-halo-width": 1,
        },
      };
      const homes: CircleLayerSpecification = {
        id: LAYER_HOMES,
        type: "circle",
        source: SOURCE_HOMES,
        paint: {
          "circle-radius": ["interpolate", ["linear"], ["zoom"], 5, 2.5, 12, 6.5],
          "circle-color": lens === "soc" ? ["get", "color"] : priceColor(live.priceRtm),
          "circle-opacity": 0.9,
          "circle-stroke-color": TOKENS.hairline,
          "circle-stroke-width": 1,
        },
      };
      const homesSelected: CircleLayerSpecification = {
        id: LAYER_HOMES_SELECTED,
        type: "circle",
        source: SOURCE_HOMES,
        filter: ["==", ["get", "id"], useAppStore.getState().selectedHomeId ?? ""],
        paint: {
          "circle-radius": ["interpolate", ["linear"], ["zoom"], 5, 4, 12, 8],
          "circle-color": "rgba(0,0,0,0)",
          "circle-stroke-color": TOKENS.energyDischarge,
          "circle-stroke-width": 2.5,
        },
      };

      map.addLayer(stateFill);
      map.addLayer(stateOutline);
      map.addLayer(zonesFill);
      map.addLayer(zonesOutline);
      map.addLayer(zonesLabel);
      map.addLayer(homes);
      map.addLayer(homesSelected);

      layersReady = true;
      lastVersionRef.current = live.version;
      lastCountRef.current = live.count;
      wireInteractions();
    };

    map.on("load", () => {
      void (async () => {
        let zonesFc: FeatureCollection = EMPTY_FC;
        try {
          const res = await fetch(ZONES_URL, { signal: abort.signal });
          zonesFc = (await res.json()) as FeatureCollection;
        } catch {
          // Aborted on unmount, or offline: zones render empty, homes still show.
        }
        if (cancelled) return;
        addLayers(zonesFc);
      })();
    });

    const timer = window.setInterval(() => {
      if (!activeRef.current) return;
      pushTelemetry(false);
    }, TELEMETRY_PUSH_MS);

    const unsubscribe = useAppStore.subscribe((state, prev) => {
      if (state.lens !== prev.lens) {
        lensRef.current = state.lens;
        if (map.getLayer(LAYER_HOMES)) {
          if (state.lens === "price") {
            map.setPaintProperty(LAYER_HOMES, "circle-color", priceColor(live.priceRtm));
          } else {
            map.setPaintProperty(LAYER_HOMES, "circle-color", ["get", "color"]);
          }
        }
        pushTelemetry(true);
      }
      if (state.selectedHomeId !== prev.selectedHomeId && map.getLayer(LAYER_HOMES_SELECTED)) {
        const filter: FilterSpecification = ["==", ["get", "id"], state.selectedHomeId ?? ""];
        map.setFilter(LAYER_HOMES_SELECTED, filter);
      }
    });

    return () => {
      cancelled = true;
      abort.abort();
      window.clearInterval(timer);
      unsubscribe();
      mapRef.current = null;
      map.remove();
    };
    // The map instance and its telemetry source live for the component's
    // lifetime; the live buffer reference is stable from bootstrap.
  }, [live]);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;
    container.style.visibility = active ? "visible" : "hidden";
    const map = mapRef.current;
    if (!map) return;
    if (active) {
      // Re-measure in case layout shifted while the stratum was inactive.
      map.resize();
    } else {
      map.stop();
    }
  }, [active]);

  return (
    <div
      ref={containerRef}
      style={{
        position: "absolute",
        inset: 0,
        background: TOKENS.bgBase,
        visibility: active ? "visible" : "hidden",
      }}
    />
  );
}
