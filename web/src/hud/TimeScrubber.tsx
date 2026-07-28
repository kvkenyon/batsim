/**
 * Time scrubber: drag through the recorded day. Scrubbing seeks the demo
 * replay tape; the day-arc lighting, price lens, and home states all
 * follow because they are pure functions of the sim clock the tape
 * republishes. Against a live API the world cannot rewind, so the bar
 * stays disabled and says why.
 */

import { useState } from "react";
import { getController } from "../state/controls";
import { useAppStore } from "../state/store";

const ctTime = new Intl.DateTimeFormat("en-US", {
  timeZone: "America/Chicago",
  hour: "2-digit",
  minute: "2-digit",
  hour12: false,
});

const ctDay = new Intl.DateTimeFormat("en-US", {
  timeZone: "America/Chicago",
  month: "2-digit",
  day: "2-digit",
});

export function TimeScrubber() {
  const traceRangeMs = useAppStore((s) => s.traceRangeMs);
  const simTimeMs = useAppStore((s) => s.simTimeMs);
  const [dragMs, setDragMs] = useState<number | null>(null);

  const scrubbable = traceRangeMs !== null;
  const [start, end] = traceRangeMs ?? [0, 1];
  const position = dragMs ?? Math.min(Math.max(simTimeMs, start), end);

  const seek = (value: number) => {
    setDragMs(value);
    getController().seekTo(value);
  };

  return (
    <div className={`scrubber-bar${scrubbable ? "" : " disabled"}`} aria-label="time scrubber">
      <span className="bound mono">
        {ctDay.format(new Date(start))} {ctTime.format(new Date(start))}
      </span>
      <input
        type="range"
        className="scrub"
        min={start}
        max={end}
        step={60_000}
        value={position}
        disabled={!scrubbable}
        onChange={(e) => seek(Number(e.target.value))}
        onPointerUp={() => setDragMs(null)}
        onKeyUp={() => setDragMs(null)}
        aria-label="scrub the recorded day"
        title={scrubbable ? "drag to seek the recorded day" : "live telemetry cannot be scrubbed"}
      />
      <span className="bound mono">{ctTime.format(new Date(end))}</span>
      <span className="position mono">
        {scrubbable ? `${ctTime.format(new Date(position))} CT` : "live · not scrubbable"}
      </span>
    </div>
  );
}
