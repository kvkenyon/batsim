/**
 * Inspector panel: the live detail view for one selected home. Device
 * identity (model, vendor, chemistry, coupling) comes from the entity
 * metadata; power, SOC, and reserve read from the telemetry buffers at
 * HUD cadence; the sparkline and day totals come from the per-home
 * history rings and energy accumulators fed by every tick.
 */

import { useEffect, useState } from "react";
import { SOC_HIST_CAP } from "../state/live";
import { getRuntime } from "../state/runtime";
import { useAppStore, type HomeMeta } from "../state/store";
import { socColor, TOKENS } from "../tokens/tokens";

interface HomeLiveSnapshot {
  soc: number;
  batteryKw: number;
  pvKw: number;
  loadKw: number;
  gridKw: number;
  chargedKwh: number;
  dischargedKwh: number;
  pvKwh: number;
  gridImportKwh: number;
  /** Oldest-to-newest SOC samples for the sparkline. */
  socHistory: number[];
}

function readSnapshot(homeId: string): HomeLiveSnapshot | null {
  const { live } = getRuntime();
  const slot = live.slotOf.get(homeId);
  if (slot === undefined) return null;
  const history: number[] = [];
  if (live.socHist.length > 0 && live.histLen > 1) {
    const base = slot * SOC_HIST_CAP;
    const samples = 120;
    const step = Math.max(1, Math.floor(live.histLen / samples));
    for (let i = 0; i < live.histLen; i += step) {
      const pos = (live.histPos - live.histLen + i + SOC_HIST_CAP * 2) % SOC_HIST_CAP;
      history.push(live.socHist[base + pos] ?? 0);
    }
  }
  return {
    soc: live.soc[slot] ?? 0,
    batteryKw: live.batteryKw[slot] ?? 0,
    pvKw: live.pvKw[slot] ?? 0,
    loadKw: live.loadKw[slot] ?? 0,
    gridKw: live.gridKw[slot] ?? 0,
    chargedKwh: live.chargedKwh[slot] ?? 0,
    dischargedKwh: live.dischargedKwh[slot] ?? 0,
    pvKwh: live.pvKwh[slot] ?? 0,
    gridImportKwh: live.gridImportKwh[slot] ?? 0,
    socHistory: history,
  };
}

function formatKw(v: number): string {
  const sign = v < 0 ? "−" : "+";
  return `${sign}${Math.abs(v).toFixed(2)} kW`;
}

/** Human-readable coupling topology (registry ids are enum-style). */
function formatCoupling(coupling: string): string {
  if (coupling === "DCCoupledHybrid") return "DC-coupled hybrid";
  if (coupling === "ACCoupled") return "AC-coupled";
  return coupling;
}

function batteryState(snap: HomeLiveSnapshot | null, reserveFloorFrac: number): { key: string; label: string } {
  const kw = snap?.batteryKw ?? 0;
  const soc = snap?.soc ?? 0;
  if (reserveFloorFrac > 0 && soc <= reserveFloorFrac + 0.005) return { key: "reserve", label: "reserve floor" };
  if (kw > 0.05) return { key: "discharging", label: "discharging" };
  if (kw < -0.05) return { key: "charging", label: "charging" };
  return { key: "idle", label: "idle" };
}

function Sparkline({ history, soc }: { history: number[]; soc: number }) {
  if (history.length < 2) return null;
  const w = 268;
  const h = 44;
  const step = w / (history.length - 1);
  const points = history
    .map((v, i) => `${(i * step).toFixed(1)},${(h - 3 - v * (h - 8)).toFixed(1)}`)
    .join(" ");
  return (
    <svg className="sparkline" viewBox={`0 0 ${w} ${h}`} preserveAspectRatio="none" aria-label="state of charge today">
      <line x1="0" y1={h - 3} x2={w} y2={h - 3} stroke="var(--hairline)" strokeWidth="1" />
      <polyline points={points} fill="none" stroke={socColor(soc)} strokeWidth="1.5" />
    </svg>
  );
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
  const state = batteryState(snap, meta.reserveFloorFrac);

  return (
    <aside className="inspector hud-panel" aria-label="home inspector">
      <button className="close" onClick={() => selectHome(null)} aria-label="close inspector">
        ×
      </button>
      <h2>
        {meta.batteryDisplayName}
        <span className={`state-chip ${state.key}`}>{state.label}</span>
      </h2>
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
      <Sparkline history={snap?.socHistory ?? []} soc={snap?.soc ?? 0} />

      <div className="row">
        <span className="k">battery ({state.label})</span>
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

      <div className="section">today</div>
      <div className="row">
        <span className="k">charged</span>
        <span className="v mono" style={{ color: TOKENS.energyCharge }}>
          {(snap?.chargedKwh ?? 0).toFixed(2)} kWh
        </span>
      </div>
      <div className="row">
        <span className="k">discharged</span>
        <span className="v mono" style={{ color: TOKENS.energyDischarge }}>
          {(snap?.dischargedKwh ?? 0).toFixed(2)} kWh
        </span>
      </div>
      <div className="row">
        <span className="k">pv generated</span>
        <span className="v mono" style={{ color: TOKENS.energySolar }}>
          {(snap?.pvKwh ?? 0).toFixed(2)} kWh
        </span>
      </div>
      <div className="row">
        <span className="k">grid import</span>
        <span className="v mono">{(snap?.gridImportKwh ?? 0).toFixed(2)} kWh</span>
      </div>

      <div className="section">system</div>
      <div className="row">
        <span className="k">model</span>
        <span className="v">{meta.batteryDisplayName}</span>
      </div>
      {meta.vendor && (
        <div className="row">
          <span className="k">vendor</span>
          <span className="v">{meta.vendor}</span>
        </div>
      )}
      {meta.chemistry && (
        <div className="row">
          <span className="k">chemistry</span>
          <span className="v">{meta.chemistry}</span>
        </div>
      )}
      {meta.coupling && (
        <div className="row">
          <span className="k">coupling</span>
          <span className="v">{formatCoupling(meta.coupling)}</span>
        </div>
      )}
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
