//! HomeSystem composition: the declarative system document, per-vendor
//! validation rules, and the resolved [`SystemSpec`] the engine consumes
//! at simulation-init.
//!
//! Split of responsibilities: this module validates and computes; it never
//! constructs engine types. batsim-core turns a [`SystemSpec`] into live
//! `Home` state.

use serde::{Deserialize, Serialize};

use crate::error::{RegistryError, Violation};
use crate::load::Registry;
use crate::types::{BatteryModel, ControllerModel, Coupling, InverterModel, InverterTopology};

/// A battery line item in a HomeSystem document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatteryRef {
    /// Battery `model_id` in the registry.
    pub model_id: String,
    /// Number of units.
    pub quantity: u32,
    /// Expansion packs per head unit (PW3; 0 for all other models).
    #[serde(default)]
    pub expansion_packs_per_unit: u32,
    /// Initial SOC fraction of usable energy.
    #[serde(default = "default_initial_soc")]
    pub initial_soc_frac: f64,
    /// User backup reserve floor.
    #[serde(default = "default_reserve_frac")]
    pub reserve_frac: f64,
}

const fn default_initial_soc() -> f64 {
    0.5
}

const fn default_reserve_frac() -> f64 {
    0.2
}

/// An inverter line item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InverterRef {
    /// Inverter `model_id` in the registry.
    pub model_id: String,
    /// Number of units.
    pub quantity: u32,
}

/// A controller line item.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControllerRef {
    /// Controller `model_id` in the registry.
    pub model_id: String,
    /// Number of units.
    pub quantity: u32,
}

/// Array orientation: named compass point or explicit azimuth degrees.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Orientation {
    /// Named orientation.
    Named(NamedOrientation),
    /// Azimuth degrees, 0..359 (180 = south).
    Azimuth(u32),
}

/// Named array orientations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NamedOrientation {
    /// North.
    N,
    /// North-east.
    NE,
    /// East.
    E,
    /// South-east.
    SE,
    /// South.
    S,
    /// South-south-west.
    SSW,
    /// South-west.
    SW,
    /// West-south-west.
    WSW,
    /// West.
    W,
    /// North-west.
    NW,
    /// Flat (horizontal).
    FLAT,
}

impl Orientation {
    /// Resolve to azimuth degrees (180 = south, 90 = east, 270 = west).
    /// `FLAT` resolves to 180 (tilt carries the flatness).
    #[must_use]
    pub const fn azimuth_deg(self) -> u32 {
        match self {
            Self::Azimuth(a) => a,
            Self::Named(n) => match n {
                NamedOrientation::N => 0,
                NamedOrientation::NE => 45,
                NamedOrientation::E => 90,
                NamedOrientation::SE => 135,
                NamedOrientation::S | NamedOrientation::FLAT => 180,
                NamedOrientation::SSW => 202,
                NamedOrientation::SW => 225,
                NamedOrientation::WSW => 247,
                NamedOrientation::W => 270,
                NamedOrientation::NW => 315,
            },
        }
    }
}

/// PV array configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PvConfig {
    /// Array DC nameplate.
    pub kw_dc: f64,
    /// Array orientation.
    pub orientation: Orientation,
    /// Tilt in degrees.
    #[serde(default = "default_tilt")]
    pub tilt_deg: f64,
    /// DC/AC ratio.
    #[serde(default = "default_dc_ac_ratio")]
    pub dc_ac_ratio: f64,
    /// PV inverter `model_id`; `None` iff PV lands on a hybrid inverter's
    /// MPPTs.
    #[serde(default)]
    pub pv_inverter_model_id: Option<String>,
}

const fn default_tilt() -> f64 {
    25.0
}

const fn default_dc_ac_ratio() -> f64 {
    1.2
}

/// Main service panel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MainPanel {
    /// Service rating in amps (240 V split-phase assumed).
    #[serde(default = "default_service_rating")]
    pub service_rating_a: f64,
    /// Utility-imposed export cap; `None` = none.
    #[serde(default)]
    pub interconnection_limit_kw: Option<f64>,
}

const fn default_service_rating() -> f64 {
    200.0
}

/// Backup sub-panel declaration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackupPanel {
    /// Peak critical-load power.
    #[serde(default = "default_critical_peak")]
    pub critical_loads_peak_kw: f64,
    /// Whole-home backup topology (no critical-loads split).
    #[serde(default)]
    pub whole_home: bool,
}

const fn default_critical_peak() -> f64 {
    5.0
}

/// Generator input declaration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratorConfig {
    /// Rated power.
    pub rated_kw: f64,
    /// Auto-transfer-switch flag.
    #[serde(default = "default_true")]
    pub auto_start: bool,
}

const fn default_true() -> bool {
    true
}

/// EV charger declaration: load-only (V1G); V2X is out of scope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvChargerConfig {
    /// Rated charge power (default 11.5 kW L2).
    #[serde(default = "default_ev_kw")]
    pub rated_kw: f64,
    /// V1G controllable load.
    #[serde(default = "default_true")]
    pub controllable: bool,
    /// Whether the charger sits on the backup sub-panel.
    #[serde(default)]
    pub on_backup_panel: bool,
}

const fn default_ev_kw() -> f64 {
    11.5
}

/// Grid meter point: the ERCOT ESIID binding, consumed by the planned
/// market-dispatch layer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GridMeter {
    /// ERCOT ESI ID.
    pub esiid: String,
    /// TDSP name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tdsp: Option<String>,
}

/// The HomeSystem composition document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HomeSystem {
    /// Schema version; must equal [`crate::types::SCHEMA_VERSION`].
    pub schema_version: String,
    /// System UUID (assigned by the planned HTTP API layer; any string
    /// accepted here).
    pub system_id: String,
    /// Human label.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Battery line items.
    pub batteries: Vec<BatteryRef>,
    /// Inverter line items.
    pub inverters: Vec<InverterRef>,
    /// Controller line items.
    #[serde(default)]
    pub controllers: Vec<ControllerRef>,
    /// PV array, if any.
    #[serde(default)]
    pub pv: Option<PvConfig>,
    /// Main service panel.
    pub main_panel: MainPanel,
    /// Whether the system asserts backup capability.
    pub backup_capable: bool,
    /// Backup sub-panel, if any.
    #[serde(default)]
    pub backup_panel: Option<BackupPanel>,
    /// Generator input, if any.
    #[serde(default)]
    pub generator: Option<GeneratorConfig>,
    /// EV chargers.
    #[serde(default)]
    pub ev_chargers: Vec<EvChargerConfig>,
    /// Grid meter point.
    pub grid_meter: GridMeter,
}

/// A resolved, validated system: the composition-time output consumed by
/// batsim-core at simulation-init, per the composition rules.
///
/// Beyond the four `total_*` aggregates the composition rules require,
/// this struct
/// carries resolved composition facts that batsim-core would otherwise have
/// to re-derive: the grid-forming controller identity and the storage
/// coupling classification.
#[derive(Debug, Clone)]
pub struct SystemSpec {
    /// The (validated) source document.
    pub system: HomeSystem,
    /// Total usable battery energy across all units and packs, kWh:
    /// `Σ quantity × (head usable + packs-per-unit × pack usable)`.
    pub total_usable_energy_kwh: f64,
    /// Total continuous discharge power at device boundaries, kW.
    /// Expansion packs add energy only, never power.
    pub total_discharge_power_kw: f64,
    /// Total continuous charge power, kW.
    pub total_charge_power_kw: f64,
    /// Computed backup-path continuous power: the minimum of every
    /// series stage of the backup path: total battery continuous discharge, the
    /// sum of (`max_ac_output_kw_backup` else `rated_ac_output_kw`) over
    /// present explicit inverters (when any are present), and Σ
    /// `max_backup_power_kw` over present controllers that declare it.
    /// With no explicit inverter and no controller cap the battery sum
    /// stands alone (integrated-inverter batteries such as PW3), making
    /// the `min` an identity. `None` when not backup-capable.
    pub backup_path_power_kw: Option<f64>,
    /// `model_id` of the single grid-forming controller resolved during
    /// validation under the backup rule. `None` when not backup-capable.
    pub resolved_controller_model_id: Option<String>,
    /// Whether any present battery is DC-coupled hybrid. The simulation
    /// engine uses this to select the single-inversion PV-storage loss path.
    pub has_dc_coupled_storage: bool,
}

