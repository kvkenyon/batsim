/**
 * App shell: boot the data layer, then stack the map and neighborhood
 * viewports with a crossfade between strata, under the HUD chrome.
 */

import { useEffect, useState } from "react";
import MapView from "../map/MapView";
import NeighborhoodView from "../scene/NeighborhoodView";
import { bootstrap } from "../state/bootstrap";
import { getRuntime } from "../state/runtime";
import { useAppStore } from "../state/store";
import { ReplayTransport } from "../state/transport";
import { cssVarOverrides } from "../hud/cssVars";
import { DispatchPanel } from "../hud/DispatchPanel";
import { EventsFeed } from "../hud/EventsFeed";
import { Inspector } from "../hud/Inspector";
import { KpiStrip } from "../hud/KpiStrip";
import { TopBar } from "../hud/TopBar";
import { ViewportBoundary } from "./ViewportBoundary";

declare global {
  interface Window {
    /** Ops/e2e handle: inspect and drive the console from the console. */
    __batsim?: { store: typeof useAppStore; seekToSimTime?: (ms: number) => boolean };
  }
}

export function App() {
  const [ready, setReady] = useState(false);
  const [bootError, setBootError] = useState<string | null>(null);
  // The stratum crossfade is armed only after the shell's first paint:
  // while it is armed at mount, the browser fades the freshly inserted
  // inactive layer in from the default styles, leaving it hit-testable
  // (and on top of the map) for the duration of the fade.
  const [crossfadeArmed, setCrossfadeArmed] = useState(false);
  const stratum = useAppStore((s) => s.stratum);
  const selectedHomeId = useAppStore((s) => s.selectedHomeId);
  const selectedMeta = useAppStore((s) => (s.selectedHomeId ? s.homesMeta[s.selectedHomeId] : undefined));
  const neighborhoodZone = useAppStore((s) => s.neighborhoodZone);
  const mapZoom = useAppStore((s) => s.mapZoom);
  const lastError = useAppStore((s) => s.lastError);

  useEffect(() => {
    for (const [key, value] of Object.entries(cssVarOverrides)) {
      document.documentElement.style.setProperty(key, value);
    }
    const params = new URLSearchParams(window.location.search);
    bootstrap({ forceDemo: params.get("demo") === "1" })
      .then(() => {
        setReady(true);
        const { transport } = getRuntime();
        if (transport instanceof ReplayTransport && window.__batsim) {
          window.__batsim.seekToSimTime = (ms: number) => transport.seekToSimTime(ms);
        }
      })
      .catch((err: unknown) => setBootError(err instanceof Error ? err.message : String(err)));
    window.__batsim = { store: useAppStore };
  }, []);

  useEffect(() => {
    if (!ready) return;
    const frame = requestAnimationFrame(() => setCrossfadeArmed(true));
    return () => cancelAnimationFrame(frame);
  }, [ready]);

  // Esc ascends one stratum, then clears the selection.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      const state = useAppStore.getState();
      if (state.stratum === "neighborhood") state.setStratum("map");
      else if (state.selectedHomeId !== null) state.selectHome(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  if (bootError) {
    return (
      <div className="boot-screen">
        <div className="title">batsim console failed to start</div>
        <div className="detail">{bootError}</div>
      </div>
    );
  }

  if (!ready) {
    return (
      <div className="boot-screen">
        <div className="title">◈ batsim</div>
        <div className="detail">connecting to the fleet…</div>
      </div>
    );
  }

  const { live } = getRuntime();

  return (
    <div className={`app-shell${crossfadeArmed ? " crossfade-armed" : ""}`}>
      <div
        className={`viewport-layer ${stratum === "map" ? "active" : "inactive"}`}
      >
        <ViewportBoundary name="map">
          <MapView live={live} active={stratum === "map"} />
        </ViewportBoundary>
      </div>
      <div
        className={`viewport-layer ${stratum === "neighborhood" ? "active" : "inactive"}`}
      >
        <ViewportBoundary name="neighborhood">
          <NeighborhoodView live={live} active={stratum === "neighborhood"} />
        </ViewportBoundary>
      </div>

      <TopBar />
      <div className="stratum-chip hud-panel">
        {stratum === "map" ? (
          <span className="crumb">ERCOT · Texas</span>
        ) : (
          <>
            <button className="crumb-link" onClick={() => useAppStore.getState().setStratum("map")}>
              ERCOT · Texas
            </button>
            <span>▸</span>
            <span className="crumb">neighborhood · {neighborhoodZone}</span>
            <span className="hint">esc to return</span>
          </>
        )}
      </div>
      {stratum === "map" && mapZoom >= 9 && (
        <button
          className="dive-chip"
          onClick={() => useAppStore.getState().setStratum("neighborhood")}>
          dive into <span className="zone">{neighborhoodZone}</span> neighborhood
        </button>
      )}
      <EventsFeed />
      <DispatchPanel />
      {selectedHomeId && selectedMeta && <Inspector meta={selectedMeta} />}
      {lastError && <div className="error-toast hud-panel">{lastError}</div>}
      <KpiStrip />
    </div>
  );
}
