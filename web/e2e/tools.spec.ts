import { expect, test } from "@playwright/test";
import type { Map as MaplibreMap } from "maplibre-gl";
import { bootDemo, emptyPointInZone } from "./helpers";

declare global {
  interface Window {
    __batsimMap?: MaplibreMap;
  }
}

/**
 * Visual acceptance for the console's operator tools, each exercised in
 * the offline demo world: build mode places and removes a home, the
 * dispatch console counts one zone's acknowledgements, a fleet scenario
 * saves and reloads, and the time scrubber seeks the recorded day.
 * Screenshots land in test-results/visual/ for review.
 */

test("build mode places a home inside a zone and removes it", async ({ page }) => {
  await bootDemo(page);
  const homesAtBoot = await page.evaluate(() => window.__batsim?.store.getState().homeOrder.length ?? 0);

  // Arm placement with a Powerwall 2, then click empty ground in North.
  await page.locator(".build-panel .model-select").selectOption("tesla.powerwall_2");
  await page.locator(".build-panel .cmd.place").click();
  await expect(page.locator(".build-panel .cmd.place")).toContainText("cancel");
  const point = await emptyPointInZone(page, "LZ_NORTH");
  if (!point) throw new Error("no empty placement point in LZ_NORTH");
  await page.mouse.click(point.x, point.y);

  await expect
    .poll(async () => page.evaluate(() => window.__batsim?.store.getState().homeOrder.length ?? 0))
    .toBe(homesAtBoot + 1);
  await expect(page.locator(".build-panel .status")).toContainText("placed in LZ_NORTH");
  // The demo world is honest about persistence.
  await expect(page.locator(".build-panel .status")).toContainText("not persisted");
  await page.waitForTimeout(700);
  await page.screenshot({ path: "test-results/visual/f-build-place.png" });

  // Out-of-zone clicks are refused.
  const gulf = await page.evaluate(() => {
    const pt = window.__batsimMap?.project([-94.5, 28.2]);
    return pt ? { x: pt.x, y: pt.y } : null;
  });
  if (!gulf) throw new Error("map handle missing");
  await page.mouse.click(gulf.x, gulf.y);
  await expect(page.locator(".build-panel .status")).toContainText("outside ERCOT zones");
  await expect
    .poll(async () => page.evaluate(() => window.__batsim?.store.getState().homeOrder.length ?? 0))
    .toBe(homesAtBoot + 1);
  await page.keyboard.press("Escape");

  // Remove the placed home through the inspector's two-step affordance.
  const marker = await page.evaluate(
    ({ lng, lat }) => {
      const pt = window.__batsimMap?.project([lng, lat]);
      return pt ? { x: pt.x, y: pt.y } : null;
    },
    { lng: point.lng, lat: point.lat },
  );
  if (!marker) throw new Error("placed marker missing");
  await page.mouse.click(marker.x, marker.y);
  await expect(page.locator(".inspector")).toBeVisible();
  await page.locator(".inspector .cmd.remove").click();
  await page.locator(".inspector .remove-confirm .cmd.remove").click();
  await expect
    .poll(async () => page.evaluate(() => window.__batsim?.store.getState().homeOrder.length ?? 0))
    .toBe(homesAtBoot);
  await expect(page.locator(".build-panel .status")).toContainText("removed");
});

test("zone dispatch counts acknowledgements as the zone responds", async ({ page }) => {
  await bootDemo(page);

  await page.locator(".dispatch-panel .zone-select").selectOption("LZ_HOUSTON");
  const expected = await page.evaluate(() => {
    const state = window.__batsim?.store.getState();
    if (!state) return 0;
    return state.homeOrder.filter((id) => state.homesMeta[id]?.zone === "LZ_HOUSTON").length;
  });
  expect(expected).toBeGreaterThan(0);

  await page.locator(".dispatch-panel .cmd.discharge").click();
  // The rollup appears immediately and climbs to the zone's size as the
  // recorded per-home responses play out.
  await expect(page.locator(".dispatch-panel .status")).toContainText(`0/${expected}`);
  await expect(page.locator(".events-feed")).toContainText("dispatch ack", { timeout: 30_000 });
  await expect
    .poll(
      async () =>
        page.evaluate(() => {
          const ack = window.__batsim?.store.getState().zoneAck;
          return ack && ack.done ? ack.acked : -1;
        }),
      { timeout: 30_000 },
    )
    .toBe(expected);
  await expect(page.locator(".dispatch-panel .status")).toContainText(`${expected}/${expected}`);
  await expect(page.locator(".dispatch-panel .status")).toContainText("complete");
  await page.waitForTimeout(800);
  await page.screenshot({ path: "test-results/visual/g-zone-dispatch.png" });
});