impl HomeSystem {
    /// Validate this composition against the registry per the composition
    /// rules and the cross-reference checks; on success return the resolved
    /// [`SystemSpec`]. All violations are enumerated, never fail-fast.
    ///
    /// Enforced rules:
    /// - `schema_version` equals [`crate::types::SCHEMA_VERSION`].
    /// - Every referenced `model_id` resolves to a registry entry of
    ///   matching kind (batteries, inverters, controllers, PV inverter).
    /// - Quantities are >= 1; SOC/reserve fractions lie in `[0, 1]`;
    ///   each battery's initial SOC lies inside its model's SOC window.
    /// - `backup_capable` requires either exactly one present controller
    ///   entry with `provides_grid_forming`, or, with no such controller,
    ///   a self-forming fleet: at least one present battery, every one
    ///   grid-forming in backup with an integrated inverter. Every
    ///   battery's `requires_controller_id` (when declared) must be
    ///   present in `controllers[]`.
    /// - DC-coupled hybrid batteries without an integrated inverter
    ///   intersect some present inverter's `compatible_battery_ids`.
    /// - Per battery model, unit count <= Σ `max_batteries × quantity`
    ///   over present compatible inverters when every such inverter
    ///   declares `max_batteries` (SolarEdge: 3 × Home Hub count).
    /// - Expansion packs only on models declaring
    ///   `expansion_pack_model_id`; packs <= `max_units_per_inverter - 1`
    ///   (PW3: 3); `packs_add_power = false` (energy only).
    /// - Microinverter-based batteries (Enphase): continuous ratings must
    ///   not exceed `microinverter_count × power_per_microinverter_kw`
    ///   (0.64 kW per IQ8D is a ceiling, exact for the 5P, loose for the
    ///   IQ 10/10C), and peak >= continuous discharge.
    /// - A generator requires a present controller with
    ///   `supports_generator_input`.
    /// - A null `pv_inverter_model_id` requires a present hybrid landing
    ///   pad (hybrid inverter MPPTs or an integrated-inverter DC-coupled
    ///   battery such as PW3).
    ///
    /// # Errors
    /// [`RegistryError::Validation`] enumerating every violation.
    pub fn validate(&self, registry: &Registry) -> Result<SystemSpec, RegistryError> {
        let mut violations: Vec<Violation> = Vec::new();

        let expected = crate::types::SCHEMA_VERSION;
        let found = &self.schema_version;
        if found != expected {
            violations.push(violation(
                "schema_version",
                format!("unsupported schema_version `{found}`; expected `{expected}`"),
            ));
        }

        let battery_models = self.resolve_batteries(registry, &mut violations);
        let inverter_models = self.resolve_inverters(registry, &mut violations);
        let controller_models = self.resolve_controllers(registry, &mut violations);

        // Present = referenced with quantity >= 1 and resolved in the registry.
        let present_inverters: Vec<(&InverterModel, u32)> = self
            .inverters
            .iter()
            .zip(&inverter_models)
            .filter(|(r, _)| r.quantity > 0)
            .filter_map(|(r, m)| m.map(|m| (m, r.quantity)))
            .collect();
        let present_controllers: Vec<(&ControllerModel, u32)> = self
            .controllers
            .iter()
            .zip(&controller_models)
            .filter(|(r, _)| r.quantity > 0)
            .filter_map(|(r, m)| m.map(|m| (m, r.quantity)))
            .collect();

        self.check_backup(&battery_models, &present_controllers, &mut violations);
        self.check_dc_coupling(&battery_models, &present_inverters, &mut violations);
        self.check_inverter_capacity(&battery_models, &present_inverters, &mut violations);
        self.check_expansion_packs(registry, &battery_models, &mut violations);
        self.check_microinverter_power(&battery_models, &mut violations);
        self.check_generator(&present_controllers, &mut violations);
        self.check_pv(
            registry,
            &battery_models,
            &present_inverters,
            &mut violations,
        );
        self.check_free_parameters(&mut violations);

        if !violations.is_empty() {
            return Err(RegistryError::Validation { violations });
        }

        Ok(self.compute_spec(
            registry,
            &battery_models,
            &present_inverters,
            &present_controllers,
        ))
    }

