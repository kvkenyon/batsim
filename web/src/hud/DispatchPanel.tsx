/**
 * Fleet dispatch panel: the watch-me moment. One click sends a real
 * fleet-wide charge or discharge command against the live API; in demo
 * replay it seeks the recording to the fleet dispatch captured in the
 * trace. Either way the fleet answers home by home with jittered
 * per-device latency, exactly like a real cloud fleet.
 */

import { getController } from "../state/controls";
import { useAppStore } from "../state/store";

export function DispatchPanel() {
  const connection = useAppStore((s) => s.connection);
  const status = useAppStore((s) => s.dispatchStatus);
  const canReplayDispatch = useAppStore((s) => s.canReplayDispatch);
  const homes = useAppStore((s) => s.fleet.homes);
  const isDemo = connection === "demo";
  const disabled = homes === 0 || (isDemo && !canReplayDispatch);

  return (
    <section className="dispatch-panel hud-panel" aria-label="fleet dispatch">
      <div className="head">fleet dispatch · {homes} homes</div>
      <div className="buttons">
        <button
          className="cmd discharge"
          disabled={disabled}
          onClick={() => getController().dispatchFleet("discharge")}
          title={isDemo ? "replay the recorded fleet discharge" : "discharge the fleet at 5 kW per home"}
        >
          ▼ discharge
        </button>
        <button
          className="cmd charge"
          disabled={disabled}
          onClick={() => getController().dispatchFleet("charge")}
          title={isDemo ? "replay the recorded fleet dispatch" : "charge the fleet at 5 kW per home"}
        >
          ▲ charge
        </button>
      </div>
      <div className="status t-num-s">{status ?? (disabled ? "no dispatch in this recording" : "ready")}</div>
    </section>
  );
}
