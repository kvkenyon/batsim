#!/usr/bin/env node
/**
 * Record a demo telemetry trace from a live batsim server.
 *
 * Usage (from web/):
 *   node tools/record-demo-trace.mjs
 *
 * Env knobs:
 *   BATSIM_URL   default http://127.0.0.1:18099
 *   OUT_DIR      default public/traces/demo
 *   HOME_COUNT   default 64
 *   TICKS        default 600   (recorded tick events, after downsampling)
 *   DOWNSAMPLE   default 1     (server sends one event per N ticks)
 *   SIM_SPEED    default 0     (0 = as fast as possible)
 *   TICK_SECONDS default 60    (scenario tick length in seconds)
 *   RUN_UNTIL    default 2025-06-15T13:00:00Z (advance the sim here
 *                  before opening the stream; empty string skips)
 *
 * Numeric payload fields are rounded (soc 4 decimals, kW 3, price 2)
 * to keep the trace within its size budget; structure is unchanged.
 *
 * The script starts `target/debug/batsim` itself unless the port is
 * already serving a healthy batsim, in which case it reuses the running
 * server and leaves it alive.
 */
import { spawn } from "node:child_process";
import { createWriteStream, mkdirSync, statSync, readFileSync, rmSync } from "node:fs";
import { mkdtempSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const WEB_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const REPO_ROOT = path.resolve(WEB_DIR, "..");

const BATSIM_URL = process.env.BATSIM_URL ?? "http://127.0.0.1:18099";
const OUT_DIR = path.resolve(WEB_DIR, process.env.OUT_DIR ?? "public/traces/demo");
const HOME_COUNT = Number(process.env.HOME_COUNT ?? 64);
const TICKS = Number(process.env.TICKS ?? 600);
const DOWNSAMPLE = Number(process.env.DOWNSAMPLE ?? 1);
const SIM_SPEED = Number(process.env.SIM_SPEED ?? 0);
const TICK_SECONDS = Number(process.env.TICK_SECONDS ?? 60);
const RUN_UNTIL = process.env.RUN_UNTIL ?? "2025-06-15T13:00:00Z";

const BATTERY_MODELS = [
  "tesla.powerwall_3",
  "tesla.powerwall_2",
  "enphase.iq_battery_5p",
  "solaredge.home_battery_400v",
  "sonnen.ecolinx",
];

function log(...args) {
  console.log("[record-trace]", ...args);
}

async function api(method, route, body) {
  const res = await fetch(`${BATSIM_URL}${route}`, {
    method,
    headers: body !== undefined ? { "content-type": "application/json" } : undefined,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  });
  const text = await res.text();
  let json = null;
  try {
    json = text ? JSON.parse(text) : null;
  } catch {
    // Non-JSON body (SSE etc.) is handled by callers that stream.
  }
  if (!res.ok) {
    throw new Error(`${method} ${route} -> ${res.status}: ${text.slice(0, 400)}`);
  }
  return json;
}

async function healthOk() {
  try {
    const res = await fetch(`${BATSIM_URL}/v1/system/health`);
    return res.status === 200;
  } catch {
    return false;
  }
}

async function waitHealthy(timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (await healthOk()) return true;
    await new Promise((r) => setTimeout(r, 250));
  }
  return false;
}

function startServer() {
  const bin = path.join(REPO_ROOT, "target", "debug", "batsim");
  const port = new URL(BATSIM_URL).port || "18099";
  const dataDir = mkdtempSync(path.join(tmpdir(), "batsim-trace-"));
  const child = spawn(bin, ["--port", String(port), "--data-dir", dataDir], {
    cwd: REPO_ROOT,
    stdio: ["ignore", "pipe", "pipe"],
  });
  child.stdout.on("data", () => {});
  child.stderr.on("data", (chunk) => {
    const line = chunk.toString().trim();
    if (line) log("server:", line.slice(0, 200));
  });
  return { child, dataDir };
}

function buildFleetManifest() {
  const fixture = JSON.parse(
    readFileSync(path.join(REPO_ROOT, "examples", "fleet-100.json"), "utf8"),
  );
  // Mix of every battery model in the demo registry set.
  const archetypes = [
    { weight: 0.30, model: "tesla.powerwall_3", count: 1, archetype: "sfh_family", pv: [5.0, 11.0] },
    { weight: 0.15, model: "tesla.powerwall_2", count: 1, archetype: "sfh_empty_nester", pv: [3.0, 8.0] },
    { weight: 0.25, model: "enphase.iq_battery_5p", count: 2, archetype: "townhome", pv: [4.0, 9.0] },
    { weight: 0.15, model: "solaredge.home_battery_400v", count: 1, archetype: "sfh_family", pv: [5.0, 10.0] },
    { weight: 0.15, model: "sonnen.ecolinx", count: 1, archetype: "apartment", pv: [2.0, 6.0] },
  ].map((a) => ({
    weight: a.weight,
    template: {
      battery: { model_id: a.model, count: a.count },
      pv: { peak_kw: { uniform: a.pv } },
      load: { archetype: a.archetype },
    },
  }));
  return {
    name: `demo-trace-${HOME_COUNT}`,
    seed: fixture.seed,
    archetypes,
    geo: {
      ercot_load_zones: {
        LZ_NORTH: 0.6,
        LZ_HOUSTON: 0.2,
        LZ_WEST: 0.1,
        LZ_SOUTH: 0.1,
      },
    },
    count: HOME_COUNT,
  };
}

