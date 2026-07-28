/**
 * Console transport controls. One facade over the two transports: the
 * live API (pause / resume / speed / run-until / fleet dispatch over
 * HTTP) and the recorded replay (the same gestures mapped onto the tape
 * deck). The HUD talks only to this module; it never branches on
 * connection kind itself.
 */

import { type BatsimApi } from "../api/client";
import { useAppStore } from "./store";
import { ReplayTransport } from "./transport";

export interface SimController {
  readonly kind: "live" | "replay";
  setPaused(paused: boolean): void;
  setSpeed(multiplier: number): void;
  /** Jump forward to the next noteworthy price moment. */
  jumpToNextPriceEvent(): void;
  /** Fire a fleet-wide charge or discharge command. */
  dispatchFleet(direction: "discharge" | "charge"): void;
}

/** Per-home setpoint for the fleet dispatch moment, kW. */
const FLEET_DISPATCH_KW = 5;
/** Hold duration for the fleet dispatch moment, seconds. */
const FLEET_DISPATCH_DURATION_S = 1800;

function commandId(): string {
  return `ui-${Date.now().toString(36)}-${Math.floor(Math.random() * 1e9).toString(36)}`;
}

export function createLiveController(api: BatsimApi, fleetId: string | null): SimController {
  const set = useAppStore.setState;
  return {
    kind: "live",
    setPaused(paused) {
      set({ paused });
      void (paused ? api.simPause() : api.simResume()).catch((err: unknown) => {
        set({ lastError: err instanceof Error ? err.message : String(err) });
      });
    },
    setSpeed(multiplier) {
      set({ speedMult: multiplier });
      void api.setSpeed(multiplier).catch((err: unknown) => {
        set({ lastError: err instanceof Error ? err.message : String(err) });
      });
    },
    jumpToNextPriceEvent() {
      // The API advances a paused sim to a wall time; the next 5-minute
      // settlement boundary is the next moment a price can change.
      void (async () => {
        try {
          const { sim_time } = await api.simStatus();
          const now = Date.parse(sim_time);
          const next = Math.ceil((now + 1) / 300_000) * 300_000;
          await api.simPause();
          await api.runUntil(new Date(next).toISOString());
          if (!useAppStore.getState().paused) await api.simResume();
        } catch (err) {
          set({ lastError: err instanceof Error ? err.message : String(err) });
        }
      })();
    },
    dispatchFleet(direction) {
      if (!fleetId) {
        set({ dispatchStatus: "no fleet to dispatch" });
        return;
      }
      set({ dispatchStatus: `${direction} ${FLEET_DISPATCH_KW} kW · sending…` });
      void api
        .dispatchFleet(fleetId, {
          command_id: commandId(),
          action: {
            type: direction === "discharge" ? "discharge_to" : "charge_to",
            kw: FLEET_DISPATCH_KW,
            duration_s: FLEET_DISPATCH_DURATION_S,
          },
        })
        .then((res) => {
          set({ dispatchStatus: `${direction} · ${res.targets} homes · command accepted` });
        })
        .catch((err: unknown) => {
          set({ dispatchStatus: err instanceof Error ? err.message : String(err) });
        });
    },
  };
}

export function createReplayController(transport: ReplayTransport): SimController {
  const set = useAppStore.setState;
  return {
    kind: "replay",
    setPaused(paused) {
      set({ paused });
      if (paused) transport.pause();
      else transport.resume();
    },
    setSpeed(multiplier) {
      set({ speedMult: multiplier });
      transport.setSpeed(multiplier);
    },
    jumpToNextPriceEvent() {
      if (!transport.jumpToNextPriceEvent()) {
        set({ dispatchStatus: "no price event ahead in the recording" });
      }
    },
    dispatchFleet(_direction) {
      // The recording already contains a real fleet dispatch with true
      // per-home execution latency; seek to it and watch the fleet move.
      if (transport.jumpToNextDispatch()) {
        set({
          paused: false,
          dispatchStatus: "replaying recorded fleet dispatch · watch the fleet respond",
        });
      } else {
        set({ dispatchStatus: "recording holds no fleet dispatch" });
      }
    },
  };
}

let controller: SimController | null = null;

export function setController(next: SimController): void {
  controller = next;
}

export function getController(): SimController {
  if (!controller) throw new Error("sim controller used before bootstrap completed");
  return controller;
}
