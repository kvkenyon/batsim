/**
 * Grid-events feed: the console's pulse. Price crossings, scarcity
 * watch, solar ramps, fleet charge/discharge swings, dispatch
 * acknowledgments, and scenario time markers arrive newest-first, each
 * with its sim timestamp and a severity tint down the leading edge.
 */

import { useEffect, useRef } from "react";
import { useAppStore } from "../state/store";

const feedTimeFormatter = new Intl.DateTimeFormat("en-US", {
  timeZone: "America/Chicago",
  hour: "2-digit",
  minute: "2-digit",
  hourCycle: "h23",
});

export function EventsFeed() {
  const events = useAppStore((s) => s.events);
  const listRef = useRef<HTMLDivElement | null>(null);
  const newestId = events[0]?.id;

  // Keep the feed pinned to the newest entry as events stream in.
  useEffect(() => {
    listRef.current?.scrollTo({ top: 0 });
  }, [newestId]);

  return (
    <section className="events-feed hud-panel" aria-label="grid events">
      <div className="feed-head">
        <span className="t-micro">grid events</span>
        <span className="t-num-s">{events.length}</span>
      </div>
      <div className="feed-list" ref={listRef}>
        {events.length === 0 && (
          <div className="event-row sev-info">
            <span className="msg t-label">listening to the grid…</span>
          </div>
        )}
        {events.map((event, index) => (
          <div
            key={event.id}
            className={`event-row sev-${event.severity} kind-${event.kind}${index === 0 ? " fresh" : ""}`}
          >
            <span className="t t-num-s">
              {event.simTimeMs > 0 ? feedTimeFormatter.format(new Date(event.simTimeMs)) : "--:--"}
            </span>
            <span className="msg t-label">{event.message}</span>
          </div>
        ))}
      </div>
    </section>
  );
}