function loadScenario() {
  const scenario = JSON.parse(
    readFileSync(path.join(REPO_ROOT, "examples", "scenario-day.json"), "utf8"),
  );
  scenario.time = { ...scenario.time, tick_seconds: TICK_SECONDS };
  return scenario;
}

function roundTo(value, decimals) {
  if (typeof value !== "number") return value;
  const factor = 10 ** decimals;
  return Math.round(value * factor) / factor;
}

function roundTickPayload(payload) {
  if (typeof payload.price_rtm === "number") {
    payload.price_rtm = roundTo(payload.price_rtm, 2);
  }
  if (Array.isArray(payload.homes)) {
    for (const row of payload.homes) {
      row.soc = roundTo(row.soc, 4);
      row.battery_power_kw = roundTo(row.battery_power_kw, 3);
      row.pv_power_kw = roundTo(row.pv_power_kw, 3);
      row.load_power_kw = roundTo(row.load_power_kw, 3);
      row.grid_power_kw = roundTo(row.grid_power_kw, 3);
    }
  }
  if (Array.isArray(payload.fleets)) {
    for (const row of payload.fleets) {
      row.battery_power_kw = roundTo(row.battery_power_kw, 3);
      row.pv_power_kw = roundTo(row.pv_power_kw, 3);
      row.load_power_kw = roundTo(row.load_power_kw, 3);
      row.grid_power_kw = roundTo(row.grid_power_kw, 3);
      row.soc_mean = roundTo(row.soc_mean, 4);
    }
  }
  return payload;
}

/**
 * Stream SSE from the telemetry endpoint and record TICKS tick events.
 * Writes {"event":"tick", ...payload} lines to `outStream`.
 */
async function recordTicks(outStream) {
  const res = await fetch(
    `${BATSIM_URL}/v1/telemetry/stream?fields=raw&downsample=${DOWNSAMPLE}`,
    { headers: { accept: "text/event-stream" } },
  );
  if (!res.ok || !res.body) {
    throw new Error(`telemetry stream -> ${res.status}`);
  }
  const reader = res.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  let recorded = 0;
  let firstTick = null;
  let lastTick = null;
  let firstSimTime = null;
  let lastSimTime = null;

  const handleEvent = (rawEvent) => {
    // One SSE event: `event:` names the kind, `data:` carries JSON.
    let kind = "message";
    let data = "";
    for (const line of rawEvent.split("\n")) {
      if (line.startsWith("event:")) {
        kind = line.slice(6).replace(/^ /, "").trim();
      } else if (line.startsWith("data:")) {
        data += (data ? "\n" : "") + line.slice(5).replace(/^ /, "");
      }
    }
    if (kind !== "tick" || !data) return false;
    let payload;
    try {
      payload = JSON.parse(data);
    } catch {
      return false;
    }
    if (!Number.isInteger(payload.tick)) return false;
    const line = { event: "tick", ...roundTickPayload(payload) };
    outStream.write(JSON.stringify(line) + "\n");
    recorded += 1;
    if (firstTick === null) {
      firstTick = payload.tick;
      firstSimTime = payload.sim_time;
    }
    lastTick = payload.tick;
    lastSimTime = payload.sim_time;
    if (recorded % 60 === 0) log(`recorded ${recorded}/${TICKS} tick events`);
    return recorded >= TICKS;
  };

  let done = false;
  while (!done) {
    const { value, done: streamDone } = await reader.read();
    if (streamDone) break;
    buffer += decoder.decode(value, { stream: true });
    let idx;
    while ((idx = buffer.indexOf("\n\n")) !== -1) {
      const rawEvent = buffer.slice(0, idx);
      buffer = buffer.slice(idx + 2);
      if (handleEvent(rawEvent)) {
        done = true;
        break;
      }
    }
  }
  try {
    await reader.cancel();
  } catch {
    // Connection teardown on an already-stopped server is fine.
  }
  if (recorded === 0) throw new Error("no tick events recorded");
  return { recorded, firstTick, lastTick, firstSimTime, lastSimTime };
}

async function fetchAllHomes() {
  const homes = [];
  let cursor = null;
  for (;;) {
    const q = new URLSearchParams({ limit: "1000" });
    if (cursor) q.set("cursor", cursor);
    const page = await api("GET", `/v1/homes?${q}`);
    homes.push(...page.data);
    cursor = page.page?.next_cursor ?? null;
    if (!cursor) break;
  }
  return homes;
}

