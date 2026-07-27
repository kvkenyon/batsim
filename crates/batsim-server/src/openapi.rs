//! OpenAPI document assembly.
//!
//! The document is generated from handler and schema annotations and is
//! the single source of truth for the API; it is served live at
//! `/openapi.json` and vendored at `api/openapi.json` (freshness is
//! CI-checked via `batsim --dump-openapi`).

use utoipa::OpenApi;

/// The generated OpenAPI 3.1 document.
#[derive(OpenApi)]
#[openapi(
    info(
        title = "batsim",
        version = "0.1.0",
        description = "Residential battery fleet simulator: physics-faithful virtual homes behind an OpenAPI-first HTTP API for dispatch-strategy testing (ERCOT)."
    ),
    paths(
        crate::routes::registry::list_batteries,
        crate::routes::registry::get_battery,
        crate::routes::registry::list_inverters,
        crate::routes::registry::get_inverter,
        crate::routes::registry::catalog_version,
        crate::routes::homes::create_home,
        crate::routes::homes::list_homes,
        crate::routes::homes::get_home,
        crate::routes::homes::patch_home,
        crate::routes::homes::delete_home,
        crate::routes::fleets::create_fleet,
        crate::routes::fleets::list_fleets,
        crate::routes::fleets::get_fleet,
        crate::routes::fleets::expand_fleet,
        crate::routes::fleets::delete_fleet,
        crate::routes::fleets::dispatch_fleet,
        crate::routes::scenarios::create_scenario,
        crate::routes::scenarios::list_scenarios,
        crate::routes::scenarios::get_scenario,
        crate::routes::scenarios::activate_scenario,
        crate::routes::scenarios::deactivate_scenario,
        crate::routes::sim::start,
        crate::routes::sim::pause,
        crate::routes::sim::resume,
        crate::routes::sim::stop,
        crate::routes::sim::step,
        crate::routes::sim::run_until,
        crate::routes::sim::set_speed,
        crate::routes::sim::status,
        crate::routes::dispatch::dispatch,
        crate::routes::dispatch::list_commands,
        crate::routes::dispatch::get_command,
        crate::routes::dispatch::cancel_command,
        crate::routes::telemetry::home_series,
        crate::routes::telemetry::fleet_series,
        crate::routes::telemetry::sse_stream,
        crate::routes::telemetry::ws_stream,
        crate::routes::system::health,
        crate::routes::system::version,
        crate::routes::system::config,
    ),
    components(schemas(
        crate::problem::Problem,
        crate::problem::ProblemCode,
        crate::model::PageInfo,
        crate::model::BatterySpec,
        crate::model::InverterSpec,
        crate::model::KwDraw,
        crate::model::PvSpec,
        crate::model::LoadSpec,
        crate::model::LocationSpec,
        crate::model::OperatingMode,
        crate::model::CreateHomeRequest,
        crate::model::HomeConfigDoc,
        crate::model::HomeStateDoc,
        crate::model::HomeDoc,
        crate::model::HomesPage,
        crate::model::PatchHomeRequest,
        crate::model::ArchetypeEntry,
        crate::model::HomeTemplate,
        crate::model::GeoSpec,
        crate::model::FleetManifest,
        crate::model::FleetDoc,
        crate::model::FleetsPage,
        crate::model::ExpandFleetRequest,
        crate::routes::fleets::FleetDispatchRequest,
        crate::model::ScenarioTime,
        crate::model::AmbientSpec,
        crate::model::WeatherSpec,
        crate::model::OutageSpec,
        crate::model::ScenarioRequest,
        crate::model::ScenarioDoc,
        crate::model::ScenariosPage,
        crate::price::PriceSourceSpec,
        crate::price::SyntheticProfile,
        crate::model::SimState,
        crate::model::SimStatusDoc,
        crate::model::StepRequest,
        crate::model::StepResponse,
        crate::model::RunUntilRequest,
        crate::model::SpeedRequest,
        crate::model::LatencySpec,
        crate::model::TargetFilter,
        crate::model::TargetSpec,
        crate::model::ActionSpec,
        crate::model::ExecutionSpec,
        crate::model::DispatchRequest,
        crate::model::CommandStatus,
        crate::model::DispatchResponse,
        crate::model::TargetExecution,
        crate::model::TargetStatus,
        crate::model::CommandDoc,
        crate::model::CommandsPage,
        crate::model::Resolution,
        crate::model::FleetAgg,
        crate::model::SeriesResponse,
        crate::model::BatterySummary,
        crate::model::InverterSummary,
        crate::model::BatteryList,
        crate::model::InverterList,
        crate::model::RegistryVersionDoc,
        crate::model::HealthDoc,
        crate::model::VersionDoc,
        crate::model::OkDoc,
    )),
    tags(
        (name = "registry", description = "Device catalog"),
        (name = "homes", description = "Simulated home CRUD"),
        (name = "fleets", description = "Bulk fleet composition"),
        (name = "scenarios", description = "Simulation run bindings"),
        (name = "sim", description = "Virtual time control"),
        (name = "dispatch", description = "Fleet control commands"),
        (name = "telemetry", description = "History and live streams"),
        (name = "system", description = "Server introspection"),
    )
)]
struct ApiDoc;

