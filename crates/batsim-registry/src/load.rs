//! Registry loading: build-time embedded catalog plus optional external
//! shadow directory (spec §1.1).
//!
//! - [`Registry::embedded`] loads the catalog compiled into the binary.
//! - [`Registry::from_dir`] loads a catalog tree from disk.
//! - [`Registry::load`] layers an optional external directory over the
//!   embedded catalog; shadowing is entry-by-entry on `(kind, model_id)`
//!   and every shadow is logged (spec §1.1).
//!
//! Loading performs, in order: JSON parse into the typed schema targets
//! (structural validation: the serde types in [`crate::types`] mirror the
//! Part A schemas field-for-field, with `deny_unknown_fields`), semantic
//! validation ([`crate::validate`]: bounds, patterns, monotonic curves,
//! cross-references), and the §4.6 integrity check of `catalog.json`
//! content hashes. Any failure aborts with every violation enumerated.
//!
//! Note: the `jsonschema` crate was evaluated for validating against the
//! JSON schema documents in `registry/schemas/` directly, but its
//! dependency tree requires a newer toolchain than the pinned 1.83.0. The
//! typed-target + semantic-pass approach covers the same contract; the
//! schema documents remain the canonical external data contract.

use std::collections::BTreeMap;
use std::path::Path;

use crate::error::RegistryError;
use crate::types::{BatteryModel, CatalogManifest, ControllerModel, InverterModel, PvPreset};

/// Where the loaded catalog came from (recorded for the run manifest,
/// spec §1.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrySource {
    /// The build-time embedded catalog.
    Embedded,
    /// An external directory, with any embedded entries it shadowed.
    External {
        /// Directory the catalog was loaded from.
        dir: String,
        /// `(kind, model_id)` keys of embedded entries shadowed by the
        /// external directory (empty when no embedded layer was used).
        shadowed: Vec<String>,
    },
}

/// The loaded, validated, immutable device registry.
#[derive(Debug)]
pub struct Registry {
    manifest: CatalogManifest,
    batteries: BTreeMap<String, BatteryModel>,
    inverters: BTreeMap<String, InverterModel>,
    controllers: BTreeMap<String, ControllerModel>,
    pv_presets: BTreeMap<String, PvPreset>,
    source: RegistrySource,
}

impl Registry {
    /// Load the build-time embedded catalog. Panics-free: all errors are
    /// returned as [`RegistryError`].
    ///
    /// # Errors
    /// Returns parse, validation, or integrity errors describing every
    /// offending entry.
    pub fn embedded() -> Result<Self, RegistryError> {
        todo!("implemented by catalog task")
    }

    /// Load a catalog tree from a directory on disk (no embedded layer).
    ///
    /// # Errors
    /// As [`Registry::embedded`], plus I/O errors.
    pub fn from_dir(dir: &Path) -> Result<Self, RegistryError> {
        let _ = dir;
        todo!("implemented by catalog task")
    }

    /// Load the embedded catalog, then shadow entries from `dir` when it is
    /// `Some` (CLI `--registry-dir` / env `SIM_REGISTRY_DIR` / legacy
    /// `BATSIM_REGISTRY_DIR`; resolution happens in the caller). Every
    /// shadowed entry is logged via `tracing`.
    ///
    /// # Errors
    /// As [`Registry::embedded`].
    pub fn load(shadow_dir: Option<&Path>) -> Result<Self, RegistryError> {
        let _ = shadow_dir;
        todo!("implemented by catalog task")
    }

    /// The catalog manifest (version, entry index, integrity hash).
    #[must_use]
    pub fn manifest(&self) -> &CatalogManifest {
        &self.manifest
    }

    /// Where this registry was loaded from.
    #[must_use]
    pub fn source(&self) -> &RegistrySource {
        &self.source
    }

    /// Look up a battery model by `model_id`.
    #[must_use]
    pub fn battery(&self, model_id: &str) -> Option<&BatteryModel> {
        self.batteries.get(model_id)
    }

    /// Look up an inverter model by `model_id`.
    #[must_use]
    pub fn inverter(&self, model_id: &str) -> Option<&InverterModel> {
        self.inverters.get(model_id)
    }

    /// Look up a controller model by `model_id`.
    #[must_use]
    pub fn controller(&self, model_id: &str) -> Option<&ControllerModel> {
        self.controllers.get(model_id)
    }

    /// Look up a PV preset by `preset_id`.
    #[must_use]
    pub fn pv_preset(&self, preset_id: &str) -> Option<&PvPreset> {
        self.pv_presets.get(preset_id)
    }

    /// Iterate all battery models in sorted `model_id` order.
    pub fn batteries(&self) -> impl Iterator<Item = &BatteryModel> {
        self.batteries.values()
    }

    /// Iterate all inverter models in sorted `model_id` order.
    pub fn inverters(&self) -> impl Iterator<Item = &InverterModel> {
        self.inverters.values()
    }

    /// Iterate all controller models in sorted `model_id` order.
    pub fn controllers(&self) -> impl Iterator<Item = &ControllerModel> {
        self.controllers.values()
    }

    /// Iterate all PV presets in sorted `preset_id` order.
    pub fn pv_presets(&self) -> impl Iterator<Item = &PvPreset> {
        self.pv_presets.values()
    }
}
