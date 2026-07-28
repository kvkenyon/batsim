/**
 * Fleet scenarios: save the current world (homes, device models,
 * positions, zones) under a name, list, load, and delete. Snapshots are
 * stored in this browser's local storage - the server's scenario API
 * binds time and prices to a run and has no fleet composition to hold -
 * and the panel labels that honestly. Loading replaces the current
 * fleet: through the homes API against a live server, or as a local
 * world swap in the demo replay.
 */

import { useState } from "react";
import { getController } from "../state/controls";
import { deleteScenario, listScenarios, saveScenario, type SavedScenario } from "../state/scenarios";
import { useAppStore } from "../state/store";

function formatSavedAt(iso: string): string {
  const date = new Date(iso);
  return Number.isFinite(date.getTime())
    ? date.toLocaleString("en-US", { month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", hour12: false })
    : iso;
}

export function ScenariosPanel() {
  const homeCount = useAppStore((s) => s.homeOrder.length);
  const [scenarios, setScenarios] = useState<SavedScenario[]>(() => listScenarios());
  const [name, setName] = useState("");

  const refresh = () => setScenarios(listScenarios());

  const save = () => {
    const trimmed = name.trim();
    if (trimmed === "") return;
    saveScenario(trimmed);
    setName("");
    refresh();
  };

  return (
    <section className="scenarios-panel hud-panel" aria-label="fleet scenarios">
      <div className="head">scenarios · stored in this browser</div>
      <div className="save-row">
        <input
          className="name-input"
          value={name}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") save();
          }}
          placeholder="name this fleet"
          aria-label="scenario name"
        />
        <button
          className="cmd save"
          disabled={name.trim() === "" || homeCount === 0}
          onClick={save}
          title={`snapshot the current ${homeCount} homes under this name`}
        >
          save
        </button>
      </div>
      {scenarios.length === 0 ? (
        <div className="status t-num-s">no saved scenarios</div>
      ) : (
        <ul className="scenario-list">
          {scenarios.map((s) => (
            <li key={s.name} className="scenario-row">
              <div className="meta">
                <span className="name">{s.name}</span>
                <span className="sub t-num-s">
                  {s.homes.length} homes · {formatSavedAt(s.savedAt)}
                </span>
              </div>
              <button
                className="cmd load"
                onClick={() => getController().loadScenario(s.homes)}
                title="replace the current fleet with this snapshot"
              >
                load
              </button>
              <button
                className="cmd delete"
                onClick={() => {
                  deleteScenario(s.name);
                  refresh();
                }}
                title="delete this snapshot"
              >
                ✕
              </button>
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
