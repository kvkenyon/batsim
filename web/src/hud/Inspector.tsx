/**
 * Inspector panel: the live detail view for one selected home. Device
 * identity comes from the entity metadata; power, SOC, and reserve read
 * from the telemetry buffers at HUD cadence.
 */

import { useEffect, useState } from "react";
import { getRuntime } from "../state/runtime";
import { useAppStore, type HomeMeta } from "../state/store";
import { socColor, TOKENS } from "../tokens/tokens";

interface HomeLiveSnapshot {
  soc: number;
  batteryKw: number;
  pvKw: number;
  loadKw: number;
  gridKw: number;
}

function readSnapshot(homeId: string): HomeLiveSnapshot | null {
  const { live } = getRuntime();
  const slot = live.slotOf.get(homeId);
  if (slot === undefined) return null;
  return {
    soc: live.soc[slot] ?? 0,
    batteryKw: live.batteryKw[slot] ?? 0,
    pvKw: live.pvKw[slot] ?? 0,
    loadKw: live.loadKw[slot] ?? 0,
    gridKw: live.gridKw[slot] ?? 0,
  };
}

function formatKw(v: number): string {
  const sign = v < 0 ? "\u2212" : "+";
  return `${sign}${Math.abs(v).toFixed(2)} kW`;
}

function batteryFlowLabel(kw: number): string {
  if (kw > 0.05) return "discharging";
  if (kw < -0.05) return "charging";
  return "idle";
}

export function Inspector({ meta }: { meta: HomeMeta }) {
  const selectHome = useAppStore((s) => s.selectHome);
  const [snap, setSnap] = useState<HomeLiveSnapshot | null>(() => readSnapshot(meta.id));

  useEffect(() => {
    setSnap(readSnapshot(meta.id));
    const timer = setInterval(() => setSnap(readSnapshot(meta.id)), 500);
    return () => clearInterval(timer);
  }, [meta.id]);

  const socPct = (snap?.soc ?? 0) * 100;
  const reservePct = meta.reserveFloorFrac * 100;
  const storedKwh = (snap?.soc ?? 0) * meta.usableEnergyKwh * meta.batteryCount;

  return (
    <aside className="inspector hud-panel" aria-label="home inspector">
      <button className="close" onClick={() => selectHome(null)} aria-label="close inspector">
        ×
      </button>
      <h2>{meta.batteryDisplayName}</h2>
      <div className="sub">
        {meta.id} · {meta.zone} · {meta.archetype}
      </div>

      <div className="row">
        <span className="k">state of charge</span>
        <span className="v mono" style={{ color: socColor(snap?.soc ?? 0) }}>
          {socPct.toFixed(1)}%
        </span>
      </div>
      <div className="soc-bar">
        <div
          className="fill"
          style={{ width: `${socPct}%`, background: socColor(snap?.soc ?? 0) }}
        />
      </div>
      <div className="reserve-marker">
        <div className="tick" style={{ left: `${reservePct}%` }} title={`reserve floor ${reservePct}%`} />
      </div>

      <div className="row">
        <span className="k">battery ({batteryFlowLabel(snap?.batteryKw ?? 0)})</span>
        <span
          className="v mono"
          style={{
            color:
              (snap?.batteryKw ?? 0) > 0.05
                ? TOKENS.energyDischarge
                : (snap?.batteryKw ?? 0) < -0.05
                  ? TOKENS.energyCharge
                  : undefined,
          }}
        >
          {formatKw(snap?.batteryKw ?? 0)}
        </span>
      </div>
      <div className="row">
        <span className="k">pv output</span>
        <span className="v mono" style={{ color: TOKENS.energySolar }}>
          {(snap?.pvKw ?? 0).toFixed(2)} kW
        </span>
      </div>
      <div className="row">
        <span className="k">home load</span>
        <span className="v mono">{(snap?.loadKw ?? 0).toFixed(2)} kW</span>
      </div>
      <div className="row">
        <span className="k">grid {(snap?.gridKw ?? 0) >= 0 ? "import" : "export"}</span>
        <span
          className="v mono"
          style={{ color: (snap?.gridKw ?? 0) < 0 ? TOKENS.energyExport : undefined }}
        >
          {formatKw(snap?.gridKw ?? 0)}
        </span>
      </div>

      <div className="row">
        <span className="k">reserve floor</span>
        <span className="v mono" style={{ color: TOKENS.warnAmber }}>
          {reservePct.toFixed(0)}%
        </span>
      </div>
      <div className="row">
        <span className="k">stored energy</span>
        <span className="v mono">{storedKwh.toFixed(1)} kWh</span>
      </div>
      <div className="row">
        <span className="k">units</span>
        <span className="v mono">{meta.batteryCount}</span>
      </div>
      <div className="row">
        <span className="k">operating mode</span>
        <span className="v mono">{meta.mode}</span>
      </div>
      {meta.pvPeakKw !== null && (
        <div className="row">
          <span className="k">pv nameplate</span>
          <span className="v mono">{meta.pvPeakKw.toFixed(1)} kW</span>
        </div>
      )}
    </aside>
  );
}