/// Build the OpenAPI document, patching in catalog-driven enums and
/// cross-field constraints that derives cannot express. Patching
/// happens on the serialized form, which is far less brittle than
/// constructing schema types by hand.
#[must_use]
pub fn build_openapi(registry: &batsim_registry::Registry) -> utoipa::openapi::OpenApi {
    let doc = ApiDoc::openapi();
    let mut value = serde_json::to_value(&doc).unwrap_or_default();
    let battery_ids: Vec<String> = registry
        .batteries()
        .map(|b| b.model_id.clone())
        .collect();
    let inverter_ids: Vec<String> = registry
        .inverters()
        .map(|i| i.model_id.clone())
        .collect();

    patch(
        &mut value,
        &[
            ("BatterySpec", "model_id", battery_ids.iter().map(String::as_str).collect()),
            ("InverterSpec", "model_id", inverter_ids.iter().map(String::as_str).collect()),
            ("LoadSpec", "archetype", crate::compose::ARCHETYPES.to_vec()),
            ("LocationSpec", "ercot_load_zone", crate::compose::LOAD_ZONES.to_vec()),
            (
                "LocationSpec",
                "climate_zone",
                ["2A", "3A", "3B", "4A", "gulf_coast", "central", "north", "west"].to_vec(),
            ),
        ],
    );
    if let Some(target) = value
        .pointer_mut("/components/schemas/TargetSpec")
    {
        *target = serde_json::json!({
            "description": "Dispatch target set: a fleet id, explicit home ids, or both.",
            "anyOf": [
                {
                    "type": "object",
                    "required": ["fleet_id"],
                    "properties": {
                        "fleet_id": {"type": ["string", "null"]},
                        "home_ids": {"type": ["array", "null"], "items": {"type": "string"}},
                        "filter": {"type": ["object", "null"]},
                        "sample_pct": {"type": ["number", "null"]}
                    }
                },
                {
                    "type": "object",
                    "required": ["home_ids"],
                    "properties": {
                        "fleet_id": {"type": ["string", "null"]},
                        "home_ids": {"type": ["array", "null"], "items": {"type": "string"}},
                        "filter": {"type": ["object", "null"]},
                        "sample_pct": {"type": ["number", "null"]}
                    }
                }
            ]
        });
    }
    if let Some(kw) = value.pointer_mut("/components/schemas/KwDraw") {
        *kw = serde_json::json!({
            "description": "A fixed value in kW, or a uniform range over (0, 100].",
            "anyOf": [
                {"type": "number", "exclusiveMinimum": 0, "maximum": 100},
                {
                    "type": "object",
                    "required": ["uniform"],
                    "properties": {
                        "uniform": {
                            "type": "array",
                            "minItems": 2,
                            "maxItems": 2,
                            "items": {"type": "number", "exclusiveMinimum": 0, "maximum": 100}
                        }
                    }
                }
            ]
        });
    }
    if let Some(zones) = value.pointer_mut("/components/schemas/GeoSpec/properties/ercot_load_zones") {
        zones["minProperties"] = serde_json::json!(1);
    }
    // Problem documents may carry extension members (RFC 9457).
    if let Some(problem) = value.pointer_mut("/components/schemas/Problem") {
        problem["additionalProperties"] = serde_json::json!(true);
    }
    // ScenarioDoc flattens the request; the request schema's
    // additionalProperties: false must not swallow the document fields.
    let req_schema = value.pointer("/components/schemas/ScenarioRequest").cloned();
    if let (Some(doc_schema), Some(req_schema)) = (
        value.pointer_mut("/components/schemas/ScenarioDoc"),
        req_schema,
    ) {
        let mut merged = req_schema.clone();
        if let Some(props) = merged.get_mut("properties").and_then(|p| p.as_object_mut()) {
            props.insert("id".to_owned(), serde_json::json!({"type": "string"}));
            props.insert(
                "created_at".to_owned(),
                serde_json::json!({"type": "string", "format": "date-time"}),
            );
            props.insert("active".to_owned(), serde_json::json!({"type": "boolean"}));
        }
        if let Some(required) = merged.get_mut("required").and_then(|r| r.as_array_mut()) {
            for f in ["id", "created_at", "active"] {
                required.push(serde_json::Value::String(f.to_owned()));
            }
        }
        merged["description"] =
            serde_json::json!("Scenario document: the binding plus identity and lifecycle.");
        *doc_schema = merged;
    }
    // utoipa renders u64 as signed int64, which mis-bounds every
    // unsigned field (seeds, ticks, timeouts). Rewrite them as unsigned.
    fix_unsigned_integers(&mut value);
    match serde_json::from_value(value) {
        Ok(doc) => doc,
        Err(e) => {
            debug_assert!(false, "patched OpenAPI must deserialize: {e}");
            ApiDoc::openapi()
        }
    }
}

