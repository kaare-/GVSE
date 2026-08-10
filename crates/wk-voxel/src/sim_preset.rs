//! Named Tab-menu presets (JSON under `presets/`).
//!
//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Presets capture live-tunable sim knobs so a soak / experiment setup
//! can be saved, shared, and reloaded without regenerating the world.
//! Worldgen size / seed are intentionally excluded (those need Regenerate).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use wk_material::{MaterialId, MaterialRegistry, MATERIAL_COUNT};

use crate::carbon::CarbonConfig;
use crate::climate::ClimateConfig;
use crate::clouds::CloudConfig;
use crate::failure::FailureConfig;
use crate::fungi::FungiConfig;
use crate::phase::PhaseConfig;
use crate::rules::{
    CondensationConfig, EvapConfig, GrainConfig, KarstConfig, OrographicConfig, PerfConfig,
    RainConfig,
};
use crate::spore_bank::SporeBankConfig;
use crate::temperature::TempConfig;

/// Directory under the process cwd for named presets.
pub const PRESET_DIR: &str = "presets";
/// File extension for Tab presets.
pub const PRESET_EXT: &str = "json";
/// Bump when the JSON shape changes incompatibly.
pub const PRESET_SCHEMA_VERSION: u32 = 1;

/// Plant / fungus gene defaults (Tab → Plants), without full [`crate::Genome`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PlantGenePreset {
    pub alloc_stem: f32,
    pub alloc_leaf: f32,
    pub alloc_root: f32,
    pub root_depth_bias: f32,
    pub leaf_absorb: f32,
    pub shade_efficiency: f32,
    pub digest_rate: f32,
    pub clone_fidelity: f32,
}

impl Default for PlantGenePreset {
    fn default() -> Self {
        let g = crate::Genome::default();
        Self {
            alloc_stem: g.alloc_stem,
            alloc_leaf: g.alloc_leaf,
            alloc_root: g.alloc_root,
            root_depth_bias: g.root_depth_bias,
            leaf_absorb: g.leaf_absorb,
            shade_efficiency: g.shade_efficiency,
            digest_rate: g.digest_rate,
            clone_fidelity: g.clone_fidelity,
        }
    }
}

/// Serializable snapshot of Tab live knobs (not worldgen / not cell state).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SimPreset {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    /// Optional human note shown in JSON / UI status.
    #[serde(default)]
    pub notes: String,
    pub rain: RainConfig,
    pub evap: EvapConfig,
    pub cond: CondensationConfig,
    pub oro: OrographicConfig,
    pub karst: KarstConfig,
    pub cloud: CloudConfig,
    pub climate: ClimateConfig,
    pub temp: TempConfig,
    pub phase: PhaseConfig,
    pub grain: GrainConfig,
    pub fungi: FungiConfig,
    pub carbon: CarbonConfig,
    pub spore_bank: SporeBankConfig,
    pub perf: PerfConfig,
    pub failure: FailureConfig,
    pub wind_vx: f32,
    pub wind_variance: f32,
    pub humidity_diffusion_alpha: f32,
    pub plant_genes: PlantGenePreset,
    pub max_atoms: f32,
    pub max_corpses: f32,
    pub max_roots: f32,
    pub max_stems: f32,
    pub max_photos: f32,
    #[serde(default = "default_mat_perm")]
    pub mat_perm: [f32; MATERIAL_COUNT],
    #[serde(default = "default_mat_poro")]
    pub mat_poro: [f32; MATERIAL_COUNT],
}

fn default_schema_version() -> u32 {
    PRESET_SCHEMA_VERSION
}

fn default_mat_perm() -> [f32; MATERIAL_COUNT] {
    let mut out = [0.0f32; MATERIAL_COUNT];
    for id in MaterialId::ALL_SOLIDS {
        out[id as usize] = MaterialRegistry::base_props(id).permeability as f32;
    }
    out
}

fn default_mat_poro() -> [f32; MATERIAL_COUNT] {
    let mut out = [0.0f32; MATERIAL_COUNT];
    for id in MaterialId::ALL_SOLIDS {
        out[id as usize] = MaterialRegistry::base_props(id).porosity as f32;
    }
    out
}

impl Default for SimPreset {
    fn default() -> Self {
        Self::tab_defaults()
    }
}