async function main() {
  log(`target ${BATSIM_URL}, ${HOME_COUNT} homes, ${TICKS} ticks @ downsample ${DOWNSAMPLE}`);

  // 1. Ensure a server is listening.
  let server = null;
  if (await healthOk()) {
    log("reusing already-healthy batsim on", BATSIM_URL);
  } else {
    log("starting target/debug/batsim ...");
    server = startServer();
    const ok = await waitHealthy(60_000);
    if (!ok) {
      server.child.kill("SIGKILL");
      throw new Error("server did not become healthy within 60s");
    }
    log("server healthy");
  }

  const stopServer = () => {
    if (server) {
      server.child.kill("SIGTERM");
      rmSync(server.dataDir, { recursive: true, force: true });
    }
  };
  process.on("SIGINT", () => {
    stopServer();
    process.exit(130);
  });

  try {
    // 2. World setup: fleet, scenario, sim start.
    const version = await api("GET", "/v1/system/version");
    const fleet = await api("POST", "/v1/fleets", buildFleetManifest());
    log(`fleet ${fleet.id}: ${fleet.home_count} homes`);
    const scenario = await api("POST", "/v1/scenarios", loadScenario());
    await api("POST", `/v1/scenarios/${scenario.id}:activate`);
    log(`scenario ${scenario.id} active`);
    await api("PUT", "/v1/sim:speed", { multiplier: SIM_SPEED });
    await api("POST", "/v1/sim:start");
    if (RUN_UNTIL) {
      // run-until only operates on a paused simulation.
      await api("POST", "/v1/sim:pause");
      await api("POST", "/v1/sim:run-until", { until: RUN_UNTIL });
      const now = await api("GET", "/v1/sim:status");
      log(`advanced to ${now.sim_time} (tick ${now.tick}), opening stream`);
      await api("POST", "/v1/sim:resume");
    }

    // 3. Record the telemetry stream.
    mkdirSync(OUT_DIR, { recursive: true });
    const telemetryPath = path.join(OUT_DIR, "telemetry.jsonl");
    const outStream = createWriteStream(telemetryPath);
    const stats = await recordTicks(outStream);
    await new Promise((resolve) => outStream.end(resolve));
    log(`recorded ${stats.recorded} tick events (ticks ${stats.firstTick}..${stats.lastTick})`);

    // 4. Snapshot entities and write the trace files.
    const status = await api("GET", "/v1/sim:status");
    const homes = await fetchAllHomes();
    const fleetsPage = await api("GET", "/v1/fleets?limit=1000");
    const batteries = await api("GET", "/v1/registry/batteries");
    const batteryList = Array.isArray(batteries) ? batteries : (batteries.data ?? []);
    const usedModels = new Set(BATTERY_MODELS);
    for (const home of homes) {
      const model = home.config?.battery?.model_id;
      if (model) usedModels.add(model);
    }
    const batteryDetails = {};
    for (const model of usedModels) {
      batteryDetails[model] = await api(
        "GET",
        `/v1/registry/batteries/${encodeURIComponent(model)}`,
      );
    }

    const scenarioReq = loadScenario();
    const entities = {
      scenarioName: status.active_scenario ?? scenarioReq.name,
      homes,
      fleets: fleetsPage.data ?? [],
      batteries: batteryList,
      batteryDetails,
    };
    const entitiesPath = path.join(OUT_DIR, "entities.json");
    await import("node:fs/promises").then((fs) =>
      fs.writeFile(entitiesPath, JSON.stringify(entities)),
    );

    const manifest = {
      format: "batsim-trace/1",
      recorded_from: { batsim_version: version.version },
      scenario: {
        name: status.active_scenario ?? scenarioReq.name,
        seed: scenarioReq.seed ?? 0,
        tick_seconds: scenarioReq.time?.tick_seconds ?? 1,
      },
      tick_range: [stats.firstTick, stats.lastTick],
      sim_time_range: [stats.firstSimTime, stats.lastSimTime],
      homes: HOME_COUNT,
      events: stats.recorded,
    };
    const manifestPath = path.join(OUT_DIR, "manifest.json");
    await import("node:fs/promises").then((fs) =>
      fs.writeFile(manifestPath, JSON.stringify(manifest, null, 2) + "\n"),
    );

    // 5. Stop the sim.
    await api("POST", "/v1/sim:stop");

    // 6. Report.
    const size = (p) => statSync(p).size;
    const fmt = (n) => (n / (1024 * 1024)).toFixed(2) + " MiB";
    log("--- trace stats ---");
    log(`events recorded: ${stats.recorded}`);
    log(`tick range: ${stats.firstTick}..${stats.lastTick}`);
    log(`sim time range: ${stats.firstSimTime} .. ${stats.lastSimTime}`);
    log(`telemetry.jsonl: ${fmt(size(telemetryPath))}`);
    log(`entities.json:   ${fmt(size(entitiesPath))}`);
    log(`manifest.json:   ${size(manifestPath)} B`);
    log(`total:           ${fmt(size(telemetryPath) + size(entitiesPath) + size(manifestPath))}`);
  } finally {
    stopServer();
    if (server) log("stopped child server");
  }
}

main().catch((err) => {
  console.error("[record-trace] FAILED:", err.message ?? err);
  process.exit(1);
});
