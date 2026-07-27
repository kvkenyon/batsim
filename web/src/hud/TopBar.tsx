/**
 * Top bar: scenario identity, transport controls (pause, speed, jump to
 * the next price moment), dual clock (simulated and wall time), lens
 * selector, and the demo-mode badge.
 */

import { useEffect, useState } from "react";
import { getController } from "../state/controls";
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

const SPEEDS = [1, 60, 3600] as const;

export function TopBar() {
  const scenarioName = useAppStore((s) => s.scenarioName);
  const connection = useAppStore((s) => s.connection);
  const simTimeMs = useAppStore((s) => s.simTimeMs);
  const lens = useAppStore((s) => s.lens);
  const setLens = useAppStore((s) => s.setLens);
  const paused = useAppStore((s) => s.paused);
  const speedMult = useAppStore((s) => s.speedMult);
  const [wall, setWall] = useState(() => new Date());

  useEffect(() => {
    const timer = setInterval(() => setWall(new Date()), 1000);
    return () => clearInterval(timer);
  }, []);

  return (
    <header className="top-bar">
      <span className="brand">batsim</span>
      <span className="scenario">{scenarioName}</span>
      {connection === "demo" && <span className="demo-badge">demo replay</span>}
      <div className="transport" role="group" aria-label="time controls">
        <div className="seg">
          <button
            className={paused ? "active" : ""}
            onClick={() => getController().setPaused(!paused)}
            title={paused ? "resume the simulation" : "pause the simulation"}
          >
            {paused ? "resume" : "pause"}
          </button>
          {SPEEDS.map((mult) => (
            <button
              key={mult}
              className={!paused && speedMult === mult ? "active" : ""}
              onClick={() => {
                const controller = getController();
                controller.setSpeed(mult);
                if (paused) controller.setPaused(false);
              }}
            >
              {mult}x
            </button>
          ))}
        </div>
        <button
          className="jump"
          onClick={() => getController().jumpToNextPriceEvent()}
          title="jump to the next price event"
        >
          next price event ▸
        </button>
      </div>
      <span className="spacer" />
      <span className={`clock mono${paused ? " paused" : ""}`}>
        <span className="label">sim</span>
        {simTimeMs > 0 ? `${simTimeFormatter.format(new Date(simTimeMs))} CT` : "-"}
      </span>
      <span className="clock mono">
        <span className="label">wall</span>
        {formatWall(wall)}
      </span>
      <div className="seg" role="group" aria-label="map lens">
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