impl SimPreset {
    /// Matches current Tab boot defaults (wetter clouds, gentler compost).
    pub fn tab_defaults() -> Self {
        let mut cond = CondensationConfig::default();
        cond.min_mass_to_rain = 140.0;
        cond.max_prob_per_tick = 0.10;
        cond.mass_per_droplet = 40.0;

        let mut cloud = CloudConfig::default();
        cloud.coag_rate = 0.08;
        cloud.coag_max_take = 18.0;
        cloud.cloud_alt_above_sea = 48;
        cloud.coag_min_above_sea = 22;
        cloud.buoyant_rise = 0.10;
        cloud.rain_cells_per_tick = 3;

        let mut rain = RainConfig::default();
        rain.closed_loop = true;
        rain.prob_per_col_per_tick = 0.02;
        rain.droplet_sat = 64;
        rain.max_flood_above_sea = 12;

        Self {
            schema_version: PRESET_SCHEMA_VERSION,
            notes: "Tab boot defaults".into(),
            rain,
            evap: EvapConfig {
                rate_per_tick: 1,
                dry_above_max: 200,
                period_ticks: 5,
            },
            cond,
            oro: OrographicConfig::default(),
            karst: KarstConfig::default(),
            cloud,
            climate: ClimateConfig::default(),
            temp: TempConfig::default(),
            phase: PhaseConfig::default(),
            grain: GrainConfig::default(),
            fungi: FungiConfig {
                soil_mycelium_threshold: 160,
                soil_convert_odds: 1_600,
            },
            carbon: CarbonConfig::default(),
            spore_bank: SporeBankConfig {
                germinate_odds: 4,
                max_age_ticks: 320_000,
                ..SporeBankConfig::default()
            },
            perf: PerfConfig::default(),
            failure: FailureConfig::default(),
            wind_vx: 0.05,
            wind_variance: 0.55,
            humidity_diffusion_alpha: 0.15,
            plant_genes: PlantGenePreset::default(),
            max_atoms: crate::MAX_ATOMS as f32,
            max_corpses: crate::MAX_CORPSES as f32,
            max_roots: crate::MAX_ROOT_MODULES as f32,
            max_stems: crate::MAX_STEM_MODULES as f32,
            max_photos: crate::MAX_PHOTO_MODULES as f32,
            mat_perm: default_mat_perm(),
            mat_poro: default_mat_poro(),
        }
    }

    /// Knobs from the 1M sim-log soak that kept plants alive continuously.
    ///
    /// Note: the soak harness also injects a small ocean humidity flux that
    /// is not a Tab knob — reload this for the Tab-side half of that setup.
    pub fn soak_survival() -> Self {
        let mut p = Self::tab_defaults();
        p.notes = "1M soak survival (wetter clouds, easier spore bank, Tab-like evap)"
            .into();
        p.evap = EvapConfig {
            rate_per_tick: 1,
            dry_above_max: 200,
            period_ticks: 5,
        };
        p.cloud.coag_rate = 0.12;
        p.cloud.coag_max_take = 22.0;
        p.cloud.cloud_alt_above_sea = 28;
        p.cloud.coag_min_above_sea = 14;
        p.cloud.buoyant_rise = 0.12;
        p.cloud.downpour_mass = 160.0;
        p.cloud.rain_cells_per_tick = 4;
        p.cloud.max_parcels = 40;
        p.cond.min_mass_to_rain = 72.0;
        p.cond.max_prob_per_tick = 0.25;
        p.cond.mass_per_droplet = 72.0;
        p.spore_bank.germinate_odds = 3;
        p.spore_bank.max_age_ticks = 500_000;
        p.spore_bank.max_total = 640;
        p.fungi.soil_convert_odds = 2_000;
        p.fungi.soil_mycelium_threshold = 160;
        p.wind_vx = 0.14;
        p.wind_variance = 0.55;
        p.humidity_diffusion_alpha = 0.15;
        p
    }

    pub fn to_json_pretty(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }

    pub fn from_json(s: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(s)
    }
}

/// Built-in presets always available without a disk file.
pub fn builtin_preset_names() -> &'static [&'static str] {
    &["soak-survival", "tab-defaults"]
}