/// Replace string fields with catalog enums, preserving descriptions
/// and optionality.
fn patch(value: &mut serde_json::Value, entries: &[(&str, &str, Vec<&str>)]) {
    for (component, field, values) in entries {
        let enum_values: Vec<serde_json::Value> = values
            .iter()
            .map(|v| serde_json::Value::String((*v).to_owned()))
            .collect();
        let path = format!("/components/schemas/{component}/properties/{field}");
        let Some(prop) = value.pointer_mut(&path) else {
            continue;
        };
        let description = prop
            .get("description")
            .and_then(|d| d.as_str())
            .map(str::to_owned);
        let required = prop
            .get("type")
            .and_then(|t| t.as_array())
            .is_none_or(|types| !types.iter().any(|t| t.as_str() == Some("null")));
        let mut patched = serde_json::json!({"type": "string", "enum": enum_values});
        if let Some(d) = description {
            patched["description"] = serde_json::Value::String(d);
        }
        *prop = if required {
            patched
        } else {
            serde_json::json!({"anyOf": [patched, {"type": "null"}]})
        };
    }
}

/// Recursively rewrite `{type: integer, format: int64}` schemas with
/// no explicit bounds as unsigned 64-bit.
fn fix_unsigned_integers(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            let integerish = match map.get("type") {
                Some(serde_json::Value::String(t)) => t == "integer",
                Some(serde_json::Value::Array(ts)) => ts.iter().any(|t| t.as_str() == Some("integer")),
                _ => false,
            };
            let is_u64_candidate = integerish
                && map.get("format").and_then(|f| f.as_str()) == Some("int64")
                && map.get("maximum").is_none();
            if is_u64_candidate {
                map.insert(
                    "maximum".to_owned(),
                    serde_json::Value::from(18_446_744_073_709_551_615u64),
                );
                if map.get("minimum").is_none() {
                    map.insert("minimum".to_owned(), serde_json::Value::from(0));
                }
                // `format: int64` pulls the signed 64-bit boundary back
                // in for tooling; the explicit bounds now carry it.
                map.remove("format");
            }
            for v in map.values_mut() {
                fix_unsigned_integers(v);
            }
        }
        serde_json::Value::Array(items) => {
            for v in items {
                fix_unsigned_integers(v);
            }
        }
        _ => {}
    }
}

/// Build the OpenAPI document.
#[must_use]
pub fn openapi_document(registry: &batsim_registry::Registry) -> utoipa::openapi::OpenApi {
    build_openapi(registry)
}

#[cfg(test)]
mod tests {
    use utoipa::OpenApi as _;

    #[test]
    fn locate_roundtrip_failure() {
        let doc = super::ApiDoc::openapi();
        let value = serde_json::to_value(&doc).unwrap();
        let text = serde_json::to_string(&value).unwrap();
        let mut de = serde_json::Deserializer::from_str(&text);
        let r: Result<utoipa::openapi::OpenApi, _> = serde_path_to_error::deserialize(&mut de);
        if let Err(e) = r {
            panic!("PATH: {} ERR: {e}", e.path());
        }
    }
}
