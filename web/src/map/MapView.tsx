/**
 * Map stratum: the whole-fleet view. CartoDB Dark Matter vector-derived
 * raster tiles carry the real geography; ERCOT load-zone polygons from
 * the bundled GeoJSON sit on top as a subtle overlay with hover
 * highlighting; one crisp dot per home is lensed by SOC or price, with a
 * soft glow at high zoom. A canvas overlay animates service-line flow
 * and dispatch ripples on top. Live telemetry is read imperatively from
 * the live buffers at a low cadence, never through React state.
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

import { dayArc } from "../state/dayArc";
import type { LiveBuffers } from "../state/live";
import { useAppStore, type Lens } from "../state/store";
import { TOKENS, priceColor, priceMarkerColor, socColor } from "../tokens/tokens";
import { MapFlowOverlay } from "./flowOverlay";

export interface MapViewProps {
  live: LiveBuffers;
  active: boolean;
}

const ZONES_URL = "geo/texas-zones.json";
/** Glyph PBFs for the zone-label symbol layer, vendored for offline use. */
const GLYPHS_URL = "fonts/{fontstack}/{range}.pbf";
const LABEL_FONT = ["open-sans-semibold"];
/** Free dark basemap, no API key; degrades to the bare background offline. */
const BASEMAP_TILES = ["https://basemaps.cartocdn.com/dark_all/{z}/{x}/{y}.png"];
const BASEMAP_ATTRIBUTION = "© OpenStreetMap contributors © CARTO";

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
/** Telemetry push cadence for marker and zone tints. */
const TELEMETRY_PUSH_MS = 500;

const SOURCE_BASEMAP = "basemap";
const SOURCE_STATE = "texas-state";
const SOURCE_ZONES = "texas-zones";
const SOURCE_ZONE_LABELS = "texas-zone-labels";
const SOURCE_HOMES = "homes";

const LAYER_BASEMAP = "basemap";
const LAYER_STATE_FILL = "state-fill";
const LAYER_STATE_OUTLINE = "state-outline";
const LAYER_ZONES_FILL = "zones-fill";
const LAYER_ZONES_FILL_HOVER = "zones-fill-hover";
const LAYER_ZONES_OUTLINE = "zones-outline";
const LAYER_ZONES_OUTLINE_HOVER = "zones-outline-hover";
const LAYER_ZONES_LABEL = "zones-label";
const LAYER_HOMES_GLOW = "homes-glow";
const LAYER_HOMES = "homes";
const LAYER_HOMES_SELECTED = "homes-selected";

type ZoneGeometry = Polygon | MultiPolygon;

