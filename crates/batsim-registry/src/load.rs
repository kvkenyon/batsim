//! Registry loading: build-time embedded catalog plus optional external
//! shadow directory.
//!
//! - [`Registry::embedded`] loads the catalog compiled into the binary.
//! - [`Registry::from_dir`] loads a catalog tree from disk.
//! - [`Registry::load`] layers an optional external directory over the
//!   embedded catalog; shadowing is entry-by-entry on `(kind, model_id)`
//!   and every shadow is logged.
//!
//! Loading performs, in order: per-file content-hash verification against
//! `catalog.json`, JSON parse into the typed schema targets
//! (structural validation: the serde types in [`crate::types`] mirror the
//! catalog JSON schemas field-for-field, with `deny_unknown_fields`),
//! semantic validation ([`crate::validate`]: bounds, patterns, monotonic
//! curves, cross-references - every violation enumerated, never fail-fast),
//! and finally the whole-catalog `catalog_sha256` integrity check.
//!
//! Note: the `jsonschema` crate was evaluated for validating against the
//! JSON schema documents in `registry/schemas/` directly, but its
//! dependency tree requires a newer toolchain than the pinned 1.83.0. The
//! typed-target + semantic-pass approach covers the same contract; the
//! schema documents remain the human-readable source of truth.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use include_dir::{include_dir, Dir, File};
use sha2::{Digest, Sha256};

use crate::error::{RegistryError, Violation};
use crate::types::{
    BatteryModel, CatalogEntry, CatalogManifest, ControllerModel, EntryKind, InverterModel,
    PvPreset,
};
use crate::validate;

/// The build-time embedded catalog tree.
static EMBEDDED: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../registry");

/// Manifest filename at the registry root.
const MANIFEST_PATH: &str = "catalog.json";

/// Recursively collect every file in an embedded directory
/// (`Dir::files` is direct-children only in include_dir 0.7).
fn walk_embedded(dir: &'static Dir<'static>, out: &mut Vec<&'static File<'static>>) {
    for file in dir.files() {
        out.push(file);
    }
    for sub in dir.dirs() {
        walk_embedded(sub, out);
    }
}

