/**
 * Top bar: scenario identity, dual clock (simulated and wall time), lens
 * selector, and the demo-mode badge.
 */

import { useEffect, useState } from "react";
import { useAppStore, type Lens } from "../state/store";
import { priceColor } from "../tokens/tokens";

const simTimeFormatter = new Intl.DateTimeFormat("en-US", {
  timeZone: "America/Chicago",
  year: "numeric",
  month: "2-digit",
  day: "2-digit",
  hour: "2-digit",
  minute: "2-digit",
  second: "2-digit",
  hour12: false,
});

function formatWall(date: Date): string {
  return date.toLocaleTimeString("en-US", { hour12: false });
}

const LENSES: Array<{ id: Lens; label: string }> = [
  { id: "price", label: "price" },
  { id: "soc", label: "soc" },
];

export function TopBar() {
  const scenarioName = useAppStore((s) => s.scenarioName);
  const connection = useAppStore((s) => s.connection);
  const simTimeMs = useAppStore((s) => s.simTimeMs);
  const lens = useAppStore((s) => s.lens);
  const setLens = useAppStore((s) => s.setLens);
  const [wall, setWall] = useState(() => new Date());

  useEffect(() => {
    const timer = setInterval(() => setWall(new Date()), 1000);
    return () => clearInterval(timer);
  }, []);

  return (
    <header className="top-bar">
      <span className="brand">◈ batsim</span>
      <span className="scenario">{scenarioName}</span>
      {connection === "demo" && <span className="demo-badge">demo replay</span>}
      <span className="spacer" />
      <span className="clock mono">
        <span className="label">sim</span>
        {simTimeMs > 0 ? `${simTimeFormatter.format(new Date(simTimeMs))} CT` : "-"}
      </span>
      <span className="clock mono">
        <span className="label">wall</span>
        {formatWall(wall)}
      </span>
      <div className="lens-toggle" role="group" aria-label="map lens">
        {LENSES.map((l) => (
          <button
            key={l.id}
            className={lens === l.id ? "active" : ""}
            onClick={() => setLens(l.id)}
            style={lens === l.id && l.id === "price" ? { color: priceColor(useAppStore.getState().priceRtm) } : undefined}
          >
            {l.label}
          </button>
        ))}
      </div>
    </header>
  );
}
