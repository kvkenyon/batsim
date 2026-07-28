/**
 * Dispatch console. Zone scope sends a charge, discharge, or idle
 * command at only the selected zone's homes and tracks the per-home
 * acknowledgements as they arrive (live: polled command detail; demo:
 * the recorded fleet dispatch's true per-device latency, counted for the
 * zone's homes). Fleet scope keeps the original one-click fleet-wide
 * moment. Either way the fleet answers home by home with jittered
 * per-device latency, exactly like a real cloud fleet.
 */

import { useState } from "react";
import { getController, type DispatchDirection } from "../state/controls";
import { useAppStore } from "../state/store";

export function DispatchPanel() {
  const connection = useAppStore((s) => s.connection);
  const status = useAppStore((s) => s.dispatchStatus);
  const canReplayDispatch = useAppStore((s) => s.canReplayDispatch);
  const homes = useAppStore((s) => s.fleet.homes);
  const zoneAck = useAppStore((s) => s.zoneAck);
  const homesMeta = useAppStore((s) => s.homesMeta);
  const homeOrder = useAppStore((s) => s.homeOrder);
  const zoneLabels = useAppStore((s) => s.zoneLabels);
  const centerZone = useAppStore((s) => s.centerZone);
  const [scope, setScope] = useState<"zone" | "fleet">("zone");
  const [pickedZone, setPickedZone] = useState<string | null>(null);
  const isDemo = connection === "demo";

  // Zones that actually have homes, with counts, in stable order.
  const zoneCounts = new Map<string, number>();
  for (const id of homeOrder) {
    const zone = homesMeta[id]?.zone;
    if (zone) zoneCounts.set(zone, (zoneCounts.get(zone) ?? 0) + 1);
  }
  const zones = [...zoneCounts.keys()].sort();
  const zone =
    pickedZone !== null && zoneCounts.has(pickedZone)
      ? pickedZone
      : centerZone !== null && zoneCounts.has(centerZone)
        ? centerZone
        : (zones[0] ?? null);
  const zoneHomeCount = zone !== null ? (zoneCounts.get(zone) ?? 0) : 0;

  const fleetDisabled = homes === 0 || (isDemo && !canReplayDispatch);
  const zoneDisabled = zone === null || zoneHomeCount === 0 || (isDemo && !canReplayDispatch);

  const fireZone = (direction: DispatchDirection) => {
    if (zone !== null) getController().dispatchZone(zone, direction);
  };

  const ackText =
    zoneAck !== null
      ? `${zoneAck.direction} ${zoneAck.zone} · acked ${zoneAck.acked}/${zoneAck.expected}${zoneAck.done ? " · complete" : ""}`
      : null;

  return (
    <section className="dispatch-panel hud-panel" aria-label="dispatch console">
      <div className="head">dispatch console · {homes} homes</div>
      <div className="seg scope" role="group" aria-label="dispatch scope">
        <button className={scope === "zone" ? "active" : ""} onClick={() => setScope("zone")}>
          zone
        </button>
        <button className={scope === "fleet" ? "active" : ""} onClick={() => setScope("fleet")}>
          fleet
        </button>
      </div>

      {scope === "zone" ? (
        <>
          <label className="field">
            <span className="k">zone</span>
            <select
              className="zone-select"
              value={zone ?? ""}
              onChange={(e) => setPickedZone(e.target.value)}
              aria-label="dispatch zone"
            >
              {zones.map((z) => (
                <option key={z} value={z}>
                  {zoneLabels[z] ?? z} · {zoneCounts.get(z) ?? 0}
                </option>
              ))}
            </select>
          </label>
          <div className="buttons">
            <button
              className="cmd discharge"
              disabled={zoneDisabled}
              onClick={() => fireZone("discharge")}
              title={
                isDemo
                  ? "replay the recorded fleet discharge and count this zone's homes as they respond"
                  : `discharge ${zoneHomeCount} homes in ${zone ?? ""} at 5 kW per home`
              }
            >
              ▼ discharge
            </button>
            <button
              className="cmd charge"
              disabled={zoneDisabled}
              onClick={() => fireZone("charge")}
              title={isDemo ? "replay the recorded fleet dispatch" : `charge ${zoneHomeCount} homes in ${zone ?? ""} at 5 kW per home`}
            >
              ▲ charge
            </button>
            <button
              className="cmd idle"
              disabled={zoneDisabled || isDemo}
              onClick={() => fireZone("idle")}
              title={
                isDemo
                  ? "the recording holds no idle command"
                  : `return ${zoneHomeCount} homes in ${zone ?? ""} to self-consumption`
              }
            >
              ■ idle
            </button>
          </div>
          <div className="status t-num-s">
            {ackText ?? (zoneDisabled ? "no dispatch in this recording" : `${zoneHomeCount} homes in scope`)}
          </div>
        </>
      ) : (
        <>
          <div className="buttons">
            <button
              className="cmd discharge"
              disabled={fleetDisabled}
              onClick={() => getController().dispatchFleet("discharge")}
              title={isDemo ? "replay the recorded fleet discharge" : "discharge the fleet at 5 kW per home"}
            >
              ▼ discharge
            </button>
            <button
              className="cmd charge"
              disabled={fleetDisabled}
              onClick={() => getController().dispatchFleet("charge")}
              title={isDemo ? "replay the recorded fleet dispatch" : "charge the fleet at 5 kW per home"}
            >
              ▲ charge
            </button>
          </div>
          <div className="status t-num-s">{status ?? (fleetDisabled ? "no dispatch in this recording" : "ready")}</div>
        </>
      )}
    </section>
  );
}