/// Where the loaded catalog came from (recorded for the run manifest).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrySource {
    /// The build-time embedded catalog.
    Embedded,
    /// An external directory, with any embedded entries it shadowed.
    External {
        /// Directory the catalog was loaded from.
        dir: String,
        /// `kind:model_id` keys of embedded entries shadowed by the
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

/// A parsed catalog entry of any kind (internal pipeline representation).
/// Boxed: `BatteryModel` is much larger than the other variants.
#[derive(Debug)]
enum ParsedEntry {
    Battery(Box<BatteryModel>),
    Inverter(Box<InverterModel>),
    Controller(Box<ControllerModel>),
    PvPreset(Box<PvPreset>),
}

impl ParsedEntry {
    fn kind(&self) -> EntryKind {
        match self {
            Self::Battery(_) => EntryKind::Battery,
            Self::Inverter(_) => EntryKind::Inverter,
            Self::Controller(_) => EntryKind::Controller,
            Self::PvPreset(_) => EntryKind::PvPreset,
        }
    }

    fn model_id(&self) -> &str {
        match self {
            Self::Battery(m) => &m.model_id,
            Self::Inverter(m) => &m.model_id,
            Self::Controller(m) => &m.model_id,
            Self::PvPreset(m) => &m.preset_id,
        }
    }

    fn entry_version(&self) -> &str {
        match self {
            Self::Battery(m) => &m.entry_version,
            Self::Inverter(m) => &m.entry_version,
            Self::Controller(m) => &m.entry_version,
            Self::PvPreset(m) => &m.entry_version,
        }
    }

    /// Per-entry semantic checks (see [`crate::validate`]).
    fn check(&self, path: &str) -> Vec<Violation> {
        match self {
            Self::Battery(m) => validate::check_battery(path, m),
            Self::Inverter(m) => validate::check_inverter(path, m),
            Self::Controller(m) => validate::check_controller(path, m),
            Self::PvPreset(m) => validate::check_pv_preset(path, m),
        }
    }
}

/// One entry file after parse: its manifest record plus the typed content.
#[derive(Debug)]
struct ParsedFile {
    record: CatalogEntry,
    parsed: ParsedEntry,
}

/// Intermediate load result: manifest plus parsed entries, before the
/// registry maps are assembled.
#[derive(Debug)]
struct LoadedTree {
    manifest: CatalogManifest,
    files: Vec<ParsedFile>,
    /// Whether `catalog_sha256` came from an on-disk/embedded manifest and
    /// must be verified (false when the manifest was synthesized from
    /// content, in which case it is consistent by construction).
    verify_catalog_hash: bool,
}

/// Lowercase hex of the SHA-256 digest of `bytes`.
fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Whole-catalog integrity hash, the normative default:
/// SHA-256 over the concatenation, in lexicographic path order, of each
/// entry file's raw 32-byte SHA-256 digest, hex-encoded.
fn catalog_sha256(records: &[&CatalogEntry]) -> String {
    let mut hasher = Sha256::new();
    for record in records {
        hasher.update(hex_to_digest(&record.sha256));
    }
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// Decode a 64-char lowercase hex digest into its raw 32 bytes. Manifest
/// hashes are produced by [`hex_sha256`], so decoding always succeeds; a
/// malformed digest hashes as all-zero bytes, which simply fails the
/// integrity comparison.
fn hex_to_digest(hex: &str) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks_exact(2).take(32).enumerate() {
        let hi = (chunk[0] as char).to_digit(16);
        let lo = (chunk[1] as char).to_digit(16);
        if let (Some(hi), Some(lo)) = (hi, lo) {
            out[i] = ((hi << 4) | lo) as u8;
        }
    }
    out
}

/// Where entry bytes come from.
enum EntrySource<'a> {
    /// The build-time embedded tree.
    Embedded,
    /// A directory on disk.
    Disk(&'a Path),
}

/// Raw tree as read: the manifest (when present) and every entry file as
/// `(relative_path, bytes)` pairs sorted by relative path.
type RawTree = (Option<CatalogManifest>, Vec<(String, Vec<u8>)>);

/// Read the raw tree: the manifest (if present) and every entry file under
/// the four kind directories.
fn read_tree(source: &EntrySource<'_>) -> Result<RawTree, RegistryError> {
    match source {
        EntrySource::Embedded => {
            let manifest_bytes = EMBEDDED
                .get_file(MANIFEST_PATH)
                .map(|f| f.contents().to_vec())
                .ok_or_else(|| {
                    RegistryError::Integrity("embedded catalog.json missing".to_owned())
                })?;
            let manifest = parse_manifest(&manifest_bytes)?;
            let mut all = Vec::new();
            walk_embedded(&EMBEDDED, &mut all);
            let mut files = Vec::new();
            for file in all {
                let rel = file.path().to_string_lossy().replace('\\', "/");
                if rel == MANIFEST_PATH {
                    continue;
                }
                // Only declared tree members: `.json` files under a kind
                // directory. Stray non-entry files (e.g. `.DS_Store`) must
                // not fail the integrity file-set check.
                let is_entry = Path::new(&rel)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
                    && rel
                        .split('/')
                        .next()
                        .and_then(EntryKind::from_dir)
                        .is_some();
                if is_entry {
                    files.push((rel, file.contents().to_vec()));
                }
            }
            files.sort_by(|a, b| a.0.cmp(&b.0));
            Ok((Some(manifest), files))
        }
        EntrySource::Disk(root) => {
            let manifest_path = root.join(MANIFEST_PATH);
            let manifest = match std::fs::read(&manifest_path) {
                Ok(bytes) => Some(parse_manifest(&bytes)?),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(source) => {
                    return Err(RegistryError::Io {
                        path: manifest_path,
                        source,
                    });
                }
            };
            let mut files = Vec::new();
            for kind in EntryKind::ALL {
                let dir = root.join(kind.dir());
                let read_dir = match std::fs::read_dir(&dir) {
                    Ok(rd) => rd,
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                    Err(source) => return Err(RegistryError::Io { path: dir, source }),
                };
                for entry in read_dir {
                    let entry = entry.map_err(|source| RegistryError::Io {
                        path: dir.clone(),
                        source,
                    })?;
                    let path = entry.path();
                    if path
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
                    {
                        let rel = format!("{}/{}", kind.dir(), entry.file_name().to_string_lossy());
                        let bytes = std::fs::read(&path).map_err(|source| RegistryError::Io {
                            path: path.clone(),
                            source,
                        })?;
                        files.push((rel, bytes));
                    }
                }
            }
            files.sort_by(|a, b| a.0.cmp(&b.0));
            Ok((manifest, files))
        }
    }
}

/// Parse `catalog.json` bytes into the typed manifest.
fn parse_manifest(bytes: &[u8]) -> Result<CatalogManifest, RegistryError> {
    serde_json::from_slice(bytes).map_err(|source| RegistryError::Parse {
        path: MANIFEST_PATH.to_owned(),
        source,
    })
}

/// Phase 1, integrity: per-file content hashes against the manifest,
/// plus file-set agreement. All mismatches are enumerated in one error.
fn verify_file_hashes(
    manifest: &CatalogManifest,
    files: &[(String, Vec<u8>)],
) -> Result<(), RegistryError> {
    let mut problems: Vec<String> = Vec::new();
    let declared: BTreeMap<&str, &CatalogEntry> = manifest
        .entries
        .iter()
        .map(|e| (e.path.as_str(), e))
        .collect();
    let on_disk: BTreeMap<&str, &Vec<u8>> = files.iter().map(|(p, b)| (p.as_str(), b)).collect();
    for path in declared.keys() {
        if !on_disk.contains_key(*path) {
            problems.push(format!("{path}: declared in catalog.json but file missing"));
        }
    }
    for (path, bytes) in &on_disk {
        match declared.get(*path) {
            None => problems.push(format!("{path}: entry file not declared in catalog.json")),
            Some(record) => {
                let actual = hex_sha256(bytes);
                if actual != record.sha256 {
                    problems.push(format!(
                        "{path}: content hash mismatch (catalog.json {}, actual {actual})",
                        record.sha256
                    ));
                }
            }
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(RegistryError::Integrity(problems.join("; ")))
    }
}

/// Phase 3: manifest identity records must match file contents.
fn verify_manifest_identity(
    manifest: &CatalogManifest,
    parsed_files: &[ParsedFile],
) -> Result<(), RegistryError> {
    let mut problems: Vec<String> = Vec::new();
    let declared: BTreeMap<&str, &CatalogEntry> = manifest
        .entries
        .iter()
        .map(|e| (e.path.as_str(), e))
        .collect();
    for file in parsed_files {
        if let Some(record) = declared.get(file.record.path.as_str()) {
            if record.model_id != file.record.model_id
                || record.entry_version != file.record.entry_version
                || EntryKind::from(record.kind) != file.parsed.kind()
            {
                problems.push(format!(
                    "{}: catalog.json identity ({:?} `{}` v{}) does not match file content ({:?} `{}` v{})",
                    file.record.path,
                    EntryKind::from(record.kind),
                    record.model_id,
                    record.entry_version,
                    file.parsed.kind(),
                    file.record.model_id,
                    file.record.entry_version
                ));
            }
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(RegistryError::Integrity(problems.join("; ")))
    }
}

/// Final manifest for a tree: the declared one, or a synthesized manifest
/// whose hashes come from the on-disk content: a directory
/// catalog without `catalog.json` is hashed from disk. The bool reports
/// whether `catalog_sha256` must still be verified.
fn build_manifest(
    manifest_opt: Option<CatalogManifest>,
    parsed_files: &[ParsedFile],
) -> (CatalogManifest, bool) {
    if let Some(mut manifest) = manifest_opt {
        manifest.entries = parsed_files.iter().map(|f| f.record.clone()).collect();
        return (manifest, true);
    }
    let mut entries: Vec<CatalogEntry> = parsed_files.iter().map(|f| f.record.clone()).collect();
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let hash = catalog_sha256(&entries.iter().collect::<Vec<_>>());
    let manifest = CatalogManifest {
        registry_version: "0.0.0-external".to_owned(),
        schema_version: crate::types::SCHEMA_VERSION.to_owned(),
        entries,
        catalog_sha256: hash,
    };
    (manifest, false)
}

/// Load one tree end to end: integrity, parse, identity, duplicates, and
/// per-entry semantic checks. Cross-reference checks are left to the
/// caller (they run on the assembled registry, which for [`Registry::load`]
/// is the layered union of two trees).
fn load_entries(source: &EntrySource<'_>) -> Result<(LoadedTree, Vec<Violation>), RegistryError> {
    let (manifest_opt, files) = read_tree(source)?;
    if let Some(manifest) = &manifest_opt {
        verify_file_hashes(manifest, &files)?;
    }

    // Phase 2: parse into the typed schema targets. The kind comes from the
    // entry's directory layout.
    let mut parsed_files: Vec<ParsedFile> = Vec::with_capacity(files.len());
    for (path, bytes) in &files {
        let kind = path
            .split('/')
            .next()
            .and_then(EntryKind::from_dir)
            .ok_or_else(|| {
                RegistryError::Integrity(format!("{path}: entry not under a known kind directory"))
            })?;
        let parsed = parse_entry(kind, path, bytes)?;
        parsed_files.push(ParsedFile {
            record: CatalogEntry {
                path: path.clone(),
                kind: kind.into(),
                model_id: parsed.model_id().to_owned(),
                entry_version: parsed.entry_version().to_owned(),
                sha256: hex_sha256(bytes),
            },
            parsed,
        });
    }

    if let Some(manifest) = &manifest_opt {
        verify_manifest_identity(manifest, &parsed_files)?;
    }

    // Phase 4: duplicate (kind, model_id) keys within one tree.
    let mut seen: BTreeMap<(EntryKind, &str), &str> = BTreeMap::new();
    for file in &parsed_files {
        let key = (file.parsed.kind(), file.parsed.model_id());
        if seen.insert(key, &file.record.path).is_some() {
            return Err(RegistryError::Duplicate {
                kind: file.parsed.kind(),
                model_id: file.parsed.model_id().to_owned(),
            });
        }
    }

    // Phase 5: per-entry semantic validation (all violations collected).
    let mut violations = Vec::new();
    for file in &parsed_files {
        violations.extend(file.parsed.check(&file.record.path));
    }

    let (manifest, verify_catalog_hash) = build_manifest(manifest_opt, &parsed_files);
    Ok((
        LoadedTree {
            manifest,
            files: parsed_files,
            verify_catalog_hash,
        },
        violations,
    ))
}

/// Parse one entry file into its typed target, dispatched by directory kind.
fn parse_entry(kind: EntryKind, path: &str, bytes: &[u8]) -> Result<ParsedEntry, RegistryError> {
    let parse = |bytes: &[u8]| -> Result<ParsedEntry, serde_json::Error> {
        Ok(match kind {
            EntryKind::Battery => ParsedEntry::Battery(Box::new(serde_json::from_slice(bytes)?)),
            EntryKind::Inverter => ParsedEntry::Inverter(Box::new(serde_json::from_slice(bytes)?)),
            EntryKind::Controller => {
                ParsedEntry::Controller(Box::new(serde_json::from_slice(bytes)?))
            }
            EntryKind::PvPreset => ParsedEntry::PvPreset(Box::new(serde_json::from_slice(bytes)?)),
        })
    };
    parse(bytes).map_err(|source| RegistryError::Parse {
        path: path.to_owned(),
        source,
    })
}

/// Assemble a registry from a loaded tree: build the lookup maps, run the
/// cross-reference checks, and verify the whole-catalog hash.
fn finalize(
    tree: LoadedTree,
    mut violations: Vec<Violation>,
    source: RegistrySource,
) -> Result<Registry, RegistryError> {
    let mut registry = Registry {
        manifest: tree.manifest,
        batteries: BTreeMap::new(),
        inverters: BTreeMap::new(),
        controllers: BTreeMap::new(),
        pv_presets: BTreeMap::new(),
        source,
    };
    let mut records: Vec<CatalogEntry> = Vec::with_capacity(tree.files.len());
    for file in tree.files {
        records.push(file.record);
        match file.parsed {
            ParsedEntry::Battery(m) => {
                registry.batteries.insert(m.model_id.clone(), *m);
            }
            ParsedEntry::Inverter(m) => {
                registry.inverters.insert(m.model_id.clone(), *m);
            }
            ParsedEntry::Controller(m) => {
                registry.controllers.insert(m.model_id.clone(), *m);
            }
            ParsedEntry::PvPreset(m) => {
                registry.pv_presets.insert(m.preset_id.clone(), *m);
            }
        }
    }

    violations.extend(validate::check_cross_references(&registry));
    if !violations.is_empty() {
        return Err(RegistryError::Validation { violations });
    }

    if tree.verify_catalog_hash {
        records.sort_by(|a, b| a.path.cmp(&b.path));
        let computed = catalog_sha256(&records.iter().collect::<Vec<_>>());
        if computed != registry.manifest.catalog_sha256 {
            return Err(RegistryError::Integrity(format!(
                "catalog_sha256 mismatch: catalog.json {}, computed {computed}",
                registry.manifest.catalog_sha256
            )));
        }
    }

    Ok(registry)
}

impl Registry {
    /// Load the build-time embedded catalog. Panics-free: all errors are
    /// returned as [`RegistryError`].
    ///
    /// # Errors
    /// Returns parse, validation, or integrity errors describing every
    /// offending entry.
    pub fn embedded() -> Result<Self, RegistryError> {
        let (tree, violations) = load_entries(&EntrySource::Embedded)?;
        finalize(tree, violations, RegistrySource::Embedded)
    }

    /// Load a catalog tree from a directory on disk (no embedded layer).
    /// When the directory has no `catalog.json`, a manifest is synthesized
    /// from on-disk content hashes.
    ///
    /// # Errors
    /// As [`Registry::embedded`], plus I/O errors.
    pub fn from_dir(dir: &Path) -> Result<Self, RegistryError> {
        let (tree, violations) = load_entries(&EntrySource::Disk(dir))?;
        finalize(
            tree,
            violations,
            RegistrySource::External {
                dir: dir.display().to_string(),
                shadowed: Vec::new(),
            },
        )
    }

    /// Load the embedded catalog, then shadow entries from `dir` when it is
    /// `Some` (CLI `--registry-dir` / env `SIM_REGISTRY_DIR` / legacy
    /// `BATSIM_REGISTRY_DIR`; resolution happens in the caller). Every
    /// shadowed entry is logged via `tracing`.
    ///
    /// # Errors
    /// As [`Registry::embedded`].
    pub fn load(shadow_dir: Option<&Path>) -> Result<Self, RegistryError> {
        let Some(dir) = shadow_dir else {
            return Self::embedded();
        };
        let mut base = Self::embedded()?;
        let (ext_tree, ext_violations) = load_entries(&EntrySource::Disk(dir))?;
        if !ext_violations.is_empty() {
            return Err(RegistryError::Validation {
                violations: ext_violations,
            });
        }
        // Whole-manifest integrity on the shadow tree, matching
        // `from_dir`/`embedded` (per-file hashes were already checked in
        // `load_entries`).
        if ext_tree.verify_catalog_hash {
            let mut records: Vec<&CatalogEntry> =
                ext_tree.files.iter().map(|f| &f.record).collect();
            records.sort_by(|a, b| a.path.cmp(&b.path));
            let computed = catalog_sha256(&records);
            if computed != ext_tree.manifest.catalog_sha256 {
                return Err(RegistryError::Integrity(format!(
                    "catalog_sha256 mismatch: catalog.json {}, computed {computed}",
                    ext_tree.manifest.catalog_sha256
                )));
            }
        }

        // Layer external entries over the embedded catalog, entry-by-entry
        // on (kind, model_id).
        let mut shadowed: Vec<String> = Vec::new();
        let mut ext_records: Vec<CatalogEntry> = Vec::new();
        for file in ext_tree.files {
            let kind = file.parsed.kind();
            let key = file.parsed.model_id().to_owned();
            ext_records.push(file.record);
            let replaced = match file.parsed {
                ParsedEntry::Battery(m) => base.batteries.insert(key.clone(), *m).is_some(),
                ParsedEntry::Inverter(m) => base.inverters.insert(key.clone(), *m).is_some(),
                ParsedEntry::Controller(m) => base.controllers.insert(key.clone(), *m).is_some(),
                ParsedEntry::PvPreset(m) => base.pv_presets.insert(key.clone(), *m).is_some(),
            };
            if replaced {
                tracing::warn!(
                    kind = ?kind,
                    model_id = %key,
                    dir = %dir.display(),
                    "registry entry shadowed by external catalog"
                );
                shadowed.push(format!("{kind:?}:{key}"));
            } else {
                tracing::info!(
                    kind = ?kind,
                    model_id = %key,
                    dir = %dir.display(),
                    "registry entry added from external catalog"
                );
            }
        }

        // Rebuild the manifest for the layered union: embedded records win
        // unless their (kind, model_id) was shadowed; external records are
        // appended. `catalog_sha256` is recomputed over the union.
        let shadow_keys: BTreeSet<&str> = shadowed.iter().map(String::as_str).collect();
        base.manifest.entries.retain(|e| {
            !shadow_keys.contains(format!("{:?}:{}", EntryKind::from(e.kind), e.model_id).as_str())
        });
        base.manifest.entries.extend(ext_records);
        base.manifest.entries.sort_by(|a, b| a.path.cmp(&b.path));
        base.manifest.catalog_sha256 =
            catalog_sha256(&base.manifest.entries.iter().collect::<Vec<_>>());

        base.source = RegistrySource::External {
            dir: dir.display().to_string(),
            shadowed,
        };

        let violations = validate::check_cross_references(&base);
        if !violations.is_empty() {
            return Err(RegistryError::Validation { violations });
        }
        Ok(base)
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

    /// Test-only constructor: build a registry directly from typed entries,
    /// bypassing the catalog files, integrity checks, and validation. Lets
    /// `system.rs` unit tests compose `HomeSystem` scenarios without the
    /// embedded catalog. The manifest is trivial and PV presets are empty.
    #[cfg(test)]
    #[must_use]
    pub fn from_parts(
        batteries: Vec<BatteryModel>,
        inverters: Vec<InverterModel>,
        controllers: Vec<ControllerModel>,
    ) -> Self {
        let manifest = CatalogManifest {
            registry_version: "0.0.0-test".to_owned(),
            schema_version: crate::types::SCHEMA_VERSION.to_owned(),
            entries: Vec::new(),
            catalog_sha256: String::new(),
        };
        Self {
            manifest,
            batteries: batteries
                .into_iter()
                .map(|m| (m.model_id.clone(), m))
                .collect(),
            inverters: inverters
                .into_iter()
                .map(|m| (m.model_id.clone(), m))
                .collect(),
            controllers: controllers
                .into_iter()
                .map(|m| (m.model_id.clone(), m))
                .collect(),
            pv_presets: BTreeMap::new(),
            source: RegistrySource::Embedded,
        }
    }
}

#[cfg(test)]
// unwrap/expect: test assertions may abort on fixture setup. float_cmp:
// catalog constants round-trip JSON bit-exactly, so exact equality against
// the same literal is the intended assertion.
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::float_cmp)]
mod tests {
    use super::*;
    use crate::types::{Chemistry, Coupling, InverterTopology, Provenance};
    use std::path::PathBuf;

    /// RAII tempdir for on-disk catalog copies (no `tempfile` dependency in
    /// the workspace; std-only).
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("batsim-registry-test-{}-{tag}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Materialize the embedded catalog tree into a tempdir.
    fn copy_embedded_to(tmp: &TempDir) {
        let mut all = Vec::new();
        walk_embedded(&EMBEDDED, &mut all);
        for file in all {
            let dest = tmp.path().join(file.path());
            std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
            std::fs::write(&dest, file.contents()).unwrap();
        }
    }

    #[test]
    fn embedded_catalog_loads_and_counts() {
        let registry = Registry::embedded().expect("embedded catalog must load");
        assert_eq!(registry.batteries().count(), 11);
        assert_eq!(registry.inverters().count(), 5);
        assert_eq!(registry.controllers().count(), 4);
        assert_eq!(registry.pv_presets().count(), 1);
        assert_eq!(registry.manifest().registry_version, "1.0.0");
        assert_eq!(registry.manifest().schema_version, "1.0.0");
        assert_eq!(registry.manifest().entries.len(), 21);
        assert_eq!(registry.manifest().catalog_sha256.len(), 64);
        assert_eq!(registry.source(), &RegistrySource::Embedded);
    }

    #[test]
    fn spec_nameplate_values_and_provenance() {
        let r = Registry::embedded().unwrap();

        let pw2 = r.battery("tesla.powerwall_2").unwrap();
        assert_eq!(pw2.nameplate_energy_kwh.value, 14.0);
        assert_eq!(pw2.nameplate_energy_kwh.provenance, Provenance::Spec);
        assert_eq!(pw2.usable_energy_kwh.value, 13.5);
        assert_eq!(pw2.continuous_discharge_power_kw.value, 5.0);
        assert_eq!(pw2.peak_discharge_power_kw.as_ref().unwrap().value, 7.0);
        assert_eq!(pw2.chemistry, Chemistry::NMC);
        assert_eq!(pw2.rte_ac_coupled.as_ref().unwrap().value, 0.90);
        assert_eq!(
            pw2.rte_ac_coupled.as_ref().unwrap().provenance,
            Provenance::Spec
        );

        let pw3 = r.battery("tesla.powerwall_3").unwrap();
        assert_eq!(pw3.nameplate_energy_kwh.value, 13.5);
        assert_eq!(pw3.continuous_discharge_power_kw.value, 11.5);
        assert_eq!(pw3.chemistry, Chemistry::LFP);

        let pack = r.battery("tesla.pw3_expansion_pack").unwrap();
        assert_eq!(pack.usable_energy_kwh.value, 13.5);
        assert_eq!(pack.continuous_discharge_power_kw.value, 0.0);
        assert_eq!(pack.integrated_inverter, Some(false));
        assert_eq!(pack.requires_controller_id, None);

        let five_p = r.battery("enphase.iq_battery_5p").unwrap();
        assert_eq!(five_p.nameplate_energy_kwh.value, 5.0);
        assert_eq!(five_p.usable_energy_kwh.value, 4.96);
        assert_eq!(five_p.microinverter_count, Some(6));
        assert_eq!(
            five_p.power_per_microinverter_kw.as_ref().unwrap().value,
            0.64
        );

        let iq10 = r.battery("enphase.iq_battery_10").unwrap();
        assert_eq!(iq10.nameplate_energy_kwh.value, 10.08);
        assert_eq!(iq10.microinverter_count, Some(12));
        assert_eq!(iq10.continuous_discharge_power_kw.value, 3.84);

        let iq10c = r.battery("enphase.iq_battery_10c").unwrap();
        assert_eq!(iq10c.nameplate_energy_kwh.value, 10.0);
        assert_eq!(iq10c.continuous_discharge_power_kw.value, 7.08);

        let se = r.battery("solaredge.home_battery_400v").unwrap();
        assert_eq!(se.nameplate_energy_kwh.value, 10.3);
        assert_eq!(se.nameplate_energy_kwh.provenance, Provenance::Estimated);
        assert_eq!(se.usable_energy_kwh.value, 9.7);
        assert_eq!(se.usable_energy_kwh.provenance, Provenance::Spec);
        assert_eq!(se.rte_pv_coupled.as_ref().unwrap().value, 0.945);
        assert_eq!(
            se.rte_pv_coupled.as_ref().unwrap().provenance,
            Provenance::Spec
        );
        assert_eq!(
            se.requires_controller_id.as_deref(),
            Some("solaredge.backup_interface")
        );

        let eco = r.battery("sonnen.ecolinx").unwrap();
        assert_eq!(eco.usable_energy_kwh.value, 20.0);
        assert_eq!(eco.continuous_discharge_power_kw.value, 8.0);
        assert_eq!(eco.chemistry, Chemistry::LFP);

        let core = r.battery("sonnen.sonnencore_plus").unwrap();
        assert_eq!(core.usable_energy_kwh.value, 10.0);
        assert_eq!(core.continuous_discharge_power_kw.value, 4.8);

        for id in [
            "sonnen.sonnenbatterie_10_ac",
            "sonnen.sonnenbatterie_10_hybrid",
        ] {
            let sb = r.battery(id).unwrap();
            assert_eq!(sb.usable_energy_kwh.value, 11.0);
            assert_eq!(sb.continuous_discharge_power_kw.value, 8.0);
            assert_eq!(sb.chemistry, Chemistry::LFP);
        }
        let hybrid = r.battery("sonnen.sonnenbatterie_10_hybrid").unwrap();
        assert_eq!(hybrid.rte_pv_coupled.as_ref().unwrap().value, 0.96);
        assert_eq!(
            hybrid.rte_pv_coupled.as_ref().unwrap().provenance,
            Provenance::Spec
        );

        // Every catalog efficiency curve is estimated (including the
        // verbatim charge and discharge curves, whose given provenance is
        // estimated).
        for battery in r.batteries() {
            assert_eq!(
                battery.charge_efficiency_curve.provenance,
                Provenance::Estimated
            );
            assert_eq!(
                battery.discharge_efficiency_curve.provenance,
                Provenance::Estimated
            );
            assert!(battery.charge_efficiency_curve.points.len() >= 2);
        }
    }

    /// Normative calibration: the AC-path round trip at the 0.5C power
    /// point lands within 0.5 percentage points of each battery's declared
    /// `rte_ac_coupled`. AC-coupled entries:
    /// `eta_chg x eta_coul x eta_dis`. DC-coupled hybrids: same product
    /// times `eta_hyb^2` from the claiming hybrid inverter (grid charge on
    /// hybrids is a double conversion).
    #[test]
    fn ac_path_rte_calibration_holds() {
        let r = Registry::embedded().unwrap();
        for battery in r.batteries() {
            let rte_ac = battery.rte_ac_coupled.as_ref().unwrap().value;
            let eta_coul = match battery.chemistry {
                Chemistry::LFP => 0.99,
                Chemistry::NMC | Chemistry::NCA => 0.98,
            };
            let p_star = 0.5 * battery.usable_energy_kwh.value;
            let eta_chg = battery.charge_efficiency_curve.eval(p_star);
            let eta_dis = battery.discharge_efficiency_curve.eval(p_star);
            let battery_product = eta_chg * eta_coul * eta_dis;
            let rte = if battery.coupling == Coupling::DCCoupledHybrid {
                let inverter = r
                    .inverters()
                    .find(|inv| {
                        inv.topology == InverterTopology::HybridDCCoupled
                            && inv.compatible_battery_ids.contains(&battery.model_id)
                    })
                    .unwrap_or_else(|| panic!("{}: no claiming hybrid inverter", battery.model_id));
                let eta_hyb = inverter.efficiency_curve.eval(p_star);
                battery_product * eta_hyb * eta_hyb
            } else {
                battery_product
            };
            assert!(
                (rte - rte_ac).abs() <= 0.005,
                "{}: AC-path RTE {rte:.4} not within ±0.5pp of rte_ac_coupled {rte_ac}",
                battery.model_id
            );
        }
    }

    #[test]
    fn tampered_entry_fails_integrity_and_names_file() {
        let tmp = TempDir::new("tamper");
        copy_embedded_to(&tmp);
        let target = tmp.path().join("batteries/tesla_powerwall_2.json");
        let bytes = std::fs::read(&target).unwrap();
        // Flip one byte: "14.0" -> "94.0" (JSON stays valid).
        let needle = b"14.0";
        let pos = bytes
            .windows(needle.len())
            .position(|w| w == needle)
            .expect("nameplate value present");
        let mut tampered = bytes;
        tampered[pos] = b'9';
        std::fs::write(&target, tampered).unwrap();

        match Registry::from_dir(tmp.path()) {
            Err(RegistryError::Integrity(msg)) => {
                assert!(
                    msg.contains("batteries/tesla_powerwall_2.json"),
                    "integrity error must enumerate the file, got: {msg}"
                );
            }
            Err(other) => panic!("expected integrity error, got: {other}"),
            Ok(_) => panic!("expected integrity error, got Ok"),
        }
    }

    #[test]
    fn tampered_catalog_hash_fails_integrity() {
        let tmp = TempDir::new("tamper-catalog");
        copy_embedded_to(&tmp);
        let target = tmp.path().join("catalog.json");
        let manifest = std::fs::read_to_string(&target).unwrap();
        // Corrupt the whole-catalog hash (first hex digit of catalog_sha256).
        let pos = manifest
            .find("\"catalog_sha256\": \"")
            .expect("field present")
            + 19;
        let mut bytes = manifest.into_bytes();
        bytes[pos] = if bytes[pos] == b'0' { b'1' } else { b'0' };
        std::fs::write(&target, String::from_utf8(bytes).unwrap()).unwrap();

        match Registry::from_dir(tmp.path()) {
            Err(RegistryError::Integrity(msg)) => {
                assert!(msg.contains("catalog_sha256"), "got: {msg}");
            }
            Err(other) => panic!("expected integrity error, got: {other}"),
            Ok(_) => panic!("expected integrity error, got Ok"),
        }
    }

    #[test]
    fn shadow_dir_overrides_one_entry_and_keeps_others() {
        let tmp = TempDir::new("shadow");
        let batteries = tmp.path().join("batteries");
        std::fs::create_dir_all(&batteries).unwrap();
        // External tree contains only a re-rated PW2 and no catalog.json:
        // the manifest is synthesized from disk content.
        let shadow_json = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../registry/batteries/tesla_powerwall_2.json"),
        )
        .unwrap()
        .replace("\"value\": 14.0", "\"value\": 99.0");
        std::fs::write(batteries.join("tesla_powerwall_2.json"), shadow_json).unwrap();

        let registry = Registry::load(Some(tmp.path())).expect("shadow load must succeed");
        assert_eq!(
            registry
                .battery("tesla.powerwall_2")
                .unwrap()
                .nameplate_energy_kwh
                .value,
            99.0,
            "shadow entry must win"
        );
        assert_eq!(
            registry
                .battery("tesla.powerwall_3")
                .unwrap()
                .nameplate_energy_kwh
                .value,
            13.5,
            "untouched entries survive shadowing"
        );
        assert_eq!(registry.batteries().count(), 11);
        match registry.source() {
            RegistrySource::External { shadowed, .. } => {
                assert_eq!(shadowed.len(), 1);
                assert!(
                    shadowed[0].contains("tesla.powerwall_2"),
                    "got {shadowed:?}"
                );
            }
            RegistrySource::Embedded => panic!("expected External source, got Embedded"),
        }
    }

    #[test]
    fn from_dir_without_manifest_synthesizes_one() {
        let tmp = TempDir::new("synth");
        let batteries = tmp.path().join("batteries");
        std::fs::create_dir_all(&batteries).unwrap();
        // ecoLinx has no outbound cross-references, so a single-entry tree
        // passes the cross-reference validation standalone.
        std::fs::copy(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../registry/batteries/sonnen_ecolinx.json"),
            batteries.join("sonnen_ecolinx.json"),
        )
        .unwrap();
        let registry = Registry::from_dir(tmp.path()).expect("single-entry tree must load");
        assert_eq!(registry.batteries().count(), 1);
        assert_eq!(registry.manifest().entries.len(), 1);
        assert_eq!(registry.manifest().catalog_sha256.len(), 64);
        assert!(matches!(registry.source(), RegistrySource::External { .. }));
    }

    #[test]
    fn load_without_shadow_dir_is_embedded() {
        let registry = Registry::load(None).unwrap();
        assert_eq!(registry.source(), &RegistrySource::Embedded);
        assert_eq!(registry.batteries().count(), 11);
    }
}
