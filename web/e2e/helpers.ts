import { expect, type Page } from "@playwright/test";

/** Boot the offline demo replay and wait for live ticks. */
export async function bootDemo(page: Page): Promise<void> {
  await page.goto("/?demo=1");
  await expect(page.locator(".top-bar")).toBeVisible({ timeout: 30_000 });
  await expect(page.locator(".demo-badge")).toHaveText("demo replay", { timeout: 30_000 });
  await expect
    .poll(async () => page.evaluate(() => window.__batsim?.store.getState().tick ?? 0), {
      timeout: 30_000,
    })
    .toBeGreaterThan(5);
}

/** True when the element's center is the topmost hit target (not occluded). */
export async function expectUnoccluded(page: Page, selector: string): Promise<void> {
  const locator = page.locator(selector).first();
  await expect(locator).toBeVisible();
  const reachable = await locator.evaluate((el) => {
    const rect = el.getBoundingClientRect();
    const hit = document.elementFromPoint(
      rect.left + rect.width / 2,
      rect.top + rect.height / 2,
    );
    return hit !== null && (hit === el || el.contains(hit));
  });
  expect(reachable, `${selector} is covered by another panel`).toBe(true);
}

/**
 * Screen point inside a zone with no home marker under it, probing
 * outward from the zone anchor. Null when the zone is fully carpeted.
 */
export async function emptyPointInZone(page: Page, zone: string): Promise<{ x: number; y: number; lng: number; lat: number } | null> {
  return page.evaluate((zoneId) => {
    const state = window.__batsim?.store.getState();
    const map = window.__batsimMap;
    const anchor = state?.zoneAnchors[zoneId];
    if (!anchor || !map) return null;
    const offsets = [
      [0, 0],
      [0.25, 0.15],
      [-0.25, 0.2],
      [0.2, -0.2],
      [-0.3, -0.25],
      [0.45, 0],
      [0, 0.45],
      [-0.45, 0.1],
      [0.6, 0.3],
      [-0.6, -0.4],
    ];
    for (const [dx, dy] of offsets) {
      const lng = anchor[0] + (dx ?? 0);
      const lat = anchor[1] + (dy ?? 0);
      const pt = map.project([lng, lat]);
      if (pt.x < 270 || pt.x > window.innerWidth - 330 || pt.y < 120 || pt.y > window.innerHeight - 110) {
        continue;
      }
      if (map.queryRenderedFeatures(pt, { layers: ["homes"] }).length === 0) {
        return { x: pt.x, y: pt.y, lng, lat };
      }
    }
    return null;
  }, zone);
}
