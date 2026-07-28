/**
 * Design tokens as CSS custom properties plus the HUD shell styles. Color
 * values come from the shared token module; keep the two in sync by
 * editing tokens.ts and re-running the app, not by editing hexes here.
 */

import { TOKENS } from "../tokens/tokens";

export const cssVarOverrides = {
  "--bg-base": TOKENS.bgBase,
  "--bg-deep": TOKENS.bgDeep,
  "--surface": TOKENS.surface,
  "--surface-raised": TOKENS.surfaceRaised,
  "--surface-glass": "rgba(18, 22, 28, 0.78)",
  "--hairline": TOKENS.hairline,
  "--text-primary": TOKENS.textPrimary,
  "--text-secondary": TOKENS.textSecondary,
  "--text-dim": TOKENS.textDim,
  "--terrain-base": TOKENS.terrainBase,
  "--slate-line": TOKENS.slateLine,
  "--energy-discharge": TOKENS.energyDischarge,
  "--energy-charge": TOKENS.energyCharge,
  "--energy-export": TOKENS.energyExport,
  "--energy-solar": TOKENS.energySolar,
  "--alert": TOKENS.alert,
  "--alert-deep": TOKENS.alertDeep,
  "--warn-amber": TOKENS.warnAmber,
} as const;
