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
import { cssVarOverrides } from "../hud/cssVars";
import { Inspector } from "../hud/Inspector";
import { KpiStrip } from "../hud/KpiStrip";
import { TopBar } from "../hud/TopBar";
import { ViewportBoundary } from "./ViewportBoundary";

declare global {
  interface Window {
    /** Ops/e2e handle: inspect and drive the console from the console. */
    __batsim?: { store: typeof useAppStore };
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
  const lastError = useAppStore((s) => s.lastError);

  useEffect(() => {
    for (const [key, value] of Object.entries(cssVarOverrides)) {
      document.documentElement.style.setProperty(key, value);
    }
    const params = new URLSearchParams(window.location.search);
    bootstrap({ forceDemo: params.get("demo") === "1" })
      .then(() => setReady(true))
      .catch((err: unknown) => setBootError(err instanceof Error ? err.message : String(err)));
    window.__batsim = { store: useAppStore };
  }, []);

  useEffect(() => {
    if (!ready) return;
    const frame = requestAnimationFrame(() => setCrossfadeArmed(true));
    return () => cancelAnimationFrame(frame);
  }, [ready]);

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
          <span>
            <span className="crumb">ERCOT · Texas</span> ▸ neighborhood · {neighborhoodZone}
          </span>
        )}
      </div>
      {selectedHomeId && selectedMeta && <Inspector meta={selectedMeta} />}
      {lastError && <div className="error-toast hud-panel">{lastError}</div>}
      <KpiStrip />
    </div>
  );
}
