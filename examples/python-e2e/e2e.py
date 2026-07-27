"""End-to-end drive of a 100-home fleet with the generated client.

Creates a fleet, binds a scenario, accelerates time, dispatches the
fleet, and reads back telemetry. Run via examples/python-e2e/run.sh.
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path

import batsim_client
from batsim_client.models.fleet_manifest import FleetManifest
from batsim_client.models.scenario_request import ScenarioRequest
from batsim_client.models.dispatch_request import DispatchRequest
from batsim_client.models.step_request import StepRequest
from batsim_client.models.run_until_request import RunUntilRequest
from batsim_client.rest import ApiException

BASE_URL = os.environ.get("BATSIM_URL", "http://127.0.0.1:8080")
EXAMPLES = Path(__file__).resolve().parent.parent


def load_fixture(name: str) -> dict:
    return json.loads((EXAMPLES / name).read_text())


def main() -> int:
    cfg = batsim_client.Configuration(host=BASE_URL)
    api = batsim_client.ApiClient(cfg)
    fleets = batsim_client.FleetsApi(api)
    scenarios = batsim_client.ScenariosApi(api)
    sim = batsim_client.SimApi(api)
    dispatch = batsim_client.DispatchApi(api)
    telemetry = batsim_client.TelemetryApi(api)
    homes = batsim_client.HomesApi(api)

    # 1. Compose a 100-home fleet from the shipped manifest fixture.
    manifest = load_fixture("fleet-100.json")
    fleet = fleets.create_fleet(FleetManifest.from_dict(manifest))
    assert fleet.home_count == 100, fleet
    print(f"fleet {fleet.id}: {fleet.home_count} homes, hash {fleet.expansion_hash[:24]}")

    # Deterministic expansion: the same manifest yields the same hash.
    fleet2 = fleets.create_fleet(FleetManifest.from_dict(manifest))
    assert fleet2.expansion_hash == fleet.expansion_hash
    fleets.delete_fleet(fleet2.id)
    print("expansion is deterministic across fleets")

    # 2. Bind a scenario (time, prices, weather, seed) and activate it.
    scenario = scenarios.create_scenario(
        ScenarioRequest.from_dict(load_fixture("scenario-day.json"))
    )
    scenarios.activate_scenario(scenario.id)
    status = sim.status()
    assert status.sim_time == "2025-06-15T00:00:00Z", status
    print(f"scenario {scenario.id} active at {status.sim_time}")

    # 3. Advance six hours synchronously.
    sim.start()
    sim.pause()
    outcome = sim.step(StepRequest(ticks=6 * 3600))
    print(f"stepped {outcome.ticks_executed} ticks to {outcome.sim_time} in {outcome.wall_ms} ms")
    assert outcome.ticks_executed == 6 * 3600

    # 4. Dispatch: charge the fleet at 4 kW for an hour.
    cmd = dispatch.dispatch(
        DispatchRequest.from_dict(
            {
                "target": {"fleet_id": fleet.id},
                "action": {"type": "charge_to", "kw": 4.0, "duration_s": 3600},
                "execution": {"latency_ms": {"uniform": [100, 2000]}},
            }
        )
    )
    assert cmd.targets == 100, cmd
    # The response is accepted under an idempotency key; replaying the
    # same key returns the stored acceptance.
    # (Client libraries surface headers via with_http_info.)
    sim.step(StepRequest(ticks=10))
    record = dispatch.get_command(cmd.command_id)
    from collections import Counter
    breakdown = Counter(str(t.status) for t in record.targets)
    applied = sum(1 for t in record.targets if t.status == "applied")
    assert applied == 100, f"{applied}/100 applied, outcomes {dict(breakdown)}"
    print(f"dispatch {record.command_id}: all 100 applied, outcomes {dict(breakdown)}")

    # 5. Continue to midday and read telemetry back.
    sim.run_until(RunUntilRequest(until="2025-06-15T12:00:00Z"))
    series = telemetry.fleet_series(
        fleet.id, fields="battery_power_kw,load_power_kw,price_rtm", resolution="5m", agg="sum"
    )
    rows = len(series.t)
    assert rows > 100, rows
    prices = {v[2] for v in series.v}
    assert len(prices) > 1, "synthetic price should vary"
    print(f"fleet series: {rows} settlement buckets, price range "
          f"{min(v[2] for v in series.v):.0f}..{max(v[2] for v in series.v):.0f} $/MWh")

    home_page = homes.list_homes(fleet_id=fleet.id, limit=1)
    home_id = home_page.data[0].id
    home_series = telemetry.home_series(home_id, fields="soc,battery_power_kw", resolution="5m")
    assert len(home_series.t) > 100
    print(f"home {home_id[:18]}…: {len(home_series.t)} buckets, final SOC {home_series.v[-1][0]:.3f}")

    sim.stop()
    print("E2E OK")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except ApiException as e:
        print(f"API error {e.status}: {e.body}", file=sys.stderr)
        sys.exit(3)
