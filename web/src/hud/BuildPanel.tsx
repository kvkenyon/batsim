/**
 * Build mode panel: pick a device model from the registry catalog, arm
 * placement, then click inside an ERCOT zone on the map to site a home.
 * Placement stays armed until it is cancelled (button or Esc), so a
 * street can be built up in a run of clicks. Live placements go through
 * the homes API; demo placements are local to the replay world and the
 * status line says so.
 */

import { useAppStore } from "../state/store";

export function BuildPanel() {
  const catalog = useAppStore((s) => s.catalog);
  const buildMode = useAppStore((s) => s.buildMode);
  const buildModelId = useAppStore((s) => s.buildModelId);
  const buildStatus = useAppStore((s) => s.buildStatus);
  const connection = useAppStore((s) => s.connection);
  const setBuildMode = useAppStore((s) => s.setBuildMode);
  const setBuildModel = useAppStore((s) => s.setBuildModel);

  const selected = buildModelId ?? catalog[0]?.modelId ?? null;
  const canPlace = selected !== null;

  return (
    <section className="build-panel hud-panel" aria-label="build mode">
      <div className="head">build mode · {connection === "demo" ? "demo world" : "live fleet"}</div>
      <label className="field">
        <span className="k">device</span>
        <select
          className="model-select"
          value={selected ?? ""}
          onChange={(e) => setBuildModel(e.target.value)}
          aria-label="device model"
        >
          {catalog.map((b) => (
            <option key={b.modelId} value={b.modelId}>
              {b.displayName} · {b.usableEnergyKwh} kWh
            </option>
          ))}
        </select>
      </label>
      <div className="buttons">
        <button
          className={`cmd place${buildMode ? " armed" : ""}`}
          disabled={!canPlace}
          onClick={() => {
            if (buildMode) {
              setBuildMode(false);
            } else {
              setBuildModel(selected);
              setBuildMode(true);
              useAppStore.setState({ buildStatus: "click inside an ERCOT zone to place" });
            }
          }}
          title={
            buildMode
              ? "cancel placement"
              : "arm placement, then click inside an ERCOT zone on the map"
          }
        >
          {buildMode ? "✕ cancel placement" : "＋ place on map"}
        </button>
      </div>
      <div className="status t-num-s">
        {buildStatus ??
          (connection === "demo"
            ? "placements are local to the demo world"
            : "homes are created through the API")}
      </div>
    </section>
  );
}