    /// Resolve battery line items against the registry, enforcing per-item
    /// numerics (quantity, SOC/reserve fractions, SOC window).
    fn resolve_batteries<'a>(
        &self,
        registry: &'a Registry,
        violations: &mut Vec<Violation>,
    ) -> Vec<Option<&'a BatteryModel>> {
        self.batteries
            .iter()
            .enumerate()
            .map(|(i, b)| {
                if b.quantity == 0 {
                    violations.push(violation(
                        &format!("batteries[{i}].quantity"),
                        "quantity must be >= 1",
                    ));
                }
                if !(0.0..=1.0).contains(&b.initial_soc_frac) {
                    violations.push(violation(
                        &format!("batteries[{i}].initial_soc_frac"),
                        "initial SOC fraction must lie in [0, 1]",
                    ));
                }
                if !(0.0..=1.0).contains(&b.reserve_frac) {
                    violations.push(violation(
                        &format!("batteries[{i}].reserve_frac"),
                        "reserve fraction must lie in [0, 1]",
                    ));
                }
                let model = registry.battery(&b.model_id);
                if let Some(m) = model {
                    let soc = b.initial_soc_frac;
                    let (lo, hi) = (m.soc_window.min_soc_frac, m.soc_window.max_soc_frac);
                    if soc < lo || soc > hi {
                        let id = &m.model_id;
                        violations.push(violation(
                            &format!("batteries[{i}].initial_soc_frac"),
                            format!(
                                "initial SOC {soc} outside usable window [{lo}, {hi}] of `{id}`"
                            ),
                        ));
                    }
                } else {
                    let id = &b.model_id;
                    violations.push(violation(
                        &format!("batteries[{i}].model_id"),
                        format!("unknown battery model `{id}`"),
                    ));
                }
                model
            })
            .collect()
    }

    /// Resolve inverter line items against the registry.
    fn resolve_inverters<'a>(
        &self,
        registry: &'a Registry,
        violations: &mut Vec<Violation>,
    ) -> Vec<Option<&'a InverterModel>> {
        self.inverters
            .iter()
            .enumerate()
            .map(|(i, inv)| {
                if inv.quantity == 0 {
                    violations.push(violation(
                        &format!("inverters[{i}].quantity"),
                        "quantity must be >= 1",
                    ));
                }
                let model = registry.inverter(&inv.model_id);
                if model.is_none() {
                    let id = &inv.model_id;
                    violations.push(violation(
                        &format!("inverters[{i}].model_id"),
                        format!("unknown inverter model `{id}`"),
                    ));
                }
                model
            })
            .collect()
    }

    /// Resolve controller line items against the registry.
    fn resolve_controllers<'a>(
        &self,
        registry: &'a Registry,
        violations: &mut Vec<Violation>,
    ) -> Vec<Option<&'a ControllerModel>> {
        self.controllers
            .iter()
            .enumerate()
            .map(|(i, c)| {
                if c.quantity == 0 {
                    violations.push(violation(
                        &format!("controllers[{i}].quantity"),
                        "quantity must be >= 1",
                    ));
                }
                let model = registry.controller(&c.model_id);
                if model.is_none() {
                    let id = &c.model_id;
                    violations.push(violation(
                        &format!("controllers[{i}].model_id"),
                        format!("unknown controller model `{id}`"),
                    ));
                }
                model
            })
            .collect()
    }

    /// Backup composition: a backup-capable
    /// system forms its island in one of two ways - either exactly one
    /// present controller entry with `provides_grid_forming = true`, or,
    /// when no such controller is present, a battery fleet that forms the
    /// grid itself: at least one present battery, and every present battery
    /// model declaring both `grid_forming_in_backup` and an integrated
    /// inverter / transfer path (e.g. ecoLinx). A mixed fleet does not
    /// qualify: the every-battery condition fails as soon as one battery
    /// cannot help form the island, which is the intended reading.
    /// Independently, every battery's declared `requires_controller_id`
    /// must be present in controllers[].
    fn check_backup(
        &self,
        battery_models: &[Option<&BatteryModel>],
        present_controllers: &[(&ControllerModel, u32)],
        violations: &mut Vec<Violation>,
    ) {
        if !self.backup_capable {
            return;
        }
        let grid_forming = present_controllers
            .iter()
            .filter(|(c, _)| c.provides_grid_forming)
            .count();
        if grid_forming != 1 {
            let present_batteries: Vec<&BatteryModel> = self
                .batteries
                .iter()
                .zip(battery_models)
                .filter(|(r, _)| r.quantity > 0)
                .filter_map(|(_, m)| *m)
                .collect();
            let self_forming_fleet = !present_batteries.is_empty()
                && present_batteries
                    .iter()
                    .all(|m| m.grid_forming_in_backup && m.integrated_inverter == Some(true));
            if grid_forming > 1 || !self_forming_fleet {
                violations.push(violation(
                    "controllers",
                    format!(
                        "backup_capable requires either exactly one present controller entry \
                         with provides_grid_forming = true (found {grid_forming}), or, with no \
                         such controller, at least one present battery where every present \
                         battery model is grid-forming in backup and has an integrated inverter"
                    ),
                ));
            }
        }
        for (i, _) in self.batteries.iter().enumerate() {
            if let Some(m) = battery_models[i] {
                if let Some(req) = &m.requires_controller_id {
                    let present = self
                        .controllers
                        .iter()
                        .any(|c| c.quantity > 0 && &c.model_id == req);
                    if !present {
                        let id = &m.model_id;
                        violations.push(violation(
                            &format!("batteries[{i}].model_id"),
                            format!(
                                "backup_capable: `{id}` requires controller `{req}`, which is \
                                 not present in controllers[]"
                            ),
                        ));
                    }
                }
            }
        }
    }

    /// DC-coupled batteries need a compatible hybrid inverter, and every
    /// hybrid inverter must name at least one system
    /// battery in `compatible_battery_ids`. Batteries
    /// with an integrated inverter (PW3) ARE their own hybrid inverter,
    /// so the explicit-inverter intersection is skipped for them. A
    /// hybrid in `inverters[]` is always a battery hybrid: PV string-
    /// inverter use is rejected by `pv_inverter_unit_count`.
    fn check_dc_coupling(
        &self,
        battery_models: &[Option<&BatteryModel>],
        present_inverters: &[(&InverterModel, u32)],
        violations: &mut Vec<Violation>,
    ) {
        for (i, b) in self.batteries.iter().enumerate() {
            let Some(m) = battery_models[i] else { continue };
            if b.quantity == 0
                || m.coupling != Coupling::DCCoupledHybrid
                || m.integrated_inverter == Some(true)
            {
                continue;
            }
            let compatible = present_inverters.iter().any(|(inv, _)| {
                inv.compatible_battery_ids
                    .iter()
                    .any(|id| id == &m.model_id)
            });
            if !compatible {
                let id = &m.model_id;
                violations.push(violation(
                    &format!("batteries[{i}].model_id"),
                    format!(
                        "DC-coupled battery `{id}` requires a present hybrid inverter listing it \
                         in compatible_battery_ids"
                    ),
                ));
            }
        }
        for (inv, _) in present_inverters {
            if inv.topology != InverterTopology::HybridDCCoupled {
                continue;
            }
            let intersects = inv.compatible_battery_ids.iter().any(|id| {
                self.batteries
                    .iter()
                    .zip(battery_models)
                    .any(|(r, m)| r.quantity > 0 && m.is_some_and(|m| &m.model_id == id))
            });
            if !intersects {
                let id = &inv.model_id;
                violations.push(violation(
                    "inverters",
                    format!(
                        "hybrid inverter `{id}` lists no system battery in \
                         compatible_battery_ids"
                    ),
                ));
            }
        }
    }

    /// Per-model compatible-inverter capacity (SolarEdge: <= 3 per Hub).
    /// Generalized: the unit count of each battery model is bounded by the
    /// sum of `max_batteries x quantity` over present compatible inverters,
    /// but only when every compatible present inverter declares
    /// `max_batteries` (an undeclared limit means unbounded capacity).
    fn check_inverter_capacity(
        &self,
        battery_models: &[Option<&BatteryModel>],
        present_inverters: &[(&InverterModel, u32)],
        violations: &mut Vec<Violation>,
    ) {
        let mut checked: Vec<&str> = Vec::new();
        for (i, _) in self.batteries.iter().enumerate() {
            let Some(m) = battery_models[i] else { continue };
            if checked.contains(&m.model_id.as_str()) {
                continue;
            }
            checked.push(&m.model_id);
            let compatible: Vec<(&InverterModel, u32)> = present_inverters
                .iter()
                .filter(|(inv, _)| {
                    inv.compatible_battery_ids
                        .iter()
                        .any(|id| id == &m.model_id)
                })
                .map(|(inv, q)| (*inv, *q))
                .collect();
            if compatible.is_empty()
                || !compatible
                    .iter()
                    .all(|(inv, _)| inv.max_batteries.is_some())
            {
                continue;
            }
            let capacity: u32 = compatible
                .iter()
                .map(|(inv, q)| inv.max_batteries.map_or(0, |mb| mb.saturating_mul(*q)))
                .sum();
            let units: u32 = self
                .batteries
                .iter()
                .filter(|x| x.model_id == m.model_id)
                .map(|x| x.quantity)
                .sum();
            if units > capacity {
                let id = &m.model_id;
                violations.push(violation(
                    &format!("batteries[{i}].model_id"),
                    format!(
                        "{units} unit(s) of `{id}` exceed compatible-inverter capacity \
                         {capacity} (sum of max_batteries over present inverters)"
                    ),
                ));
            }
        }
    }

    /// Expansion packs (PW3: <= 3 per head unit, energy only).
    fn check_expansion_packs(
        &self,
        registry: &Registry,
        battery_models: &[Option<&BatteryModel>],
        violations: &mut Vec<Violation>,
    ) {
        for (i, b) in self.batteries.iter().enumerate() {
            if b.expansion_packs_per_unit == 0 {
                continue;
            }
            let Some(m) = battery_models[i] else { continue };
            let field = format!("batteries[{i}].expansion_packs_per_unit");
            let id = &m.model_id;
            let Some(exp) = &m.expansion else {
                violations.push(violation(
                    &field,
                    format!(
                        "`{id}` declares no expansion metadata; expansion packs are not supported"
                    ),
                ));
                continue;
            };
            let Some(pack_id) = &exp.expansion_pack_model_id else {
                violations.push(violation(
                    &field,
                    format!("`{id}` declares no expansion_pack_model_id; expansion packs are not supported"),
                ));
                continue;
            };
            if let Some(max_units) = exp.max_units_per_inverter {
                let max_packs = max_units.saturating_sub(1);
                if b.expansion_packs_per_unit > max_packs {
                    let packs = b.expansion_packs_per_unit;
                    violations.push(violation(
                        &field,
                        format!(
                            "{packs} expansion packs per unit exceed {max_packs} \
                             (max_units_per_inverter {max_units} minus the head unit)"
                        ),
                    ));
                }
            }
            if exp.packs_add_power == Some(true) {
                violations.push(violation(
                    &field,
                    format!(
                        "`{id}` declares packs_add_power = true; expansion packs must add \
                         energy only"
                    ),
                ));
            }
            if registry.battery(pack_id).is_none() {
                violations.push(violation(
                    &field,
                    format!("expansion pack model `{pack_id}` does not resolve to a battery entry"),
                ));
            }
        }
    }

    /// Microinverter power ceiling check (Enphase: 0.64 kW x IQ8D count).
    /// The derived 0.64 kW/microinverter value is a CEILING, not
    /// an equality: it holds exactly for the 5P (6 x 0.64 = 3.84 kW) but the
    /// IQ 10/10C declare continuous ratings BELOW 12 x 0.64 kW. Enforced per
    /// distinct model: continuous charge/discharge must not exceed
    /// `microinverter_count x power_per_microinverter_kw` (1e-6 kW
    /// tolerance), and a declared peak discharge rating must be >= the
    /// continuous discharge rating.
    fn check_microinverter_power(
        &self,
        battery_models: &[Option<&BatteryModel>],
        violations: &mut Vec<Violation>,
    ) {
        let mut checked: Vec<&str> = Vec::new();
        for (i, _) in self.batteries.iter().enumerate() {
            let Some(m) = battery_models[i] else { continue };
            if checked.contains(&m.model_id.as_str()) {
                continue;
            }
            checked.push(&m.model_id);
            let (Some(count), Some(per_micro)) =
                (m.microinverter_count, m.power_per_microinverter_kw.as_ref())
            else {
                continue;
            };
            let ceiling = f64::from(count) * per_micro.value;
            let id = &m.model_id;
            let per = per_micro.value;
            for (label, rating) in [
                ("discharge", &m.continuous_discharge_power_kw),
                ("charge", &m.continuous_charge_power_kw),
            ] {
                let rated = rating.value;
                if rated > ceiling + 1e-6 {
                    violations.push(violation(
                        &format!("batteries[{i}].model_id"),
                        format!(
                            "microinverter ceiling exceeded for `{id}`: continuous {label} \
                             rating {rated} kW > {count} microinverter(s) x {per} kW = \
                             {ceiling} kW"
                        ),
                    ));
                }
            }
            if let Some(peak) = &m.peak_discharge_power_kw {
                let (peak_kw, cont_kw) = (peak.value, m.continuous_discharge_power_kw.value);
                if peak_kw + 1e-6 < cont_kw {
                    violations.push(violation(
                        &format!("batteries[{i}].model_id"),
                        format!(
                            "peak discharge rating {peak_kw} kW of `{id}` is below its \
                             continuous discharge rating {cont_kw} kW"
                        ),
                    ));
                }
            }
        }
    }

    /// Generator interlock: a generator requires a present
    /// controller with `supports_generator_input`.
    fn check_generator(
        &self,
        present_controllers: &[(&ControllerModel, u32)],
        violations: &mut Vec<Violation>,
    ) {
        let Some(g) = &self.generator else { return };
        if !g.rated_kw.is_finite() || g.rated_kw <= 0.0 {
            violations.push(violation(
                "generator.rated_kw",
                "rated power must be finite and > 0",
            ));
        }
        let supported = present_controllers
            .iter()
            .any(|(c, _)| c.supports_generator_input);
        if !supported {
            violations.push(violation(
                "generator",
                "a generator requires a present controller with supports_generator_input = true",
            ));
        }
    }

    /// PV array numerics plus the inverter-landing rule: a null
    /// `pv_inverter_model_id` requires a present hybrid landing pad (hybrid
    /// inverter MPPTs or an integrated-inverter DC-coupled battery, PW3).
    fn check_pv(
        &self,
        registry: &Registry,
        battery_models: &[Option<&BatteryModel>],
        present_inverters: &[(&InverterModel, u32)],
        violations: &mut Vec<Violation>,
    ) {
        let Some(pv) = &self.pv else { return };
        if !pv.kw_dc.is_finite() || pv.kw_dc <= 0.0 {
            violations.push(violation(
                "pv.kw_dc",
                "array nameplate must be finite and > 0",
            ));
        }
        if !(0.0..=90.0).contains(&pv.tilt_deg) {
            violations.push(violation("pv.tilt_deg", "tilt must lie in [0, 90] degrees"));
        }
        if !pv.dc_ac_ratio.is_finite() || pv.dc_ac_ratio <= 0.0 {
            violations.push(violation(
                "pv.dc_ac_ratio",
                "DC/AC ratio must be finite and > 0",
            ));
        }
        if let Orientation::Azimuth(a) = pv.orientation {
            if a > 359 {
                violations.push(violation(
                    "pv.orientation",
                    "azimuth must lie in 0..=359 degrees",
                ));
            }
        }
        if let Some(pid) = &pv.pv_inverter_model_id {
            if registry.inverter(pid).is_none() {
                violations.push(violation(
                    "pv.pv_inverter_model_id",
                    format!("unknown inverter model `{pid}`"),
                ));
            }
            return;
        }
        let landing_pad = present_inverters
            .iter()
            .any(|(inv, _)| inv.topology == InverterTopology::HybridDCCoupled)
            || self.batteries.iter().zip(battery_models).any(|(r, m)| {
                r.quantity > 0
                    && m.is_some_and(|m| {
                        m.coupling == Coupling::DCCoupledHybrid
                            && m.integrated_inverter == Some(true)
                    })
            });
        if !landing_pad {
            violations.push(violation(
                "pv.pv_inverter_model_id",
                "null PV inverter requires a present hybrid landing pad (hybrid inverter \
                 MPPTs or an integrated-inverter DC-coupled battery)",
            ));
        }
    }

    /// Free-parameter numerics (main panel, backup panel, EV chargers).
    fn check_free_parameters(&self, violations: &mut Vec<Violation>) {
        if !self.main_panel.service_rating_a.is_finite() || self.main_panel.service_rating_a <= 0.0
        {
            violations.push(violation(
                "main_panel.service_rating_a",
                "service rating must be finite and > 0",
            ));
        }
        if let Some(limit) = self.main_panel.interconnection_limit_kw {
            if !limit.is_finite() || limit < 0.0 {
                violations.push(violation(
                    "main_panel.interconnection_limit_kw",
                    "interconnection limit must be finite and >= 0",
                ));
            }
        }
        if let Some(bp) = &self.backup_panel {
            if !bp.critical_loads_peak_kw.is_finite() || bp.critical_loads_peak_kw < 0.0 {
                violations.push(violation(
                    "backup_panel.critical_loads_peak_kw",
                    "critical-loads peak must be finite and >= 0",
                ));
            }
        }
        for (i, ev) in self.ev_chargers.iter().enumerate() {
            if !ev.rated_kw.is_finite() || ev.rated_kw <= 0.0 {
                violations.push(violation(
                    &format!("ev_chargers[{i}].rated_kw"),
                    "rated power must be finite and > 0",
                ));
            }
        }
    }

    /// Compute the resolved [`SystemSpec`] aggregates. Only
    /// called after validation passed, so every reference resolves.
    fn compute_spec(
        &self,
        registry: &Registry,
        battery_models: &[Option<&BatteryModel>],
        present_inverters: &[(&InverterModel, u32)],
        present_controllers: &[(&ControllerModel, u32)],
    ) -> SystemSpec {
        let mut total_usable_energy_kwh = 0.0;
        let mut total_discharge_power_kw = 0.0;
        let mut total_charge_power_kw = 0.0;
        for (b, m) in self.batteries.iter().zip(battery_models) {
            let Some(m) = m else { continue };
            let q = f64::from(b.quantity);
            let head_usable = m.usable_energy_kwh.value;
            // Packs add their usable energy per head unit, never power.
            let pack_usable = if b.expansion_packs_per_unit > 0 {
                m.expansion
                    .as_ref()
                    .and_then(|e| e.expansion_pack_model_id.as_deref())
                    .and_then(|pid| registry.battery(pid))
                    .map_or(0.0, |p| p.usable_energy_kwh.value)
            } else {
                0.0
            };
            total_usable_energy_kwh +=
                q * (head_usable + f64::from(b.expansion_packs_per_unit) * pack_usable);
            total_discharge_power_kw += q * m.continuous_discharge_power_kw.value;
            total_charge_power_kw += q * m.continuous_charge_power_kw.value;
        }

        // Backup-path rating: the minimum of every series stage of the
        // backup path: battery continuous sum, inverter backup-rating sum, and
        // the controller throughput cap when declared.
        let backup_path_power_kw = if self.backup_capable {
            let mut controller_sum = 0.0;
            let mut any_controller_rating = false;
            for (c, q) in present_controllers {
                if let Some(max) = &c.max_backup_power_kw {
                    controller_sum += max.value * f64::from(*q);
                    any_controller_rating = true;
                }
            }
            let inverter_sum: f64 = present_inverters
                .iter()
                .map(|(inv, q)| {
                    inv.max_ac_output_kw_backup
                        .as_ref()
                        .unwrap_or(&inv.rated_ac_output_kw)
                        .value
                        * f64::from(*q)
                })
                .sum();
            let mut path_rating = total_discharge_power_kw;
            if inverter_sum > 0.0 {
                path_rating = path_rating.min(inverter_sum);
            }
            if any_controller_rating {
                path_rating = path_rating.min(controller_sum);
            }
            Some(path_rating)
        } else {
            None
        };

        let resolved_controller_model_id = if self.backup_capable {
            present_controllers
                .iter()
                .find(|(c, _)| c.provides_grid_forming)
                .map(|(c, _)| c.model_id.clone())
        } else {
            None
        };

        let has_dc_coupled_storage = self.batteries.iter().zip(battery_models).any(|(r, m)| {
            r.quantity > 0 && m.is_some_and(|m| m.coupling == Coupling::DCCoupledHybrid)
        });

        SystemSpec {
            system: self.clone(),
            total_usable_energy_kwh,
            total_discharge_power_kw,
            total_charge_power_kw,
            backup_path_power_kw,
            resolved_controller_model_id,
            has_dc_coupled_storage,
        }
    }

    /// Parse a HomeSystem JSON document.
    ///
    /// # Errors
    /// [`RegistryError::Parse`] on malformed JSON or schema-shape mismatch.
    pub fn from_json(json: &str) -> Result<Self, RegistryError> {
        serde_json::from_str(json).map_err(|source| RegistryError::Parse {
            path: "<home_system>".to_owned(),
            source,
        })
    }
}

