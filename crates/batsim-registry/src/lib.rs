//! batsim-registry: OEM hardware registry.
//!
//! Device models (batteries, inverters, controllers, PV presets) are
//! declarative JSON catalog files under `registry/` at the workspace root,
//! embedded into the binary at build time. `Registry::load` layers an
//! external directory over the embedded catalog: entries shadow
//! one-for-one on `(kind, model_id)` and every shadow is logged.
//!
//! Design rules:
//! - The catalog is immutable after load; loading happens once at startup.
//! - Validation errors enumerate every broken entry, not just the first.
//! - Devices are data, not code: these types exist only as serde
//!   deserialization targets plus evaluation helpers for the simulation
//!   engine.

pub mod error;
pub mod load;
pub mod system;
pub mod types;
pub mod validate;

pub use error::RegistryError;
pub use load::Registry;
pub use system::{HomeSystem, SystemSpec};
pub use types::{
    AnnotatedNumber, BatteryModel, Chemistry, ControllerModel, Coupling, EfficiencyCurve,
    EfficiencyPoint, InverterModel, InverterTopology, Provenance, PvPreset, TemperatureRange,
    VendorApi, Warranty,
};
