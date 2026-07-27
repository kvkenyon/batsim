/**
 * Bottom KPI strip: fleet power rollups, mean state of charge, and the
 * current real-time price. Numbers come from the throttled tick rollup in
 * the app store.
 */

import { useAppStore } from "../state/store";
import { priceColor, TOKENS } from "../tokens/tokens";

export function KpiStrip() {
  const fleet = useAppStore((s) => s.fleet);
  const priceRtm = useAppStore((s) => s.priceRtm);

  const fleetMw = Math.abs(fleet.batteryKw) / 1000;
  const socPct = fleet.socMean * 100;

  return (
    <footer className="kpi-strip">
      <span className="kpi">
        <span className="kpi-label">homes</span>
        <span className="kpi-value mono">{fleet.homes}</span>
      </span>
      <span className="kpi">
        <span className="kpi-label">fleet battery</span>
        <span className="kpi-value mono">{fleetMw.toFixed(2)} MW</span>
      </span>
      <span className="kpi">
        <span className="kpi-label">mean soc</span>
        <span className="kpi-value mono">{socPct.toFixed(1)}%</span>
      </span>
      <span className="kpi">
        <span className="kpi-label">pv</span>
        <span className="kpi-value mono" style={{ color: TOKENS.energySolar }}>
          {(fleet.pvKw / 1000).toFixed(2)} MW
        </span>
      </span>
      <span className="kpi">
        <span className="kpi-label">grid</span>
        <span className="kpi-value mono">{(fleet.gridKw / 1000).toFixed(2)} MW</span>
      </span>
      <span className="kpi">
        <span className="kpi-label">rtm price</span>
        <span className="kpi-value mono" style={{ color: priceColor(priceRtm) }}>
          ${priceRtm.toFixed(2)}/MWh
        </span>
      </span>
    </footer>
  );
}