/// Violation constructor helper for composition checks.
#[must_use]
pub fn violation(field: &str, message: impl Into<String>) -> Violation {
    Violation {
        path: "<home_system>".to_owned(),
        field: field.to_owned(),
        message: message.into(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::types::{
        AnnotatedNumber, AuthStyle, Chemistry, EfficiencyCurve, EfficiencyPoint, Expansion,
        Provenance, RampRate, SocWindow, TemperatureRange, VendorApi, VendorApiFamily, Warranty,
    };

    const PW3: &str = "tesla.powerwall_3";
    const PW3_PACK: &str = "tesla.powerwall_3_expansion";
    const GATEWAY: &str = "tesla.gateway_2";
    const SE_BATTERY: &str = "solaredge.home_battery_400v";
    const SE_HUB: &str = "solaredge.home_hub_7600h";
    const SE_BACKUP_IFACE: &str = "solaredge.backup_interface";
    const ENP_5P: &str = "enphase.iq_battery_5p";
    const ENP_CTRL: &str = "enphase.iq_system_controller_3";
    const ECOLINX: &str = "sonnen.ecolinx";

    fn curve() -> EfficiencyCurve {
        EfficiencyCurve {
            points: vec![
                EfficiencyPoint {
                    x_kw: 0.5,
                    efficiency: 0.90,
                },
                EfficiencyPoint {
                    x_kw: 20.0,
                    efficiency: 0.95,
                },
            ],
            provenance: Provenance::Estimated,
        }
    }

    fn vendor_api() -> VendorApi {
        VendorApi {
            family: VendorApiFamily::Generic,
            auth_style: AuthStyle::None,
            base_path_hint: None,
            endpoints: vec![],
            provenance: Provenance::Estimated,
        }
    }

    fn battery(
        model_id: &str,
        coupling: Coupling,
        usable_kwh: f64,
        discharge_kw: f64,
        charge_kw: f64,
    ) -> BatteryModel {
        BatteryModel {
            schema_version: crate::types::SCHEMA_VERSION.to_owned(),
            entry_version: "1.0.0".to_owned(),
            supersedes: None,
            model_id: model_id.to_owned(),
            vendor: "Test".to_owned(),
            display_name: model_id.to_owned(),
            chemistry: Chemistry::LFP,
            coupling,
            nameplate_energy_kwh: AnnotatedNumber::spec(usable_kwh, "kWh"),
            usable_energy_kwh: AnnotatedNumber::spec(usable_kwh, "kWh"),
            continuous_discharge_power_kw: AnnotatedNumber::spec(discharge_kw, "kW"),
            peak_discharge_power_kw: None,
            peak_duration_s: None,
            continuous_charge_power_kw: AnnotatedNumber::spec(charge_kw, "kW"),
            soc_window: SocWindow {
                min_soc_frac: 0.0,
                max_soc_frac: 1.0,
                reserve_floor_frac: Some(0.2),
                provenance: Provenance::Spec,
            },
            charge_efficiency_curve: curve(),
            discharge_efficiency_curve: curve(),
            rte_pv_coupled: None,
            rte_ac_coupled: None,
            grid_forming_in_backup: true,
            requires_controller_id: None,
            integrated_inverter: None,
            microinverter_count: None,
            power_per_microinverter_kw: None,
            expansion: None,
            warranty: Warranty::default(),
            operating_temperature: TemperatureRange {
                min_c: -20.0,
                max_c: 50.0,
                derating_note: None,
                provenance: Provenance::Spec,
            },
            cooling: None,
            ramp_rate: RampRate {
                max_kw_per_s: discharge_kw,
                provenance: Provenance::Estimated,
                note: None,
            },
            self_discharge_frac_per_day: None,
            vendor_api: vendor_api(),
        }
    }

    /// PW3 head unit: integrated hybrid inverter, expansion to 3 packs,
    /// backup requires the Tesla Gateway.
    fn pw3() -> BatteryModel {
        let mut m = battery(PW3, Coupling::DCCoupledHybrid, 13.5, 11.5, 11.5);
        m.integrated_inverter = Some(true);
        m.requires_controller_id = Some(GATEWAY.to_owned());
        m.expansion = Some(Expansion {
            max_units_per_inverter: Some(4),
            expansion_pack_model_id: Some(PW3_PACK.to_owned()),
            packs_add_power: Some(false),
        });
        m
    }

    fn pw3_pack() -> BatteryModel {
        battery(PW3_PACK, Coupling::DCCoupledHybrid, 13.5, 0.0, 0.0)
    }

    fn se_battery() -> BatteryModel {
        battery(SE_BATTERY, Coupling::DCCoupledHybrid, 9.7, 5.0, 5.0)
    }

    /// ecoLinx-style AC-coupled battery: forms the island itself through an
    /// integrated inverter and transfer path, so backup needs no separate
    /// controller.
    fn ecolinx() -> BatteryModel {
        let mut m = battery(ECOLINX, Coupling::ACCoupled, 20.0, 8.0, 8.0);
        m.integrated_inverter = Some(true);
        m
    }

    /// Enphase IQ Battery 5P: 6 x 0.64 kW IQ8D microinverters = 3.84 kW.
    fn enphase_5p() -> BatteryModel {
        let mut m = battery(ENP_5P, Coupling::MicroinverterBased, 5.0, 3.84, 3.84);
        m.microinverter_count = Some(6);
        m.power_per_microinverter_kw = Some(AnnotatedNumber::spec(0.64, "kW"));
        m.requires_controller_id = Some(ENP_CTRL.to_owned());
        m.integrated_inverter = Some(true);
        m
    }

    /// Enphase IQ Battery 10: 12 x IQ8D microinverters, but continuous
    /// 3.84 kW / peak 7.68 kW - BELOW the 12 x 0.64 = 7.68 kW ceiling.
    fn enphase_10() -> BatteryModel {
        let mut m = battery(
            "enphase.iq_battery_10",
            Coupling::MicroinverterBased,
            10.0,
            3.84,
            3.84,
        );
        m.peak_discharge_power_kw = Some(AnnotatedNumber::spec(7.68, "kW"));
        m.microinverter_count = Some(12);
        m.power_per_microinverter_kw = Some(AnnotatedNumber::spec(0.64, "kW"));
        m.requires_controller_id = Some(ENP_CTRL.to_owned());
        m.integrated_inverter = Some(true);
        m
    }

    fn controller(model_id: &str, grid_forming: bool, generator: bool) -> ControllerModel {
        ControllerModel {
            schema_version: crate::types::SCHEMA_VERSION.to_owned(),
            entry_version: "1.0.0".to_owned(),
            model_id: model_id.to_owned(),
            vendor: "Test".to_owned(),
            display_name: model_id.to_owned(),
            provides_grid_forming: grid_forming,
            transfer_time_s: AnnotatedNumber::estimated(0.1, "s", "default"),
            reconnect_s: None,
            supports_generator_input: generator,
            frequency_shift_curtailment: None,
            max_backup_power_kw: None,
            pv_blackstart: None,
            standby_power_w: None,
            vendor_api: None,
        }
    }

    fn gateway() -> ControllerModel {
        controller(GATEWAY, true, true)
    }

    fn enphase_controller() -> ControllerModel {
        controller(ENP_CTRL, true, true)
    }

    fn se_hub() -> InverterModel {
        InverterModel {
            schema_version: crate::types::SCHEMA_VERSION.to_owned(),
            entry_version: "1.0.0".to_owned(),
            model_id: SE_HUB.to_owned(),
            vendor: "SolarEdge".to_owned(),
            display_name: SE_HUB.to_owned(),
            topology: InverterTopology::HybridDCCoupled,
            rated_ac_output_kw: AnnotatedNumber::spec(7.6, "kW"),
            max_ac_output_kw_backup: Some(AnnotatedNumber::spec(7.6, "kW")),
            max_pv_dc_input_kw: None,
            mppt_count: None,
            max_pv_voltage_v: None,
            efficiency_curve: curve(),
            grid_following_on_grid: true,
            grid_forming_in_backup: true,
            compatible_battery_ids: vec![SE_BATTERY.to_owned()],
            max_batteries: Some(3),
            vendor_api: None,
        }
    }

    fn battery_ref(model_id: &str, quantity: u32) -> BatteryRef {
        BatteryRef {
            model_id: model_id.to_owned(),
            quantity,
            expansion_packs_per_unit: 0,
            initial_soc_frac: 0.5,
            reserve_frac: 0.2,
        }
    }

    fn inverter_ref(model_id: &str, quantity: u32) -> InverterRef {
        InverterRef {
            model_id: model_id.to_owned(),
            quantity,
        }
    }

    fn controller_ref(model_id: &str, quantity: u32) -> ControllerRef {
        ControllerRef {
            model_id: model_id.to_owned(),
            quantity,
        }
    }

    fn base_system() -> HomeSystem {
        HomeSystem {
            schema_version: crate::types::SCHEMA_VERSION.to_owned(),
            system_id: "00000000-0000-0000-0000-000000000001".to_owned(),
            label: None,
            batteries: vec![],
            inverters: vec![],
            controllers: vec![],
            pv: None,
            main_panel: MainPanel {
                service_rating_a: 200.0,
                interconnection_limit_kw: None,
            },
            backup_capable: false,
            backup_panel: None,
            generator: None,
            ev_chargers: vec![],
            grid_meter: GridMeter {
                esiid: "1008901000000000000001".to_owned(),
                tdsp: None,
            },
        }
    }

    fn violations_of(result: Result<SystemSpec, RegistryError>) -> Vec<Violation> {
        match result {
            Err(RegistryError::Validation { violations }) => violations,
            Err(other) => panic!("unexpected error kind: {other}"),
            Ok(_) => panic!("expected validation violations, got Ok"),
        }
    }

    fn assert_field(violations: &[Violation], field: &str) {
        assert!(
            violations.iter().any(|v| v.field == field),
            "expected a violation at `{field}`, got: {violations:?}"
        );
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-9,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn valid_pw3_system_with_packs_and_gateway_passes() {
        let registry = Registry::from_parts(vec![pw3(), pw3_pack()], vec![], vec![gateway()]);
        let mut sys = base_system();
        sys.batteries = vec![BatteryRef {
            expansion_packs_per_unit: 3,
            ..battery_ref(PW3, 1)
        }];
        sys.controllers = vec![controller_ref(GATEWAY, 1)];
        sys.backup_capable = true;
        sys.pv = Some(PvConfig {
            kw_dc: 8.0,
            orientation: Orientation::Named(NamedOrientation::S),
            tilt_deg: 25.0,
            dc_ac_ratio: 1.2,
            pv_inverter_model_id: None, // lands on the PW3's integrated MPPTs
        });

        let resolved = sys.validate(&registry).expect("valid PW3 system must pass");
        // 1 x (13.5 + 3 x 13.5) kWh.
        assert_close(resolved.total_usable_energy_kwh, 54.0);
        assert_close(resolved.total_discharge_power_kw, 11.5);
        assert_close(resolved.total_charge_power_kw, 11.5);
        // No controller rating, no explicit inverter: battery rating rules.
        assert_close(resolved.backup_path_power_kw.expect("backup-capable"), 11.5);
        assert_eq!(
            resolved.resolved_controller_model_id.as_deref(),
            Some(GATEWAY)
        );
        assert!(resolved.has_dc_coupled_storage);
    }

    #[test]
    fn unknown_model_ids_are_all_enumerated() {
        let registry = Registry::from_parts(vec![], vec![], vec![]);
        let mut sys = base_system();
        sys.batteries = vec![battery_ref("no.such_battery", 1)];
        sys.inverters = vec![inverter_ref("no.such_inverter", 1)];
        sys.controllers = vec![controller_ref("no.such_controller", 1)];

        let v = violations_of(sys.validate(&registry));
        assert_eq!(v.len(), 3, "every unknown id enumerated: {v:?}");
        assert_field(&v, "batteries[0].model_id");
        assert_field(&v, "inverters[0].model_id");
        assert_field(&v, "controllers[0].model_id");
        assert!(v.iter().all(|x| x.message.contains("no.such_")));
    }

    #[test]
    fn schema_version_mismatch_is_a_violation() {
        let registry = Registry::from_parts(vec![], vec![], vec![]);
        let mut sys = base_system();
        sys.schema_version = "0.9.0".to_owned();
        let v = violations_of(sys.validate(&registry));
        assert_field(&v, "schema_version");
    }

    #[test]
    fn backup_capable_requires_one_grid_forming_controller() {
        let registry = Registry::from_parts(vec![pw3()], vec![], vec![gateway()]);

        // No controller at all: PW3 can form the grid itself, so the fleet
        // rule is satisfied - but PW3 declares a required controller, and
        // that check still rejects the system on the battery line item.
        let mut sys = base_system();
        sys.batteries = vec![battery_ref(PW3, 1)];
        sys.backup_capable = true;
        let v = violations_of(sys.validate(&registry));
        assert!(v.iter().all(|x| x.field != "controllers"), "{v:?}");
        assert_field(&v, "batteries[0].model_id");

        // Two grid-forming controller entries: still a violation.
        sys.controllers = vec![controller_ref(GATEWAY, 1), controller_ref(GATEWAY, 1)];
        let v = violations_of(sys.validate(&registry));
        assert_eq!(v.len(), 1);
        assert_field(&v, "controllers");
        assert!(v[0].message.contains('2'));

        // A non-grid-forming controller does not count, and a battery that
        // cannot form the grid itself leaves the system without an island
        // source.
        let registry = Registry::from_parts(
            vec![se_battery()],
            vec![],
            vec![controller(GATEWAY, false, false)],
        );
        sys.batteries = vec![battery_ref(SE_BATTERY, 1)];
        sys.controllers = vec![controller_ref(GATEWAY, 1)];
        let v = violations_of(sys.validate(&registry));
        assert_field(&v, "controllers");
    }

    #[test]
    fn dc_coupled_battery_requires_compatible_hybrid_inverter() {
        let registry = Registry::from_parts(vec![se_battery()], vec![se_hub()], vec![]);

        let mut sys = base_system();
        sys.batteries = vec![battery_ref(SE_BATTERY, 1)];
        let v = violations_of(sys.validate(&registry));
        assert_field(&v, "batteries[0].model_id");
        assert!(v[0].message.contains("DC-coupled"));

        sys.inverters = vec![inverter_ref(SE_HUB, 1)];
        sys.validate(&registry).expect("compatible hybrid present");
    }

    #[test]
    fn solaredge_battery_count_bounded_by_hub_capacity() {
        let registry = Registry::from_parts(vec![se_battery()], vec![se_hub()], vec![]);

        // 4 batteries on a single 3-battery hub: violation.
        let mut sys = base_system();
        sys.batteries = vec![battery_ref(SE_BATTERY, 4)];
        sys.inverters = vec![inverter_ref(SE_HUB, 1)];
        let v = violations_of(sys.validate(&registry));
        assert_field(&v, "batteries[0].model_id");
        assert!(v[0].message.contains("capacity 3"));

        // 3 batteries: exactly at capacity, passes.
        sys.batteries = vec![battery_ref(SE_BATTERY, 3)];
        sys.validate(&registry).expect("3 on one hub is allowed");

        // Two hubs lift capacity to 6; 7 still fails.
        sys.inverters = vec![inverter_ref(SE_HUB, 2)];
        sys.validate(&registry).expect("3 on two hubs is allowed");
        sys.batteries = vec![battery_ref(SE_BATTERY, 7)];
        let v = violations_of(sys.validate(&registry));
        assert!(v[0].message.contains("capacity 6"));
    }

    #[test]
    fn pw3_expansion_pack_rules() {
        // packs > max_units_per_inverter - 1 (i.e. > 3).
        let registry = Registry::from_parts(vec![pw3(), pw3_pack()], vec![], vec![gateway()]);
        let mut sys = base_system();
        sys.batteries = vec![BatteryRef {
            expansion_packs_per_unit: 4,
            ..battery_ref(PW3, 1)
        }];
        sys.controllers = vec![controller_ref(GATEWAY, 1)];
        sys.backup_capable = true;
        let v = violations_of(sys.validate(&registry));
        assert_field(&v, "batteries[0].expansion_packs_per_unit");
        assert!(v.iter().any(|x| x.message.contains("exceed 3")));

        // Packs on a model that declares no expansion support.
        let registry = Registry::from_parts(vec![enphase_5p()], vec![], vec![enphase_controller()]);
        let mut sys = base_system();
        sys.batteries = vec![BatteryRef {
            expansion_packs_per_unit: 1,
            ..battery_ref(ENP_5P, 1)
        }];
        sys.controllers = vec![controller_ref(ENP_CTRL, 1)];
        sys.backup_capable = true;
        let v = violations_of(sys.validate(&registry));
        assert_field(&v, "batteries[0].expansion_packs_per_unit");
        assert!(v.iter().any(|x| x.message.contains("not supported")));

        // Doctored model claiming packs add power.
        let mut dirty = pw3();
        dirty.expansion.as_mut().unwrap().packs_add_power = Some(true);
        let registry = Registry::from_parts(vec![dirty, pw3_pack()], vec![], vec![gateway()]);
        let mut sys = base_system();
        sys.batteries = vec![BatteryRef {
            expansion_packs_per_unit: 1,
            ..battery_ref(PW3, 1)
        }];
        sys.controllers = vec![controller_ref(GATEWAY, 1)];
        sys.backup_capable = true;
        let v = violations_of(sys.validate(&registry));
        assert!(v.iter().any(|x| x.message.contains("packs_add_power")));

        // Pack model missing from the registry.
        let registry = Registry::from_parts(vec![pw3()], vec![], vec![gateway()]);
        let v = violations_of(sys.validate(&registry));
        assert!(v.iter().any(|x| x.message.contains(PW3_PACK)));
    }

    #[test]
    fn enphase_microinverter_ceiling() {
        // 2 x 5P: 2 x 6 x 0.64 = 7.68 kW continuous, 10 kWh usable; the 5P
        // sits exactly at the ceiling.
        let registry = Registry::from_parts(vec![enphase_5p()], vec![], vec![enphase_controller()]);
        let mut sys = base_system();
        sys.batteries = vec![battery_ref(ENP_5P, 2)];
        sys.controllers = vec![controller_ref(ENP_CTRL, 1)];
        sys.backup_capable = true;
        let resolved = sys
            .validate(&registry)
            .expect("2x5P passes the ceiling check");
        assert_close(resolved.total_usable_energy_kwh, 10.0);
        assert_close(resolved.total_discharge_power_kw, 7.68);
        assert_close(resolved.total_charge_power_kw, 7.68);
        assert_close(resolved.backup_path_power_kw.expect("backup-capable"), 7.68);
        assert!(!resolved.has_dc_coupled_storage);

        // IQ 10: continuous 3.84 kW is BELOW the 12 x 0.64 = 7.68 kW
        // ceiling; the catalog's own entry must pass.
        let registry = Registry::from_parts(vec![enphase_10()], vec![], vec![enphase_controller()]);
        let mut sys = base_system();
        sys.batteries = vec![battery_ref("enphase.iq_battery_10", 1)];
        sys.controllers = vec![controller_ref(ENP_CTRL, 1)];
        sys.backup_capable = true;
        sys.validate(&registry)
            .expect("IQ 10 under the ceiling passes");

        // Doctored model: continuous rating ABOVE the ceiling.
        let mut dirty = enphase_5p();
        dirty.continuous_discharge_power_kw = AnnotatedNumber::spec(3.9, "kW");
        let registry = Registry::from_parts(vec![dirty], vec![], vec![enphase_controller()]);
        let mut sys = base_system();
        sys.batteries = vec![battery_ref(ENP_5P, 1)];
        sys.controllers = vec![controller_ref(ENP_CTRL, 1)];
        sys.backup_capable = true;
        let v = violations_of(sys.validate(&registry));
        assert_eq!(v.len(), 1);
        assert_field(&v, "batteries[0].model_id");
        assert!(v[0].message.contains("microinverter ceiling"));

        // Doctored model: peak below continuous.
        let mut dirty = enphase_10();
        dirty.peak_discharge_power_kw = Some(AnnotatedNumber::spec(3.0, "kW"));
        let registry = Registry::from_parts(vec![dirty], vec![], vec![enphase_controller()]);
        sys.batteries = vec![battery_ref("enphase.iq_battery_10", 1)];
        let v = violations_of(sys.validate(&registry));
        assert_eq!(v.len(), 1);
        assert!(v[0].message.contains("peak discharge rating"));
    }

    #[test]
    fn generator_requires_a_supporting_controller() {
        // Controller without generator input support.
        let registry = Registry::from_parts(
            vec![enphase_5p()],
            vec![],
            vec![controller(ENP_CTRL, true, false)],
        );
        let mut sys = base_system();
        sys.batteries = vec![battery_ref(ENP_5P, 1)];
        sys.controllers = vec![controller_ref(ENP_CTRL, 1)];
        sys.generator = Some(GeneratorConfig {
            rated_kw: 18.0,
            auto_start: true,
        });
        let v = violations_of(sys.validate(&registry));
        assert_field(&v, "generator");

        // IQ System Controller supports a generator input.
        let registry = Registry::from_parts(vec![enphase_5p()], vec![], vec![enphase_controller()]);
        sys.validate(&registry)
            .expect("generator-capable controller present");
    }

    #[test]
    fn pv_inverter_rules() {
        let registry = Registry::from_parts(
            vec![se_battery(), enphase_5p()],
            vec![se_hub()],
            vec![enphase_controller()],
        );
        let pv = |id: Option<&str>| PvConfig {
            kw_dc: 8.0,
            orientation: Orientation::Azimuth(180),
            tilt_deg: 25.0,
            dc_ac_ratio: 1.2,
            pv_inverter_model_id: id.map(str::to_owned),
        };

        // Unknown PV inverter id.
        let mut sys = base_system();
        sys.pv = Some(pv(Some("no.such_pv_inverter")));
        let v = violations_of(sys.validate(&registry));
        assert_field(&v, "pv.pv_inverter_model_id");

        // Resolved PV inverter id is accepted even off the inverters[] list.
        sys.pv = Some(pv(Some(SE_HUB)));
        sys.validate(&registry).expect("resolvable PV inverter id");

        // Null PV inverter with no hybrid landing pad (AC-coupled Enphase only).
        sys.batteries = vec![battery_ref(ENP_5P, 1)];
        sys.controllers = vec![controller_ref(ENP_CTRL, 1)];
        sys.pv = Some(pv(None));
        let v = violations_of(sys.validate(&registry));
        assert_field(&v, "pv.pv_inverter_model_id");
        assert!(v[0].message.contains("hybrid"));

        // Null PV inverter with a hybrid inverter present: PV lands on MPPTs.
        sys.batteries = vec![battery_ref(SE_BATTERY, 1)];
        sys.inverters = vec![inverter_ref(SE_HUB, 1)];
        sys.controllers = vec![];
        sys.validate(&registry)
            .expect("hybrid MPPT landing pad present");
    }

    #[test]
    fn numeric_bounds_are_enforced_per_line_item() {
        let mut narrow_window = pw3();
        narrow_window.soc_window.min_soc_frac = 0.1;
        narrow_window.soc_window.max_soc_frac = 0.9;
        let registry =
            Registry::from_parts(vec![narrow_window, pw3_pack()], vec![], vec![gateway()]);

        let mut sys = base_system();
        sys.batteries = vec![BatteryRef {
            quantity: 0,
            initial_soc_frac: 0.95, // outside [0.1, 0.9]
            reserve_frac: 1.5,
            ..battery_ref(PW3, 1)
        }];
        sys.controllers = vec![controller_ref(GATEWAY, 1)];
        sys.backup_capable = true;
        let v = violations_of(sys.validate(&registry));
        assert_field(&v, "batteries[0].quantity");
        assert_field(&v, "batteries[0].initial_soc_frac");
        assert_field(&v, "batteries[0].reserve_frac");
        assert!(v
            .iter()
            .filter(|x| x.field == "batteries[0].initial_soc_frac")
            .any(|x| x.message.contains("outside usable window")));
    }

    #[test]
    fn backup_path_power_resolution_order() {
        // Explicit hybrid inverter backup rating caps the battery sum:
        // min(2 x 5.0, 7.6) = 7.6 kW.
        let se_iface = controller(SE_BACKUP_IFACE, true, false);
        let registry = Registry::from_parts(vec![se_battery()], vec![se_hub()], vec![se_iface]);
        let mut sys = base_system();
        sys.batteries = vec![battery_ref(SE_BATTERY, 2)];
        sys.inverters = vec![inverter_ref(SE_HUB, 1)];
        sys.controllers = vec![controller_ref(SE_BACKUP_IFACE, 1)];
        sys.backup_capable = true;
        let resolved = sys
            .validate(&registry)
            .expect("valid SolarEdge backup system");
        assert_close(resolved.backup_path_power_kw.expect("backup-capable"), 7.6);

        // A controller throughput cap outranks the battery sum:
        // min(11.5, 10.0) = 10.0 kW.
        let mut capped = gateway();
        capped.max_backup_power_kw = Some(AnnotatedNumber::spec(10.0, "kW"));
        let registry = Registry::from_parts(vec![pw3(), pw3_pack()], vec![], vec![capped]);
        let mut sys = base_system();
        sys.batteries = vec![battery_ref(PW3, 1)];
        sys.controllers = vec![controller_ref(GATEWAY, 1)];
        sys.backup_capable = true;
        let resolved = sys.validate(&registry).expect("valid PW3 backup system");
        assert_close(resolved.backup_path_power_kw.expect("backup-capable"), 10.0);

        // Not backup-capable: no backup-path power.
        sys.backup_capable = false;
        let resolved = sys.validate(&registry).expect("non-backup system");
        assert!(resolved.backup_path_power_kw.is_none());
        assert!(resolved.resolved_controller_model_id.is_none());
    }

    #[test]
    fn backup_with_integrated_grid_forming_battery_needs_no_controller() {
        let registry = Registry::from_parts(vec![ecolinx()], vec![], vec![]);
        let mut sys = base_system();
        sys.batteries = vec![battery_ref(ECOLINX, 2)];
        sys.backup_capable = true;

        let spec = sys
            .validate(&registry)
            .expect("self-forming battery fleet must pass");
        assert!(spec.resolved_controller_model_id.is_none());
        // No controller and no explicit inverter: the battery continuous sum rules.
        assert_close(spec.backup_path_power_kw.expect("backup-capable"), 16.0);
    }

    #[test]
    fn backup_without_controller_rejects_non_integrated_battery() {
        let registry = Registry::from_parts(vec![se_battery()], vec![se_hub()], vec![]);
        let mut sys = base_system();
        sys.batteries = vec![battery_ref(SE_BATTERY, 1)];
        sys.inverters = vec![inverter_ref(SE_HUB, 1)];
        sys.backup_capable = true;

        let violations = violations_of(sys.validate(&registry));
        assert_field(&violations, "controllers");
    }

    #[test]
    fn backup_without_controller_rejects_mixed_battery_fleet() {
        let registry = Registry::from_parts(vec![ecolinx(), se_battery()], vec![se_hub()], vec![]);
        let mut sys = base_system();
        sys.batteries = vec![battery_ref(ECOLINX, 1), battery_ref(SE_BATTERY, 1)];
        sys.inverters = vec![inverter_ref(SE_HUB, 1)];
        sys.backup_capable = true;

        let violations = violations_of(sys.validate(&registry));
        assert_field(&violations, "controllers");
    }

    #[test]
    fn embedded_ecolinx_backup_system_validates_without_controller() {
        // The reported composition, against the shipped catalog: a
        // backup-capable ecoLinx fleet forms its own island through its
        // integrated inverter, so it must validate with no controller.
        let registry = Registry::embedded().expect("embedded registry");
        let mut sys = base_system();
        sys.batteries = vec![battery_ref(ECOLINX, 2)];
        sys.backup_capable = true;

        let spec = sys
            .validate(&registry)
            .expect("embedded ecoLinx backup system must pass");
        assert!(spec.resolved_controller_model_id.is_none());
        assert_close(spec.backup_path_power_kw.expect("backup-capable"), 16.0);
    }
}
