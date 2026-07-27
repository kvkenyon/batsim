import { expect, test, type Page } from "@playwright/test";
import type { Map as MaplibreMap } from "maplibre-gl";

declare global {
  interface Window {
    /** Set by the map view for ops/e2e driving. */
    __batsimMap?: MaplibreMap;
  }
}

/**
 * Visual acceptance captures for the console: the tiled map with zone
 * overlay and lensed markers, the neighborhood with live flow animation,
 * the events feed carrying a dispatch entry, and the inspect panel.
 * Screenshots land in test-results/visual/ for review.
 */

async function bootDemo(page: Page): Promise<void> {
  await page.goto("/?demo=1");
  await expect(page.locator(".top-bar")).toBeVisible({ timeout: 30_000 });
  await expect(page.locator(".demo-badge")).toHaveText("demo replay", { timeout: 30_000 });
  await expect
    .poll(async () => page.evaluate(() => window.__batsim?.store.getState().tick ?? 0), {
      timeout: 30_000,
    })
    .toBeGreaterThan(5);
}

test("tiled map renders zone overlay and lensed markers", async ({ page }) => {
  await bootDemo(page);

  // Real basemap tiles are loaded and rendered beneath the overlays.
  await expect
    .poll(async () =>
      page.evaluate(() => {
        const map = window.__batsimMap;
        if (!map || !map.getLayer("basemap")) return false;
        return map.isSourceLoaded("basemap") && map.queryRenderedFeatures().length > 0;
      }),
    )
    .toBe(true);

  // Zone polygons and home dots are on the map.
  const counts = await page.evaluate(() => {
    const map = window.__batsimMap;
    if (!map) return { zones: 0, homes: 0 };
    return {
      zones: map.queryRenderedFeatures({ layers: ["zones-fill"] }).length,
      homes: map.queryRenderedFeatures({ layers: ["homes"] }).length,
    };
  });
  expect(counts.zones).toBeGreaterThan(5);
  expect(counts.homes).toBeGreaterThan(10);

  // The SOC lens recolors markers per home (dots differ, not a flat wash).
  await page.locator(".seg button", { hasText: "soc" }).click();
  await page.waitForTimeout(1200);
  const distinct = await page.evaluate(() => {
    const map = window.__batsimMap;
    if (!map) return 0;
    const colors = new Set<string>();
    for (const f of map.querySourceFeatures("homes")) {
      const c: unknown = f.properties?.color;
      if (typeof c === "string") colors.add(c);
    }
    return colors.size;
  });
  expect(distinct).toBeGreaterThan(3);

  await page.screenshot({ path: "test-results/visual/a-map.png" });
});

test("neighborhood flow animation advances between frames", async ({ page }) => {
  await bootDemo(page);

  // Seek to the recorded fleet dispatch so service lines carry real
  // flow, zoom near the neighborhood so the dive chip appears, then dive.
  await page.locator(".dispatch-panel .cmd.discharge").click();
  await expect(page.locator(".events-feed")).toContainText("dispatch ack", { timeout: 30_000 });
  await page.evaluate(() => {
    const anchor = window.__batsim?.store.getState().neighborhoodAnchor;
    if (anchor) window.__batsimMap?.jumpTo({ center: anchor, zoom: 11 });
  });
  await page.waitForTimeout(800);
  await page.locator(".dive-chip").click();
  await expect
    .poll(async () => page.evaluate(() => window.__batsim?.store.getState().stratum))
    .toBe("neighborhood");
  await page.waitForTimeout(1500);

  const canvas = page.locator(".viewport-layer.active canvas").first();
  const frameA = await canvas.screenshot({ path: "test-results/visual/b-neighborhood-a.png" });
  await page.waitForTimeout(500);
  const frameB = await canvas.screenshot({ path: "test-results/visual/b-neighborhood-b.png" });
  // Flow particles and SOC arcs move; consecutive frames must differ.
  expect(Buffer.compare(frameA, frameB)).not.toBe(0);
});

test("events feed records the fleet dispatch entry", async ({ page }) => {
  await bootDemo(page);

  await page.locator(".dispatch-panel .cmd.discharge").click();
  const feed = page.locator(".events-feed");
  await expect(feed).toContainText("dispatch ack", { timeout: 30_000 });
  await expect(feed).toContainText("homes");
  // Severity tint class is present on the dispatch row.
  await expect(feed.locator(".event-row.kind-dispatch").first()).toBeVisible();
  await page.waitForTimeout(1500);
  await page.screenshot({ path: "test-results/visual/c-events-feed.png" });
});

test("inspect panel shows device detail, sparkline, and day totals", async ({ page }) => {
  await bootDemo(page);

  // Click a rendered home dot through the real pointer path.
  await expect
    .poll(async () =>
      page.evaluate(() => {
        const map = window.__batsimMap;
        if (!map || !map.getLayer("homes")) return false;
        return map.queryRenderedFeatures({ layers: ["homes"] }).length > 0;
      }),
    )
    .toBe(true);
  const dot = await page.evaluate(() => {
    const map = window.__batsimMap;
    if (!map) return null;
    for (const feature of map.queryRenderedFeatures({ layers: ["homes"] })) {
      if (feature.geometry.type !== "Point") continue;
      const [lng, lat] = feature.geometry.coordinates;
      const pt = map.project([lng ?? 0, lat ?? 0]);
      if (pt.x > 80 && pt.x < window.innerWidth - 320 && pt.y > 120 && pt.y < window.innerHeight - 120) {
        const id: unknown = feature.properties?.id;
        if (typeof id === "string") return { id, x: pt.x, y: pt.y };
      }
    }
    return null;
  });
  if (!dot) throw new Error("no rendered home dot found to click");
  await page.mouse.click(dot.x, dot.y);

  const inspector = page.locator(".inspector");
  await expect(inspector).toBeVisible();
  await expect(inspector).toContainText("state of charge");
  await expect(inspector).toContainText("reserve floor");
  await expect(inspector).toContainText("chemistry");
  await expect(inspector).toContainText("coupling");
  await expect(inspector).toContainText("charged");
  await expect(inspector).toContainText("pv generated");
  await expect(inspector).toContainText("grid import");
  // A per-home state chip is always present.
  await expect(inspector.locator(".state-chip").first()).toBeVisible();
  await page.waitForTimeout(2500);
  await page.screenshot({ path: "test-results/visual/d-inspect.png" });
});
