/**
 * Session runtime: the boot-created objects (telemetry buffers, transport)
 * that React components need outside the zustand store. Set once during
 * bootstrap, read by the map and neighborhood views every frame.
 */

import type { LiveBuffers } from "./live";
import type { TelemetryTransport } from "./transport";

export interface SessionRuntime {
  live: LiveBuffers;
  transport: TelemetryTransport;
}

let runtime: SessionRuntime | null = null;

export function setRuntime(next: SessionRuntime): void {
  runtime = next;
}

export function getRuntime(): SessionRuntime {
  if (!runtime) throw new Error("session runtime used before bootstrap completed");
  return runtime;
}