test("scenario save and load round-trips the fleet", async ({ page }) => {
  await bootDemo(page);
  const homesAtBoot = await page.evaluate(() => window.__batsim?.store.getState().homeOrder.length ?? 0);

  await page.locator(".scenarios-panel .name-input").fill("spec-fleet");
  await page.locator(".scenarios-panel .cmd.save").click();
  await expect(page.locator(".scenarios-panel .scenario-row")).toHaveCount(1);
  await expect(page.locator(".scenarios-panel .scenario-row")).toContainText(`${homesAtBoot} homes`);

  // Change the world, then load: the snapshot wins.
  await page.locator(".build-panel .cmd.place").click();
  const point = await emptyPointInZone(page, "LZ_WEST");
  if (!point) throw new Error("no empty placement point in LZ_WEST");
  await page.mouse.click(point.x, point.y);
  await expect
    .poll(async () => page.evaluate(() => window.__batsim?.store.getState().homeOrder.length ?? 0))
    .toBe(homesAtBoot + 1);
  await page.keyboard.press("Escape");

  await page.locator(".scenarios-panel .cmd.load").click();
  await expect(page.locator(".build-panel .status")).toContainText("scenario loaded");
  await expect
    .poll(async () => page.evaluate(() => window.__batsim?.store.getState().homeOrder.length ?? 0))
    .toBe(homesAtBoot);
  await page.waitForTimeout(700);
  await page.screenshot({ path: "test-results/visual/h-scenario-load.png" });

  // Delete leaves the browser clean.
  await page.locator(".scenarios-panel .cmd.delete").click();
  await expect(page.locator(".scenarios-panel .scenario-row")).toHaveCount(0);
  await expect(page.locator(".scenarios-panel .status")).toContainText("no saved scenarios");
});

test("time scrubber seeks the recorded day", async ({ page }) => {
  await bootDemo(page);

  const range = await page.evaluate(() => window.__batsim?.store.getState().traceRangeMs ?? null);
  if (!range) throw new Error("demo boot did not publish trace bounds");
  const [start, end] = range as [number, number];

  // Drag to 85% of the tape: deep evening in the recording. Range
  // inputs are not fillable; set the value and fire input, exactly what
  // a real drag dispatches.
  const target = Math.round(start + (end - start) * 0.85);
  await page.evaluate((ms) => {
    const slider = document.querySelector<HTMLInputElement>(".scrubber-bar .scrub");
    if (!slider) return;
    const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, "value")?.set;
    setter?.call(slider, String(ms));
    slider.dispatchEvent(new Event("input", { bubbles: true }));
  }, target);
  await expect
    .poll(async () => page.evaluate(() => window.__batsim?.store.getState().simTimeMs ?? 0), {
      timeout: 15_000,
    })
    .toBeGreaterThanOrEqual(target - 120_000);
  const position = await page.locator(".scrubber-bar .position").textContent();
  expect(position).toMatch(/\d{2}:\d{2} CT/);
  // The slider thumb follows the scrub position.
  const sliderValue = await page.locator(".scrubber-bar .scrub").inputValue();
  expect(Math.abs(Number(sliderValue) - target)).toBeLessThan(300_000);
  await page.waitForTimeout(1200);
  await page.screenshot({ path: "test-results/visual/i-scrub-evening.png" });
});
