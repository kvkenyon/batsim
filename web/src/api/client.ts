/**
 * HTTP client for the batsim API, generated types over a thin fetch
 * wrapper. This module is the only file allowed to import openapi-fetch;
 * everything else consumes the facade functions below.
 */

import createClient from "openapi-fetch";
import type { components, paths } from "./gen/batsim";

export type HomeDoc = components["schemas"]["HomeDoc"];
export type FleetDoc = components["schemas"]["FleetDoc"];
export type BatterySummary = components["schemas"]["BatterySummary"];
export type SimStatusDoc = components["schemas"]["SimStatusDoc"];
export type DispatchResponse = components["schemas"]["DispatchResponse"];
export type FleetDispatchRequest = components["schemas"]["FleetDispatchRequest"];
export type Problem = components["schemas"]["Problem"];

export class ApiError extends Error {
  constructor(
    readonly status: number,
    readonly problem: Problem | null,
    message: string,
  ) {
    super(message);
    this.name = "ApiError";
  }
}

export interface BatsimApi {
  health: () => Promise<boolean>;
  listHomes: (limit?: number) => Promise<HomeDoc[]>;
  listFleets: () => Promise<FleetDoc[]>;
  listBatteries: () => Promise<BatterySummary[]>;
  /** Raw catalog entry for one battery model (untyped by the OpenAPI document). */
  batteryDetail: (modelId: string) => Promise<Record<string, unknown> | null>;
  simStatus: () => Promise<SimStatusDoc>;
  simPause: () => Promise<void>;
  simResume: () => Promise<void>;
  setSpeed: (multiplier: number) => Promise<void>;
  /** Advance a paused simulation to a target time synchronously. */
  runUntil: (untilIso: string) => Promise<void>;
  /** Send one command to every home in a fleet. */
  dispatchFleet: (fleetId: string, request: FleetDispatchRequest) => Promise<DispatchResponse>;
}

export function createBatsimApi(baseUrl: string): BatsimApi {
  const client = createClient<paths>({ baseUrl });

  return {
    async health() {
      try {
        const { response } = await client.GET("/v1/system/health");
        return response.ok;
      } catch {
        return false;
      }
    },

    async listHomes(limit = 1000) {
      const homes: HomeDoc[] = [];
      let cursor: string | undefined;
      for (;;) {
        const { data, error, response } = await client.GET("/v1/homes", {
          params: { query: cursor ? { limit, cursor } : { limit } },
        });
        if (error || !data) {
          throw new ApiError(response.status, (error as Problem) ?? null, "failed to list homes");
        }
        homes.push(...(data.data ?? []));
        const next = data.page?.next_cursor;
        if (!next || (data.data ?? []).length === 0) break;
        cursor = next;
      }
      return homes;
    },

    async listFleets() {
      const { data, error, response } = await client.GET("/v1/fleets", {
        params: { query: { limit: 500 } },
      });
      if (error || !data) {
        throw new ApiError(response.status, (error as Problem) ?? null, "failed to list fleets");
      }
      return data.data ?? [];
    },

    async listBatteries() {
      const { data, error, response } = await client.GET("/v1/registry/batteries");
      if (error || !data) {
        throw new ApiError(response.status, (error as Problem) ?? null, "failed to list batteries");
      }
      return data.data ?? [];
    },

    async batteryDetail(modelId: string) {
      const { data, response } = await client.GET("/v1/registry/batteries/{model_id}", {
        params: { path: { model_id: modelId } },
      });
      if (!response.ok) return null;
      return (data ?? null) as Record<string, unknown> | null;
    },

    async simStatus() {
      const { data, error, response } = await client.GET("/v1/sim:status");
      // The spec declares only a 200 response, so `error` is typed never;
      // a non-2xx still arrives at runtime and must carry its status.
      if (!response.ok) {
        throw new ApiError(response.status, error ?? null, "failed to read sim status");
      }
      if (!data) throw new ApiError(response.status, null, "sim status returned no body");
      return data;
    },

    async simPause() {
      const { response } = await client.POST("/v1/sim:pause");
      if (!response.ok) throw new ApiError(response.status, null, "failed to pause the simulation");
    },

    async simResume() {
      const { response } = await client.POST("/v1/sim:resume");
      if (!response.ok) throw new ApiError(response.status, null, "failed to resume the simulation");
    },

    async setSpeed(multiplier: number) {
      const { response } = await client.PUT("/v1/sim:speed", {
        body: { multiplier },
      });
      if (!response.ok) throw new ApiError(response.status, null, "failed to set sim speed");
    },

    async runUntil(untilIso: string) {
      const { response } = await client.POST("/v1/sim:run-until", {
        body: { until: untilIso },
      });
      if (!response.ok) throw new ApiError(response.status, null, "failed to advance the simulation");
    },

    async dispatchFleet(fleetId: string, request: FleetDispatchRequest) {
      const { data, error, response } = await client.POST("/v1/fleets/{id}:dispatch", {
        params: { path: { id: fleetId } },
        body: request,
      });
      if (!response.ok || !data) {
        throw new ApiError(response.status, (error as Problem) ?? null, "fleet dispatch rejected");
      }
      return data;
    },
  };
}