const EMPTY_FC: FeatureCollection = { type: "FeatureCollection", features: [] };
const NO_ZONE: FilterSpecification = ["==", ["get", "zone"], ""];

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
  const veilRef = useRef<HTMLDivElement | null>(null);
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
      name: "batsim-dark",
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
      attributionControl: { compact: true },
      cooperativeGestures: false,
    });
    mapRef.current = map;
    // Ops/e2e handle for driving the map from the console.
    (window as unknown as { __batsimMap?: MaplibreMap }).__batsimMap = map;
    map.addControl(new maplibregl.NavigationControl({ showCompass: false }), "top-right");
    // Double-click is the dive gesture; zoom-on-double-click would fight it.
    map.doubleClickZoom.disable();

    const overlay = new MapFlowOverlay(container);
    overlay.attach(map, live);
    overlay.start();

    /** Zone styling for the active lens: the fill stays a quiet neutral;
     * the price lens speaks through the boundary color, not a flood fill. */
    const applyZoneTint = (lens: Lens) => {
      if (!map.getLayer(LAYER_ZONES_FILL)) return;
      map.setPaintProperty(LAYER_ZONES_FILL, "fill-color", TOKENS.terrainBase);
      map.setPaintProperty(LAYER_ZONES_FILL, "fill-opacity", 0.16);
      map.setPaintProperty(
        LAYER_ZONES_OUTLINE,
        "line-color",
        lens === "price" ? priceColor(live.priceRtm) : TOKENS.hairline,
      );
      map.setPaintProperty(LAYER_ZONES_OUTLINE, "line-width", lens === "price" ? 1.5 : 0.75);
    };

    /** Simulated-time-of-day veil over the whole map. */
    const applyDayVeil = () => {
      const veil = veilRef.current;
      if (!veil) return;
      const arc = dayArc(live.simTimeMs > 0 ? live.simTimeMs : Date.now());
      veil.style.opacity = arc.veilOpacity.toFixed(3);
      veil.style.backgroundColor = arc.veilColor;
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
        map.setPaintProperty(LAYER_HOMES, "circle-color", priceMarkerColor(live.priceRtm));
        map.setPaintProperty(LAYER_HOMES_GLOW, "circle-color", priceMarkerColor(live.priceRtm));
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

      // Zone hover highlighting: a brightened fill + outline pair filtered
      // to the zone under the pointer.
      let hoveredZone = "";
      map.on("mousemove", LAYER_ZONES_FILL, (e) => {
        const zone: unknown = e.features?.[0]?.properties?.zone;
        const next = typeof zone === "string" ? zone : "";
        if (next === hoveredZone) return;
        hoveredZone = next;
        const filter: FilterSpecification = ["==", ["get", "zone"], hoveredZone];
        map.setFilter(LAYER_ZONES_FILL_HOVER, filter);
        map.setFilter(LAYER_ZONES_OUTLINE_HOVER, filter);
      });
      map.on("mouseleave", LAYER_ZONES_FILL, () => {
        hoveredZone = "";
        map.setFilter(LAYER_ZONES_FILL_HOVER, NO_ZONE);
        map.setFilter(LAYER_ZONES_OUTLINE_HOVER, NO_ZONE);
      });

      // Click a zone to frame it; double-click anywhere dives into the
      // street-level stratum. The zoom crossing below stays as an ambient
      // path, never the only one.
      map.on("click", LAYER_ZONES_FILL, (e) => {
        if (e.defaultPrevented) return;
        const feature = e.features?.[0];
        if (!feature) return;
        const geometry = feature.geometry;
        const coords =
          geometry.type === "Polygon" ? coordsOf(geometry.coordinates) : geometry.type === "MultiPolygon" ? coordsOf(geometry.coordinates[0] ?? []) : null;
        if (!coords) return;
        let west = Infinity;
        let south = Infinity;
        let east = -Infinity;
        let north = -Infinity;
        for (const [lng, lat] of coords) {
          if (lng < west) west = lng;
          if (lat < south) south = lat;
          if (lng > east) east = lng;
          if (lat > north) north = lat;
        }
        map.fitBounds(
          [
            [west, south],
            [east, north],
          ],
          { padding: 64, duration: 650 },
        );
      });
      // Zone under the map crosshair: the dive chip and the zoom
      // handoff both target this zone, not a fixed one.
      const updateCenterZone = () => {
        const center = map.getCenter();
        const features = map.queryRenderedFeatures(map.project([center.lng, center.lat]), {
          layers: [LAYER_ZONES_FILL],
        });
        const zone: unknown = features[0]?.properties?.zone;
        const next = typeof zone === "string" ? zone : null;
        if (useAppStore.getState().centerZone !== next) {
          useAppStore.setState({ centerZone: next });
        }
      };

      map.on("dblclick", (e) => {
        e.preventDefault();
        const features = map.queryRenderedFeatures(e.point, { layers: [LAYER_ZONES_FILL] });
        const zone: unknown = features[0]?.properties?.zone;
        useAppStore.getState().diveZone(typeof zone === "string" ? zone : null);
      });

      // Handoff is armed while zoomed out below the threshold and consumed
      // when it fires; re-entering the map stratum at a high zoom does not
      // immediately bounce back to the street level.
      let handoffArmed = map.getZoom() < NEIGHBORHOOD_ZOOM;
      map.on("moveend", () => {
        useAppStore.setState({ mapZoom: map.getZoom() });
        updateCenterZone();
        if (map.getZoom() < NEIGHBORHOOD_ZOOM) {
          handoffArmed = true;
          return;
        }
        if (!handoffArmed || !activeRef.current) return;
        const state = useAppStore.getState();
        if (state.stratum !== "map") return;
        if (state.centerZone !== null) {
          handoffArmed = false;
          state.diveZone(state.centerZone);
        }
      });
      updateCenterZone();
    };

    const addLayers = (zonesFc: FeatureCollection) => {
      const { state, zones, labels } = splitZones(zonesFc);
      const lens = lensRef.current;

      map.addSource(SOURCE_BASEMAP, {
        type: "raster",
        tiles: BASEMAP_TILES,
        tileSize: 256,
        attribution: BASEMAP_ATTRIBUTION,
      });
      map.addSource(SOURCE_STATE, { type: "geojson", data: state });
      map.addSource(SOURCE_ZONES, { type: "geojson", data: zones });
      map.addSource(SOURCE_ZONE_LABELS, { type: "geojson", data: labels });
      map.addSource(SOURCE_HOMES, { type: "geojson", data: buildHomesCollection(live, lens) });

      const basemap: maplibregl.RasterLayerSpecification = {
        id: LAYER_BASEMAP,
        type: "raster",
        source: SOURCE_BASEMAP,
        paint: {
          "raster-opacity": 0.96,
          "raster-contrast": 0.18,
          "raster-brightness-min": 0.12,
        },
      };
      const stateFill: FillLayerSpecification = {
        id: LAYER_STATE_FILL,
        type: "fill",
        source: SOURCE_STATE,
        paint: { "fill-color": TOKENS.terrainBase, "fill-opacity": 0.1 },
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
        paint: { "fill-color": TOKENS.terrainBase, "fill-opacity": 0.16 },
      };
      const zonesFillHover: FillLayerSpecification = {
        id: LAYER_ZONES_FILL_HOVER,
        type: "fill",
        source: SOURCE_ZONES,
        filter: NO_ZONE,
        paint: { "fill-color": TOKENS.textPrimary, "fill-opacity": 0.08 },
      };
      const zonesOutline: LineLayerSpecification = {
        id: LAYER_ZONES_OUTLINE,
        type: "line",
        source: SOURCE_ZONES,
        paint:
          lens === "price"
            ? { "line-color": priceColor(live.priceRtm), "line-width": 1.5, "line-opacity": 0.95 }
            : { "line-color": TOKENS.hairline, "line-width": 0.75, "line-opacity": 0.9 },
      };
      const zonesOutlineHover: LineLayerSpecification = {
        id: LAYER_ZONES_OUTLINE_HOVER,
        type: "line",
        source: SOURCE_ZONES,
        filter: NO_ZONE,
        paint: { "line-color": TOKENS.textSecondary, "line-width": 1.75, "line-opacity": 0.95 },
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
          "text-halo-width": 1.25,
        },
      };
      const homesGlow: CircleLayerSpecification = {
        id: LAYER_HOMES_GLOW,
        type: "circle",
        source: SOURCE_HOMES,
        paint: {
          "circle-radius": ["interpolate", ["linear"], ["zoom"], 8, 4, 13, 16],
          "circle-color": lens === "soc" ? ["get", "color"] : priceMarkerColor(live.priceRtm),
          "circle-blur": 1,
          "circle-opacity": ["interpolate", ["linear"], ["zoom"], 8, 0, 10.5, 0.45],
        },
      };
      const homes: CircleLayerSpecification = {
        id: LAYER_HOMES,
        type: "circle",
        source: SOURCE_HOMES,
        paint: {
          "circle-radius": ["interpolate", ["linear"], ["zoom"], 5, 2.5, 12, 6.5],
          "circle-color": lens === "soc" ? ["get", "color"] : priceMarkerColor(live.priceRtm),
          "circle-opacity": 0.92,
          "circle-stroke-color": TOKENS.bgDeep,
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

      map.addLayer(basemap);
      map.addLayer(stateFill);
      map.addLayer(stateOutline);
      map.addLayer(zonesFill);
      map.addLayer(zonesFillHover);
      map.addLayer(zonesOutline);
      map.addLayer(zonesOutlineHover);
      map.addLayer(zonesLabel);
      map.addLayer(homesGlow);
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
      applyDayVeil();
    }, TELEMETRY_PUSH_MS);

    const unsubscribe = useAppStore.subscribe((state, prev) => {
      if (state.lens !== prev.lens) {
        lensRef.current = state.lens;
        if (map.getLayer(LAYER_HOMES)) {
          if (state.lens === "price") {
            map.setPaintProperty(LAYER_HOMES, "circle-color", priceMarkerColor(live.priceRtm));
            map.setPaintProperty(LAYER_HOMES_GLOW, "circle-color", priceMarkerColor(live.priceRtm));
          } else {
            map.setPaintProperty(LAYER_HOMES, "circle-color", ["get", "color"]);
            map.setPaintProperty(LAYER_HOMES_GLOW, "circle-color", ["get", "color"]);
          }
        }
        pushTelemetry(true);
      }
      if (state.selectedHomeId !== prev.selectedHomeId && map.getLayer(LAYER_HOMES_SELECTED)) {
        const filter: FilterSpecification = ["==", ["get", "id"], state.selectedHomeId ?? ""];
        map.setFilter(LAYER_HOMES_SELECTED, filter);
      }
      if (state.dispatchWave !== prev.dispatchWave) {
        overlay.triggerDispatchWave();
      }
    });

    return () => {
      cancelled = true;
      abort.abort();
      window.clearInterval(timer);
      unsubscribe();
      overlay.detach();
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
    >
      <div ref={veilRef} className="day-veil" style={{ opacity: 0 }} />
    </div>
  );
}

/** Flatten the first (outer) ring of a polygon coordinate set. */
function coordsOf(rings: number[][][]): [number, number][] {
  const ring = rings[0] ?? [];
  const out: [number, number][] = [];
  for (const pair of ring) {
    if (typeof pair[0] === "number" && typeof pair[1] === "number") {
      out.push([pair[0], pair[1]]);
    }
  }
  return out;
}
