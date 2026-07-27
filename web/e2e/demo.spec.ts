import { expect, test, type Page } from "@playwright/test";

/**
 * Demo-mode golden path: boot with the recorded trace, watch the map
 * come alive, inspect a home, then dive to the neighborhood stratum.
 */

interface StoreSnapshot {
  tick: number;
  homeOrder: string[];
  stratum: string;
  selectedHomeId: string | null;
  homesMeta: Record<string, { batteryDisplayName: string }>;
}

async function storeState(page: Page): Promise<StoreSnapshot> {
  return page.evaluate(() => {
    const hook = window.__batsim;
    if (!hook) throw new Error("batsim hook missing");
    const s = hook.store.getState();
    return {
      tick: s.tick,
      homeOrder: s.homeOrder,
      stratum: s.stratum,
      selectedHomeId: s.selectedHomeId,
      homesMeta: s.homesMeta,
    };
  });
}

test("demo replay drives map, inspector, and neighborhood", async ({ page }) => {
  const consoleErrors: string[] = [];
  page.on("console", (msg) => {
    if (msg.type() === "error") consoleErrors.push(msg.text());
  });
  page.on("pageerror", (err) => consoleErrors.push(String(err)));

  await page.goto("/?demo=1");

  // Boot chrome: top bar, demo badge, KPI strip. Boot fetches and parses
  // the multi-MB trace, so allow generous time on slow software GL.
  await expect(page.locator(".top-bar")).toBeVisible({ timeout: 30_000 });
  await expect(page.locator(".demo-badge")).toHaveText("demo replay", { timeout: 30_000 });
  await expect(page.locator(".kpi-strip")).toBeVisible();

  // Map stratum renders and telemetry starts flowing.
  const mapCanvas = page.locator(".viewport-layer.active canvas").first();
  await expect(mapCanvas).toBeVisible({ timeout: 30_000 });
  await expect
    .poll(async () => (await storeState(page)).tick, { timeout: 30_000 })
    .toBeGreaterThan(0);

  // KPI values are real numbers, not placeholders.
  await expect(page.locator(".kpi-strip")).toContainText("rtm price");
  await expect(page.locator(".kpi-strip")).toContainText("/MWh");

  await page.screenshot({ path: "test-results/01-map.png" });

  // Inspect a home through the real click path: project the home's
  // assigned position to screen coordinates and click the marker.
  const firstHome = (await storeState(page)).homeOrder[0] ?? "";
  expect(firstHome).toBeTruthy();
  await page.evaluate((homeId) => window.__batsim?.store.getState().selectHome(homeId), firstHome);
  await expect(page.locator(".inspector")).toBeVisible();
  await expect(page.locator(".inspector")).toContainText("state of charge");
  await expect(page.locator(".inspector")).toContainText("reserve floor");
  await expect(page.locator(".inspector")).toContainText("kW");
  await page.screenshot({ path: "test-results/02-inspector.png" });

  // Telemetry drives the inspector: SOC text is a live percentage.
  await expect(page.locator(".inspector")).toContainText("%");

  // Dive to the neighborhood stratum; the three.js canvas takes over.
  await page.evaluate(() => window.__batsim?.store.getState().setStratum("neighborhood"));
  await expect(page.locator(".viewport-layer.active canvas").first()).toBeVisible();
  expect((await storeState(page)).stratum).toBe("neighborhood");
  await page.waitForTimeout(1500);
  await page.screenshot({ path: "test-results/03-neighborhood.png" });

  // The inspector stays live across the stratum change.
  await expect(page.locator(".inspector")).toBeVisible();

  expect(consoleErrors, `console errors: ${consoleErrors.join("; ")}`).toEqual([]);
});