pub fn load_builtin_preset(name: &str) -> Option<SimPreset> {
    match sanitize_preset_name(name).as_deref() {
        Some("soak-survival") => Some(SimPreset::soak_survival()),
        Some("tab-defaults") => Some(SimPreset::tab_defaults()),
        _ => None,
    }
}

/// Keep names filesystem-safe: lowercase, `[a-z0-9_-]`, 1..=48 chars.
pub fn sanitize_preset_name(raw: &str) -> Option<String> {
    let s = raw.trim().to_ascii_lowercase();
    if s.is_empty() || s.len() > 48 {
        return None;
    }
    if !s
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
    {
        return None;
    }
    if s.starts_with('-') || s.starts_with('_') {
        return None;
    }
    Some(s)
}

pub fn preset_path(name: &str) -> Option<PathBuf> {
    let name = sanitize_preset_name(name)?;
    Some(PathBuf::from(PRESET_DIR).join(format!("{name}.{PRESET_EXT}")))
}

pub fn ensure_preset_dir() -> io::Result<()> {
    fs::create_dir_all(PRESET_DIR)
}

/// Save a named preset as pretty JSON under [`PRESET_DIR`].
pub fn save_preset(name: &str, preset: &SimPreset) -> io::Result<PathBuf> {
    let path = preset_path(name).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "preset name must be 1..=48 chars of [a-z0-9_-]",
        )
    })?;
    ensure_preset_dir()?;
    let json = preset
        .to_json_pretty()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(&path, json)?;
    Ok(path)
}

/// Load a named preset: disk file first, then built-in.
pub fn load_preset(name: &str) -> io::Result<SimPreset> {
    if let Some(path) = preset_path(name) {
        if path.is_file() {
            let s = fs::read_to_string(&path)?;
            return SimPreset::from_json(&s)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e));
        }
    }
    load_builtin_preset(name).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("preset `{name}` not found in {PRESET_DIR}/ or builtins"),
        )
    })
}

/// Disk preset names (sorted), excluding extension.
pub fn list_disk_presets() -> Vec<String> {
    let Ok(rd) = fs::read_dir(PRESET_DIR) else {
        return Vec::new();
    };
    let mut names = Vec::new();
    for entry in rd.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some(PRESET_EXT) {
            continue;
        }
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            if let Some(name) = sanitize_preset_name(stem) {
                names.push(name);
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

/// Built-ins first, then disk names not already listed.
pub fn list_all_presets() -> Vec<String> {
    let mut out: Vec<String> = builtin_preset_names()
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    for name in list_disk_presets() {
        if !out.iter().any(|n| n == &name) {
            out.push(name);
        }
    }
    out
}

/// Write `presets/<name>.json` from a [`Path`] (test helper / tooling).
pub fn write_preset_file(path: &Path, preset: &SimPreset) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = preset
        .to_json_pretty()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(path, json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_rejects_bad_names() {
        assert!(sanitize_preset_name("").is_none());
        assert!(sanitize_preset_name("../x").is_none());
        assert!(sanitize_preset_name("Has Space").is_none());
        assert!(sanitize_preset_name("-leading").is_none());
        assert_eq!(
            sanitize_preset_name("Soak-Survival").as_deref(),
            Some("soak-survival")
        );
    }

    #[test]
    fn soak_survival_roundtrip_json() {
        let p = SimPreset::soak_survival();
        let s = p.to_json_pretty().unwrap();
        let q = SimPreset::from_json(&s).unwrap();
        assert_eq!(p, q);
        assert!((q.cloud.coag_rate - 0.12).abs() < 1e-6);
        assert_eq!(q.evap.period_ticks, 5);
        assert_eq!(q.spore_bank.germinate_odds, 3);
        assert_eq!(q.fungi.soil_convert_odds, 2_000);
        assert!((q.wind_vx - 0.14).abs() < 1e-6);
    }

    #[test]
    fn builtins_resolve() {
        assert!(load_builtin_preset("soak-survival").is_some());
        assert!(load_builtin_preset("tab-defaults").is_some());
        assert!(load_builtin_preset("nope").is_none());
    }

    #[test]
    fn shipped_soak_survival_matches_builtin() {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../presets/soak-survival.json");
        let s = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("missing {}: {e}", path.display()));
        let disk = SimPreset::from_json(&s).unwrap();
        assert_eq!(disk, SimPreset::soak_survival());
    }
}
