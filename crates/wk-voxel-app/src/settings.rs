//! Live settings menu for wk-voxel-app (Tab to toggle).
//!
//! Isolation: wk-voxel + wk-material + macroquad only.

use macroquad::prelude::*;
use macroquad::ui::{hash, root_ui, widgets};
use wk_material::{MaterialId, MaterialRegistry, MATERIAL_COUNT};
use wk_voxel::{
    list_all_presets, load_preset, sanitize_preset_name, save_preset, CarbonBudget, CarbonConfig,
    ClimateConfig, CloudConfig, CompetentFallConfig, CondensationConfig, EvapConfig, FailureConfig,
    FungiConfig, Genome, GrainConfig, KarstConfig, OrographicConfig, PerfConfig, PhaseConfig,
    PlantGenePreset, PlantGrowthCaps, RainConfig, SimPreset, SporeBankConfig, TempConfig, World,
    WorldgenParams, CHUNK_CELLS_W, MAX_ATOMS, MAX_CORPSES, MAX_PHOTO_MODULES, MAX_ROOT_MODULES,
    MAX_STEM_MODULES, PRESET_DIR,
};

use crate::atmosphere::AtmosphereLookConfig;

/// Top-level Tab settings pages (keeps the long menu navigable).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsPage {
    World,
    Climate,
    Physics,
    Life,
}

/// Default plant / fungus gene knobs applied on spawn (and optionally to living plants).
#[derive(Debug, Clone)]
pub struct PlantGeneSettings {
    pub alloc_stem: f32,
    pub alloc_leaf: f32,
    pub alloc_root: f32,
    pub root_depth_bias: f32,
    pub leaf_absorb: f32,
    pub shade_efficiency: f32,
    pub digest_rate: f32,
    pub clone_fidelity: f32,
}

impl Default for PlantGeneSettings {
    fn default() -> Self {
        let g = Genome::default();
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

impl PlantGeneSettings {
    pub fn to_genome(&self) -> Genome {
        Genome {
            alloc_stem: self.alloc_stem.clamp(0.0, 1.0),
            alloc_leaf: self.alloc_leaf.clamp(0.0, 1.0),
            alloc_root: self.alloc_root.clamp(0.0, 1.0),
            root_depth_bias: self.root_depth_bias.clamp(0.0, 1.0),
            leaf_absorb: self.leaf_absorb.clamp(0.05, 1.0),
            shade_efficiency: self.shade_efficiency.clamp(0.0, 1.0),
            digest_rate: self.digest_rate.clamp(0.05, 2.0),
            clone_fidelity: self.clone_fidelity.clamp(0.05, 1.0),
            ..Genome::default()
        }
    }

    pub fn to_preset(&self) -> PlantGenePreset {
        PlantGenePreset {
            alloc_stem: self.alloc_stem,
            alloc_leaf: self.alloc_leaf,
            alloc_root: self.alloc_root,
            root_depth_bias: self.root_depth_bias,
            leaf_absorb: self.leaf_absorb,
            shade_efficiency: self.shade_efficiency,
            digest_rate: self.digest_rate,
            clone_fidelity: self.clone_fidelity,
        }
    }

    pub fn from_preset(p: &PlantGenePreset) -> Self {
        Self {
            alloc_stem: p.alloc_stem,
            alloc_leaf: p.alloc_leaf,
            alloc_root: p.alloc_root,
            root_depth_bias: p.root_depth_bias,
            leaf_absorb: p.leaf_absorb,
            shade_efficiency: p.shade_efficiency,
            digest_rate: p.digest_rate,
            clone_fidelity: p.clone_fidelity,
        }
    }
}

/// All live-tunable knobs for the voxel demo.
#[derive(Debug, Clone)]
pub struct SimSettings {
    pub open: bool,
    pub rain: RainConfig,
    pub evap: EvapConfig,
    pub cond: CondensationConfig,
    /// `C` — condensation / dew (the real rain). Default on.
    pub cond_rain_on: bool,
    /// `W` — extra climatic faucet. Default off.
    pub climatic_rain_on: bool,
    /// `E` — surface water → humidity. Default on.
    pub evap_on: bool,
    /// `K` — limestone + groundwater dissolve. Default on.
    pub karst_on: bool,
    pub oro: OrographicConfig,
    pub karst: KarstConfig,
    pub cloud: CloudConfig,
    pub climate: ClimateConfig,
    /// Sky / ridge / sun-moon cosmetics (Tab → Climate → Sky look).
    pub atmosphere: AtmosphereLookConfig,
    /// 0 = landscape only, 1 = heatmap only (when U/T/M/G overlays are on).
    pub heatmap_blend: f32,
    /// How dark a fully waterlogged cell renders (Tab → World → Look).
    /// Measured against the cell's own capacity, quantized so merged terrain
    /// runs survive.
    pub wet_darken: f32,
    /// Strength of the porous-rock stipple (0 = off). Only the upper pore
    /// buckets are marked, which the fracture tail keeps rare.
    pub pore_stipple: f32,
    pub temp: TempConfig,
    pub phase: PhaseConfig,
    pub grain: GrainConfig,
    /// Mycelium compost knobs (Tab → Life → Fungi / compost).
    pub fungi: FungiConfig,
    /// Crude CO₂ buckets (Tab → Life → Carbon).
    pub carbon: CarbonConfig,
    /// Hibernating spore bank (Tab → Life → Spore bank).
    pub spore_bank: SporeBankConfig,
    /// Which settings page is open.
    pub page: SettingsPage,
    /// Physics trade-offs (Tab → Performance). Defaults preserve water feel.
    pub perf: PerfConfig,
    /// Geotech failure (Tab → Geotech). Roof collapse on by default.
    pub failure: FailureConfig,
    /// Competent rock rigid-body knobs (Tab → Geotech).
    pub competent_fall: CompetentFallConfig,
    /// Scratch f32 for max roof events slider.
    pub max_roof_events: f32,
    /// Scratch f32 for max shear events slider.
    pub max_shear_events: f32,
    /// Scratch f32 for max compaction events slider.
    pub max_compaction_events: f32,
    /// Scratch f32 for shear chance (percent UI → per-mille).
    pub shear_chance_pct: f32,
    /// Scratch f32 for competent fall max drop / tick.
    pub competent_max_drop: f32,
    /// Scratch f32 for competent impact threshold (fall cells).
    pub competent_min_impact: f32,
    /// Scratch f32 for max slope rolls / tick.
    pub competent_max_rolls: f32,
    pub wind_vx: f32,
    /// Natural variance 0..1 — wind force and direction wander around the mean.
    pub wind_variance: f32,
    pub humidity_diffusion_alpha: f32,
    /// Scratch f32s for material range sliders (synced → world hydro overrides).
    pub mat_perm_min: [f32; MATERIAL_COUNT],
    pub mat_perm_max: [f32; MATERIAL_COUNT],
    pub mat_poro_min: [f32; MATERIAL_COUNT],
    pub mat_poro_max: [f32; MATERIAL_COUNT],
    /// Plant / fungus gene defaults (Tab → Plants).
    pub plant_genes: PlantGeneSettings,
    /// Set by UI when user clicks "Apply genes to living plants".
    pub apply_genes_to_living: bool,
    /// Living creature hard cap (Tab → Creatures). Synced onto OrganismStore.
    pub max_atoms: f32,
    /// Lingering corpse hard cap.
    pub max_corpses: f32,
    /// Per-plant Root / Stem / Photosystem pixel ceilings.
    pub max_roots: f32,
    pub max_stems: f32,
    pub max_photos: f32,
    /// Draft world size (chunks wide) — applied on Regenerate.
    pub world_width_chunks: f32,
    pub world_sea_level: f32,
    pub world_sky_ceiling: f32,
    /// Set by UI when user clicks "Regenerate world with size".
    pub request_regen: bool,
    /// Draft name for Save under `presets/<name>.json`.
    pub preset_name: String,
    /// Last save/load status line for the presets UI.
    pub preset_status: String,
}

impl SimSettings {
    pub fn new(params: &WorldgenParams) -> Self {
        let rain = RainConfig {
            top_y: params.sky_ceiling_y - 1,
            x_range: (0, params.width_cols - 1),
            prob_per_col_per_tick: 0.02,
            droplet_sat: 64,
            seed_salt: 0xC10D_5EED,
            closed_loop: true,
            sea_level_y: params.sea_level_y,
            max_flood_above_sea: 12,
        };
        let mut cond = CondensationConfig {
            top_y: params.sky_ceiling_y - 2,
            ..CondensationConfig::default()
        };
        // Match previous demo drizzle defaults.
        cond.min_mass_to_rain = 140.0;
        cond.max_prob_per_tick = 0.10;
        cond.mass_per_droplet = 40.0;

        let mut mat_perm_min = [0.0f32; MATERIAL_COUNT];
        let mut mat_perm_max = [0.0f32; MATERIAL_COUNT];
        let mut mat_poro_min = [0.0f32; MATERIAL_COUNT];
        let mut mat_poro_max = [0.0f32; MATERIAL_COUNT];
        for id in MaterialId::ALL_SOLIDS {
            let i = id as usize;
            let base = MaterialRegistry::hydrology(id);
            mat_perm_min[i] = base.permeability.min as f32;
            mat_perm_max[i] = base.permeability.max as f32;
            mat_poro_min[i] = base.porosity.min as f32;
            mat_poro_max[i] = base.porosity.max as f32;
        }

        Self {
            open: false,
            rain,
            evap: EvapConfig {
                rate_per_tick: 1,
                dry_above_max: 200,
                period_ticks: 5,
            },
            cond,
            cond_rain_on: true,
            climatic_rain_on: false,
            evap_on: true,
            karst_on: true,
            oro: OrographicConfig {
                seed: params.seed,
                width_cols: params.width_cols,
                sea_level_y: params.sea_level_y,
                ..OrographicConfig::default()
            },
            karst: KarstConfig::default(),
            cloud: {
                // Slightly wetter sky defaults so lakes refill overnight
                // without requiring Tab fiddling on every new world.
                let mut c = CloudConfig::default();
                c.cloud_alt_above_sea = 48;
                c.coag_min_above_sea = 22;
                c.buoyant_rise = 0.10;
                c
            },
            climate: ClimateConfig::default(),
            atmosphere: AtmosphereLookConfig::default(),
            heatmap_blend: 0.55,
            wet_darken: crate::palette::WET_DARKEN_DEFAULT,
            pore_stipple: 0.35,
            temp: TempConfig::default(),
            phase: PhaseConfig::default(),
            grain: GrainConfig::default(),
            fungi: FungiConfig {
                // Slower than crate default so Organic cream beds linger
                // under living roots (Tab can still speed compost up).
                soil_mycelium_threshold: 160,
                soil_convert_odds: 1_600,
            },
            carbon: CarbonConfig::default(),
            spore_bank: SporeBankConfig {
                germinate_odds: 4,
                max_age_ticks: 320_000,
                ..SporeBankConfig::default()
            },
            page: SettingsPage::World,
            perf: PerfConfig::default(),
            failure: FailureConfig::default(),
            competent_fall: CompetentFallConfig::default(),
            max_roof_events: FailureConfig::default().max_roof_events as f32,
            max_shear_events: FailureConfig::default().max_shear_events as f32,
            max_compaction_events: FailureConfig::default().max_compaction_events as f32,
            shear_chance_pct: FailureConfig::default().shear_chance_per_mille as f32 / 10.0,
            competent_max_drop: CompetentFallConfig::default().max_passes as f32,
            competent_min_impact: CompetentFallConfig::default().min_impact_fall_cells as f32,
            competent_max_rolls: CompetentFallConfig::default().max_roll_events as f32,
            wind_vx: 0.05,
            wind_variance: 0.55,
            humidity_diffusion_alpha: 0.15,
            mat_perm_min,
            mat_perm_max,
            mat_poro_min,
            mat_poro_max,
            plant_genes: PlantGeneSettings::default(),
            apply_genes_to_living: false,
            max_atoms: MAX_ATOMS as f32,
            max_corpses: MAX_CORPSES as f32,
            max_roots: MAX_ROOT_MODULES as f32,
            max_stems: MAX_STEM_MODULES as f32,
            max_photos: MAX_PHOTO_MODULES as f32,
            world_width_chunks: (params.width_cols as f32 / CHUNK_CELLS_W as f32).max(1.0),
            world_sea_level: params.sea_level_y as f32,
            world_sky_ceiling: params.sky_ceiling_y as f32,
            request_regen: false,
            preset_name: "soak-survival".into(),
            preset_status: String::new(),
        }
    }

    /// Snapshot live Tab knobs (excludes worldgen size / regen).
    pub fn to_preset(&self) -> SimPreset {
        SimPreset {
            schema_version: wk_voxel::PRESET_SCHEMA_VERSION,
            notes: String::new(),
            rain: self.rain,
            evap: self.evap,
            cond: self.cond,
            oro: self.oro,
            karst: self.karst,
            cloud: self.cloud,
            climate: self.climate,
            temp: self.temp,
            phase: self.phase,
            grain: self.grain.clone(),
            fungi: self.fungi,
            carbon: self.carbon,
            spore_bank: self.spore_bank,
            perf: self.perf,
            failure: self.failure,
            wind_vx: self.wind_vx,
            wind_variance: self.wind_variance,
            humidity_diffusion_alpha: self.humidity_diffusion_alpha,
            plant_genes: self.plant_genes.to_preset(),
            max_atoms: self.max_atoms,
            max_corpses: self.max_corpses,
            max_roots: self.max_roots,
            max_stems: self.max_stems,
            max_photos: self.max_photos,
            mat_perm_min: self.mat_perm_min,
            mat_perm_max: self.mat_perm_max,
            mat_poro_min: self.mat_poro_min,
            mat_poro_max: self.mat_poro_max,
        }
    }

    /// Apply a preset without regenerating the world.
    ///
    /// World-bound rain/cond/oro extents (`top_y`, `x_range`, sea/seed/width)
    /// stay on the live world so a preset from another map still fits.
    pub fn apply_preset(&mut self, p: &SimPreset) {
        let rain_top = self.rain.top_y;
        let rain_range = self.rain.x_range;
        let rain_sea = self.rain.sea_level_y;
        let cond_top = self.cond.top_y;
        let oro_seed = self.oro.seed;
        let oro_w = self.oro.width_cols;
        let oro_sea = self.oro.sea_level_y;

        self.rain = p.rain;
        self.rain.top_y = rain_top;
        self.rain.x_range = rain_range;
        self.rain.sea_level_y = rain_sea;
        self.evap = p.evap;
        self.cond = p.cond;
        self.cond.top_y = cond_top;
        self.oro = p.oro;
        self.oro.seed = oro_seed;
        self.oro.width_cols = oro_w;
        self.oro.sea_level_y = oro_sea;
        self.karst = p.karst;
        self.cloud = p.cloud;
        self.climate = p.climate;
        self.temp = p.temp;
        self.phase = p.phase;
        self.grain = p.grain.clone();
        self.fungi = p.fungi;
        self.carbon = p.carbon;
        self.spore_bank = p.spore_bank;
        self.perf = p.perf;
        self.failure = p.failure;
        self.max_roof_events = self.failure.max_roof_events as f32;
        self.max_shear_events = self.failure.max_shear_events as f32;
        self.max_compaction_events = self.failure.max_compaction_events as f32;
        self.shear_chance_pct = self.failure.shear_chance_per_mille as f32 / 10.0;
        self.competent_max_drop = self.competent_fall.max_passes as f32;
        self.competent_min_impact = self.competent_fall.min_impact_fall_cells as f32;
        self.competent_max_rolls = self.competent_fall.max_roll_events as f32;
        self.wind_vx = p.wind_vx;
        self.wind_variance = p.wind_variance;
        self.humidity_diffusion_alpha = p.humidity_diffusion_alpha;
        self.plant_genes = PlantGeneSettings::from_preset(&p.plant_genes);
        self.max_atoms = p.max_atoms;
        self.max_corpses = p.max_corpses;
        self.max_roots = p.max_roots;
        self.max_stems = p.max_stems;
        self.max_photos = p.max_photos;
        self.mat_perm_min = p.mat_perm_min;
        self.mat_perm_max = p.mat_perm_max;
        self.mat_poro_min = p.mat_poro_min;
        self.mat_poro_max = p.mat_poro_max;
    }

    pub fn on_world_reseed(&mut self, params: &WorldgenParams) {
        self.rain.top_y = params.sky_ceiling_y - 1;
        self.rain.x_range = (0, params.width_cols - 1);
        self.rain.sea_level_y = params.sea_level_y;
        self.cond.top_y = params.sky_ceiling_y - 2;
        self.oro.seed = params.seed;
        self.oro.width_cols = params.width_cols;
        self.oro.sea_level_y = params.sea_level_y;
        self.world_width_chunks = (params.width_cols as f32 / CHUNK_CELLS_W as f32).max(1.0);
        self.world_sea_level = params.sea_level_y as f32;
        self.world_sky_ceiling = params.sky_ceiling_y as f32;
    }

    /// Build worldgen params from the World size draft sliders.
    pub fn draft_world_params(&self, base: &WorldgenParams) -> WorldgenParams {
        let chunks = self.world_width_chunks.round().clamp(2.0, 64.0) as i32;
        WorldgenParams {
            width_cols: chunks * CHUNK_CELLS_W as i32,
            sea_level_y: self.world_sea_level.round().clamp(8.0, 400.0) as i32,
            sky_ceiling_y: self
                .world_sky_ceiling
                .round()
                .clamp(32.0, 640.0)
                .max(self.world_sea_level.round() + 16.0) as i32,
            ..*base
        }
    }

    /// Push material slider values onto `world.hydro` (read by physics
    /// via [`World::water_capacity`] / `props_with` — no install step).
    pub fn apply_material_overrides(&self, world: &mut World) {
        world.hydro.clear();
        for id in MaterialId::ALL_SOLIDS {
            let i = id as usize;
            world.hydro.set_permeability_range(
                id,
                self.mat_perm_min[i].round() as u8,
                self.mat_perm_max[i].round() as u8,
            );
            world.hydro.set_porosity_range(
                id,
                self.mat_poro_min[i].round() as u8,
                self.mat_poro_max[i].round() as u8,
            );
        }
    }

    pub fn reset_materials_to_defaults(&mut self, world: &mut World) {
        world.hydro.clear();
        for id in MaterialId::ALL_SOLIDS {
            let i = id as usize;
            let base = MaterialRegistry::hydrology(id);
            self.mat_perm_min[i] = base.permeability.min as f32;
            self.mat_perm_max[i] = base.permeability.max as f32;
            self.mat_poro_min[i] = base.porosity.min as f32;
            self.mat_poro_max[i] = base.porosity.max as f32;
        }
    }

    pub fn draw(&mut self, world: &mut World, carbon: &CarbonBudget) {
        if !self.open {
            return;
        }
        let carbon_atm = carbon.atmosphere;
        let carbon_diss = carbon.dissolved;
        let carbon_total = carbon.total();

        // Keep integer-ish fields as f32 scratch for sliders.
        let mut evap_rate = self.evap.rate_per_tick as f32;
        let mut evap_period = self.evap.period_ticks as f32;
        let mut dry_above = self.evap.dry_above_max as f32;
        let mut droplet = self.rain.droplet_sat as f32;
        let mut day_ticks = self.climate.day_ticks as f32;
        let mut night_ticks = self.climate.night_ticks as f32;
        let mut max_parcels = self.cloud.max_parcels as f32;
        let mut cloud_alt = self.cloud.cloud_alt_above_sea as f32;
        let mut coag_min_alt = self.cloud.coag_min_above_sea as f32;
        let mut tall_above = self.oro.tall_above_sea as f32;
        let mut cond_events = self.cond.max_events_per_tick as f32;
        let mut karst_period = self.karst.period_ticks as f32;
        let mut reset_materials = false;
        let mut min_sat = self.karst.min_wet_neighbour_sat as f32;

        // Wide enough for value + track; labels sit on their own line
        // so they never clip against the window edge.
        let win_w = screen_width().min(720.0).max(520.0);
        let win_h = screen_height().min(780.0).max(560.0);
        widgets::Window::new(hash!(), vec2(12.0, 12.0), vec2(win_w, win_h))
            .label("Settings (Tab to close)")
            .ui(&mut *root_ui(), |ui| {
                ui.label(None, "Page:");
                if ui.button(
                    None,
                    if self.page == SettingsPage::World {
                        "[*] World"
                    } else {
                        "[ ] World"
                    },
                ) {
                    self.page = SettingsPage::World;
                }
                if ui.button(
                    None,
                    if self.page == SettingsPage::Climate {
                        "[*] Climate"
                    } else {
                        "[ ] Climate"
                    },
                ) {
                    self.page = SettingsPage::Climate;
                }
                if ui.button(
                    None,
                    if self.page == SettingsPage::Physics {
                        "[*] Physics"
                    } else {
                        "[ ] Physics"
                    },
                ) {
                    self.page = SettingsPage::Physics;
                }
                if ui.button(
                    None,
                    if self.page == SettingsPage::Life {
                        "[*] Life"
                    } else {
                        "[ ] Life"
                    },
                ) {
                    self.page = SettingsPage::Life;
                }
                ui.separator();
                ui.label(
                    None,
                    match self.page {
                        SettingsPage::World => "World — size, materials, karst",
                        SettingsPage::Climate => "Climate — day/night, ice, wind, N clouds, C drizzle",
                        SettingsPage::Physics => "Physics — performance, geotech, grain",
                        SettingsPage::Life => {
                            "Life — creatures, plants, fungi compost, carbon, spore bank"
                        }
                    },
                );
                ui.separator();

                ui.tree_node(hash!(), "Named presets", |ui| {
                    ui.label(
                        None,
                        &format!(
                            "Save / load Tab knobs as {PRESET_DIR}/<name>.json \
                             (world size needs Regenerate; not included)."
                        ),
                    );
                    ui.input_text(hash!(), "name", &mut self.preset_name);
                    if ui.button(None, "Save preset") {
                        match sanitize_preset_name(&self.preset_name) {
                            None => {
                                self.preset_status =
                                    "Name must be 1..=48 chars of [a-z0-9_-]".into();
                            }
                            Some(name) => {
                                self.preset_name = name.clone();
                                let mut preset = self.to_preset();
                                if preset.notes.is_empty() {
                                    preset.notes = format!("saved as {name}");
                                }
                                match save_preset(&name, &preset) {
                                    Ok(path) => {
                                        self.preset_status =
                                            format!("Saved {}", path.display());
                                    }
                                    Err(e) => {
                                        self.preset_status = format!("Save failed: {e}");
                                    }
                                }
                            }
                        }
                    }
                    ui.same_line(0.0);
                    if ui.button(None, "Load named") {
                        match load_preset(&self.preset_name) {
                            Ok(p) => {
                                self.apply_preset(&p);
                                self.apply_material_overrides(world);
                                let note = if p.notes.is_empty() {
                                    String::new()
                                } else {
                                    format!(" — {}", p.notes)
                                };
                                self.preset_status =
                                    format!("Loaded {}{note}", self.preset_name.trim());
                            }
                            Err(e) => {
                                self.preset_status = format!("Load failed: {e}");
                            }
                        }
                    }
                    ui.label(None, "Quick load:");
                    for name in list_all_presets() {
                        let label = format!("> {name}");
                        if ui.button(None, label.as_str()) {
                            match load_preset(&name) {
                                Ok(p) => {
                                    self.apply_preset(&p);
                                    self.apply_material_overrides(world);
                                    self.preset_name = name.clone();
                                    let note = if p.notes.is_empty() {
                                        String::new()
                                    } else {
                                        format!(" — {}", p.notes)
                                    };
                                    self.preset_status = format!("Loaded {name}{note}");
                                }
                                Err(e) => {
                                    self.preset_status = format!("Load {name} failed: {e}");
                                }
                            }
                        }
                    }
                    if !self.preset_status.is_empty() {
                        ui.label(None, &self.preset_status.clone());
                    }
                });
                ui.separator();

                if self.page == SettingsPage::World {
                ui.tree_node(hash!(), "World size", |ui| {
                    ui.label(
                        None,
                        "Draft size — click Regenerate to rebuild (keeps seed).",
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Width (chunks of 64 cells)",
                        2.0..48.0,
                        &mut self.world_width_chunks,
                    );
                    ui.label(
                        None,
                        &format!(
                            "  → {} cells wide",
                            self.world_width_chunks.round().clamp(2.0, 64.0) as i32
                                * CHUNK_CELLS_W as i32
                        ),
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Sea level (y)",
                        8.0..240.0,
                        &mut self.world_sea_level,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Sky ceiling (y)",
                        64.0..480.0,
                        &mut self.world_sky_ceiling,
                    );
                    if ui.button(None, "Regenerate world with size") {
                        self.request_regen = true;
                    }
                });
                } // World page (size); materials/karst further below
                if self.page == SettingsPage::Climate {
                ui.separator();

                ui.tree_node(hash!(), "Heatmap overlays", |ui| {
                    ui.label(None, "U = ground saturation. T/M/G also use this blend.");
                    ui.label(None, "0 = landscape only · 1 = heatmap only");
                    labeled_slider(
                        ui,
                        hash!(),
                        "Landscape ↔ heatmap",
                        0.0..1.0,
                        &mut self.heatmap_blend,
                    );
                });

                ui.tree_node(hash!(), "Sky look / atmosphere", |ui| {
                    ui.label(None, "Cosmetics only — tweak live, no regen needed.");
                    ui.label(None, "Sun/moon radii are screen pixels (finer than voxels).");
                    labeled_slider(ui, hash!(), "Sun radius (px)", 12.0..80.0, &mut self.atmosphere.sun_radius);
                    labeled_slider(
                        ui,
                        hash!(),
                        "Sun glow radius (px)",
                        24.0..140.0,
                        &mut self.atmosphere.sun_glow_radius,
                    );
                    labeled_slider(ui, hash!(), "Moon radius (px)", 10.0..72.0, &mut self.atmosphere.moon_radius);
                    labeled_slider(
                        ui,
                        hash!(),
                        "Moon bite offset (px)",
                        4.0..40.0,
                        &mut self.atmosphere.moon_bite_offset,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Moon bite radius (px)",
                        8.0..64.0,
                        &mut self.atmosphere.moon_bite_radius,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Cloud whiteness",
                        0.0..1.0,
                        &mut self.atmosphere.cloud_whiteness,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Cloud far (depth)",
                        0.0..1.0,
                        &mut self.atmosphere.vapour_far,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Cloud mid (depth)",
                        0.0..1.0,
                        &mut self.atmosphere.vapour_mid,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Cloud active",
                        0.0..1.0,
                        &mut self.atmosphere.vapour_active,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Cloud front (depth)",
                        0.0..1.0,
                        &mut self.atmosphere.vapour_front,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Ridge sky mix (near)",
                        0.0..1.0,
                        &mut self.atmosphere.ridge_sky_mix_near,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Ridge sky mix (far)",
                        0.0..1.0,
                        &mut self.atmosphere.ridge_sky_mix_far,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Ridge desat (near)",
                        0.0..1.0,
                        &mut self.atmosphere.ridge_desat_near,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Ridge desat (far)",
                        0.0..1.0,
                        &mut self.atmosphere.ridge_desat_far,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Ridge feather (near)",
                        2.0..10.0,
                        &mut self.atmosphere.ridge_feather_near,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Ridge feather (far)",
                        2.0..10.0,
                        &mut self.atmosphere.ridge_feather_far,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Ridge crest blend",
                        0.0..1.0,
                        &mut self.atmosphere.ridge_crest_blend,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Far into mid crest",
                        0.0..1.0,
                        &mut self.atmosphere.ridge_far_into_crest,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Cast shadow strength",
                        0.0..1.0,
                        &mut self.atmosphere.cast_shadow_strength,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Cloud shade (visual)",
                        0.0..1.0,
                        &mut self.atmosphere.cloud_shade_strength,
                    );
                });
                ui.separator();

                ui.tree_node(hash!(), "Day / night / temperature", |ui| {
                    labeled_slider(ui, hash!(), "Day length (ticks)", 60.0..6_000.0, &mut day_ticks);
                    labeled_slider(ui, hash!(), "Night length (ticks)", 60.0..6_000.0, &mut night_ticks);
                    ui.label(
                        None,
                        &format!(
                            "  cycle ≈ {:.1}s at 60 tick/s",
                            (day_ticks + night_ticks) / 60.0
                        ),
                    );
                    labeled_slider(ui, hash!(), "Base temp (C)", -20.0..40.0, &mut self.temp.base_temp_c);
                    labeled_slider(ui, hash!(), "Day/night swing (C)", 0.0..20.0, &mut self.temp.day_amp_c);
                    labeled_slider(ui, hash!(), "Lapse (C per cell elev)", 0.0..0.4, &mut self.temp.lapse_c);
                    labeled_slider(ui, hash!(), "Solar heat / step", 0.0..1.5, &mut self.temp.solar_heat_c);
                    labeled_slider(ui, hash!(), "Night cool / step", 0.0..1.5, &mut self.temp.night_cool_c);
                    labeled_slider(ui, hash!(), "Cloud shade (thermal)", 0.0..1.0, &mut self.temp.cloud_shade);
                    labeled_slider(ui, hash!(), "Sea bias (C)", -10.0..5.0, &mut self.temp.sea_bias_c);
                    labeled_slider(
                        ui,
                        hash!(),
                        "Sky relax / step",
                        0.0..0.5,
                        &mut self.temp.sky_relax,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Inertia scale",
                        0.0..4.0,
                        &mut self.temp.inertia_scale,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Water stack capacity bonus",
                        0.0..4.0,
                        &mut self.temp.water_stack_cap,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Geothermal surface (C)",
                        -5.0..25.0,
                        &mut self.temp.geothermal_surface_c,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Geothermal gradient (C/cell)",
                        0.0..1.0,
                        &mut self.temp.geothermal_gradient_c_per_cell,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Geothermal flux / step",
                        0.0..0.3,
                        &mut self.temp.geothermal_flux_c,
                    );
                });
                ui.separator();

                ui.tree_node(hash!(), "Ice / snow / slush", |ui| {
                    ui.checkbox(hash!(), "Phase enabled (I)", &mut self.phase.enabled);
                    ui.checkbox(hash!(), "Freeze standing water", &mut self.phase.enable_freeze);
                    ui.checkbox(hash!(), "Thaw ice / snow", &mut self.phase.enable_thaw);
                    ui.checkbox(hash!(), "Slush (water↔snow/ice)", &mut self.phase.enable_slush);
                    ui.checkbox(
                        hash!(),
                        "Break ice on haze (empty gaps fall)",
                        &mut self.phase.enable_break_unsupported,
                    );
                    ui.checkbox(
                        hash!(),
                        "Cold avalanche (wet sand/snow→ice)",
                        &mut self.phase.enable_cold_avalanche,
                    );
                    ui.checkbox(
                        hash!(),
                        "Break thin ice under debris",
                        &mut self.phase.enable_ice_load_break,
                    );
                    ui.checkbox(hash!(), "Cull tall ice/snow stacks", &mut self.phase.enable_cull);
                    ui.checkbox(
                        hash!(),
                        "W snow packs (cold air; melts on warm ground)",
                        &mut self.phase.enable_snow_precip,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Freeze point (C)",
                        -10.0..5.0,
                        &mut self.phase.freeze_point_c,
                    );
                    let mut min_freeze = self.phase.min_sat_to_freeze as f32;
                    let mut max_ice = self.phase.max_ice_cells_per_column as f32;
                    let mut max_freeze = self.phase.max_freeze_cells_per_column_per_tick as f32;
                    let mut max_thaw = self.phase.max_thaw_cells_per_column_per_tick as f32;
                    let mut max_slush = self.phase.max_slush_cells_per_column_per_tick as f32;
                    let mut max_break = self.phase.max_break_cells_per_column_per_tick as f32;
                    let mut max_load_break =
                        self.phase.max_load_break_cells_per_column_per_tick as f32;
                    let mut carry = self.phase.ice_carry_thickness as f32;
                    let mut spread = self.phase.snow_spread_radius as f32;
                    let mut blanket = self.phase.snow_blanket_depth as f32;
                    let mut frost_depth = self.phase.frost_coat_depth as f32;
                    let mut frost_spread = self.phase.frost_spread_radius as f32;
                    let mut period = self.phase.period_ticks as f32;
                    labeled_slider(
                        ui,
                        hash!(),
                        "Min sat to freeze (255=full only)",
                        1.0..255.0,
                        &mut min_freeze,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Max ice+snow cells / column",
                        1.0..48.0,
                        &mut max_ice,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Freeze cells / col / tick",
                        1.0..8.0,
                        &mut max_freeze,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Thaw cells / col / tick",
                        1.0..8.0,
                        &mut max_thaw,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Slush cells / col / tick",
                        1.0..8.0,
                        &mut max_slush,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Break cells / col / tick",
                        1.0..8.0,
                        &mut max_break,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Load-break cells / col / tick",
                        1.0..8.0,
                        &mut max_load_break,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Ice thickness to carry debris",
                        1.0..8.0,
                        &mut carry,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Min precip budget to snow (full cell)",
                        1.0..255.0,
                        &mut self.phase.min_budget_to_snow,
                    );
                    labeled_slider(ui, hash!(), "Snow spread radius (cols)", 0.0..24.0, &mut spread);
                    labeled_slider(ui, hash!(), "Snow blanket prefer depth", 0.0..12.0, &mut blanket);
                    labeled_slider(
                        ui,
                        hash!(),
                        "Frost coat depth (cells)",
                        1.0..4.0,
                        &mut frost_depth,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Frost spread radius (cols)",
                        0.0..12.0,
                        &mut frost_spread,
                    );
                    labeled_slider(ui, hash!(), "Phase period (ticks)", 1.0..60.0, &mut period);
                    self.phase.min_sat_to_freeze = min_freeze.round().clamp(1.0, 255.0) as u8;
                    self.phase.max_ice_cells_per_column = max_ice.round().clamp(1.0, 64.0) as u8;
                    self.phase.max_freeze_cells_per_column_per_tick =
                        max_freeze.round().clamp(1.0, 16.0) as u8;
                    self.phase.max_thaw_cells_per_column_per_tick =
                        max_thaw.round().clamp(1.0, 16.0) as u8;
                    self.phase.max_slush_cells_per_column_per_tick =
                        max_slush.round().clamp(1.0, 16.0) as u8;
                    self.phase.max_break_cells_per_column_per_tick =
                        max_break.round().clamp(1.0, 16.0) as u8;
                    self.phase.max_load_break_cells_per_column_per_tick =
                        max_load_break.round().clamp(1.0, 16.0) as u8;
                    self.phase.ice_carry_thickness = carry.round().clamp(1.0, 16.0) as u8;
                    self.phase.snow_spread_radius = spread.round().clamp(0.0, 48.0) as i32;
                    self.phase.snow_blanket_depth = blanket.round().clamp(0.0, 32.0) as u8;
                    self.phase.frost_coat_depth = frost_depth.round().clamp(1.0, 8.0) as u8;
                    self.phase.frost_spread_radius = frost_spread.round().clamp(0.0, 24.0) as i32;
                    self.phase.period_ticks = period.round().clamp(1.0, 120.0) as u64;
                    self.phase.min_budget_to_snow =
                        self.phase.min_budget_to_snow.clamp(1.0, 255.0);
                });
                } // Climate (day + ice)
                if self.page == SettingsPage::Physics {
                ui.separator();

                ui.tree_node(hash!(), "Performance", |ui| {
                    ui.label(
                        None,
                        "Defaults favour fast surface flow (every substep, quiet EO → max 8; seepage every 4 ticks).",
                    );
                    ui.label(
                        None,
                        "Uncheck quiet EO for full ×12 feel. Enable every-other for thrift A/B.",
                    );
                    ui.checkbox(
                        hash!(),
                        "Flow every other substep (gravity still every step)",
                        &mut self.perf.flow_every_other_substep,
                    );
                    ui.label(
                        None,
                        "  Off by default — hillside runoff needs every pass. On ≈ half surface-flow work.",
                    );
                    ui.checkbox(
                        hash!(),
                        "Quiet flow early-out (tiny / shrinking dirty halo)",
                        &mut self.perf.flow_quiet_early_out,
                    );
                    ui.label(
                        None,
                        "  On skips polish when the halo shrinks. Also cadence-gates pore seepage (beds still wake every tick).",
                    );
                    ui.checkbox(
                        hash!(),
                        "Parallel physics (rayon checkerboard)",
                        &mut self.perf.parallel_physics,
                    );
                    ui.label(
                        None,
                        "  Off by default — demo ~6 dirty regions; rayon was slower on 32 cores.",
                    );
                });
                ui.separator();

                ui.tree_node(hash!(), "Geotech / failure", |ui| {
                    ui.label(
                        None,
                        "Roof collapse: ceilings over Air wider than roof_span_max_m drop.",
                    );
                    ui.label(
                        None,
                        "Sand/clay never roof; stone holds short spans; bedrock never falls.",
                    );
                    ui.checkbox(
                        hash!(),
                        "Roof / overhang collapse",
                        &mut self.failure.enable_roof_collapse,
                    );
                    ui.checkbox(
                        hash!(),
                        "Competent rock rigid fall",
                        &mut self.failure.enable_competent_fall,
                    );
                    ui.label(
                        None,
                        "Stone / limestone fall as connected bodies; impact → debris; roll on slopes.",
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Max fall cells / tick",
                        8.0..128.0,
                        &mut self.competent_max_drop,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Min impact fall cells",
                        0.0..32.0,
                        &mut self.competent_min_impact,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Max slope rolls / tick",
                        0.0..16.0,
                        &mut self.competent_max_rolls,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Max roof events / tick",
                        1.0..128.0,
                        &mut self.max_roof_events,
                    );
                    ui.label(
                        None,
                        "Shear: wet low-c′ grains loosen in repose; rock → LooseRock, limestone → LooseLimestone.",
                    );
                    ui.checkbox(
                        hash!(),
                        "Shear weaken (rock faces)",
                        &mut self.failure.enable_shear_weaken,
                    );
                    ui.checkbox(
                        hash!(),
                        "Use geotech map for shear (S3)",
                        &mut self.failure.use_geotech_map,
                    );
                    ui.label(
                        None,
                        "Map gate: tall wet columns can break thin dams (G cycles overlays).",
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Max shear events / tick",
                        1.0..64.0,
                        &mut self.max_shear_events,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Shear chance %",
                        1.0..100.0,
                        &mut self.shear_chance_pct,
                    );
                    ui.checkbox(
                        hash!(),
                        "Compaction (deep Clay/Organic)",
                        &mut self.failure.enable_compaction,
                    );
                    ui.label(
                        None,
                        "Under high σᵥ, wet soft sediment squeezes pore water upward.",
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Max compaction events / tick",
                        1.0..64.0,
                        &mut self.max_compaction_events,
                    );
                });
                ui.separator();

                ui.tree_node(hash!(), "Grain / sediment", |ui| {
                    ui.label(
                        None,
                        "Repose (sand piles) always runs in tick. Flow erosion is opt-in.",
                    );
                    ui.checkbox(hash!(), "Flow erosion + deposit", &mut self.grain.enabled);
                    labeled_slider(
                        ui,
                        hash!(),
                        "Erosion rate",
                        0.0..1.0,
                        &mut self.grain.erosion_rate,
                    );
                    let mut min_sat = self.grain.min_flow_sat as f32;
                    let mut max_ev = self.grain.max_events_per_tick as f32;
                    labeled_slider(ui, hash!(), "Min flow sat", 1.0..255.0, &mut min_sat);
                    labeled_slider(ui, hash!(), "Max events / tick", 0.0..256.0, &mut max_ev);
                    self.grain.min_flow_sat = min_sat.round().clamp(1.0, 255.0) as u8;
                    self.grain.max_events_per_tick = max_ev.round().clamp(0.0, 512.0) as u32;
                    ui.separator();
                    ui.label(
                        None,
                        "Floating Organic: higher waterlog = mats sink sooner; \
                         bind radius dilates root rafts (0 = body span only). \
                         Mycelium cream (0–255) also sticks grounded litter and \
                         toughens colonized rafts automatically.",
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Organic waterlog rate",
                        0.0..0.02,
                        &mut self.grain.organic_waterlog_rate,
                    );
                    let mut bind = self.grain.raft_root_bind_radius as f32;
                    labeled_slider(ui, hash!(), "Raft root bind radius", 0.0..4.0, &mut bind);
                    self.grain.raft_root_bind_radius = bind.round().clamp(0.0, 8.0) as i32;
                });
                } // Physics
                if self.page == SettingsPage::Climate {
                ui.separator();

                ui.tree_node(hash!(), "Wind + humidity", |ui| {
                    labeled_slider(ui, hash!(), "Wind mean (tiles/tick)", -0.5..0.5, &mut self.wind_vx);
                    ui.label(
                        None,
                        "Natural variance: 0 = steady push; higher = force & direction shift over time.",
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Natural variance",
                        0.0..1.0,
                        &mut self.wind_variance,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Humidity diffuse alpha",
                        0.0..0.25,
                        &mut self.humidity_diffusion_alpha,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Buoyant rise / tick",
                        0.0..0.4,
                        &mut self.cloud.buoyant_rise,
                    );
                });
                ui.separator();

                ui.tree_node(hash!(), "Clouds (N visual echo)", |ui| {
                    ui.label(
                        None,
                        "N banks copy wet humidity tiles. They do not store or rain water.",
                    );
                    labeled_slider(ui, hash!(), "Max parcels", 1.0..64.0, &mut max_parcels);
                    labeled_slider(ui, hash!(), "Visual min humidity", 1.0..120.0, &mut self.cloud.coag_min_hum);
                    labeled_slider(ui, hash!(), "N streak wetness scale", 40.0..500.0, &mut self.cloud.downpour_mass);
                    labeled_slider(ui, hash!(), "Vapor rise deck above sea", 8.0..160.0, &mut cloud_alt);
                    labeled_slider(ui, hash!(), "Visual min above sea", 4.0..120.0, &mut coag_min_alt);
                    labeled_slider(
                        ui,
                        hash!(),
                        "Ridge clearance",
                        0.0..36.0,
                        &mut self.cloud.ridge_clearance,
                    );
                });
                ui.separator();

                ui.tree_node(hash!(), "Weather (C drizzle / E evap / W faucet)", |ui| {
                    ui.label(
                        None,
                        "C is the rain (humidity → ground). W is an extra faucet, off by default.",
                    );
                    ui.checkbox(hash!(), "C drizzle / dew (hotkey C)", &mut self.cond_rain_on);
                    ui.checkbox(hash!(), "E evap into humidity (hotkey E)", &mut self.evap_on);
                    labeled_slider(
                        ui,
                        hash!(),
                        "Drizzle min mass",
                        10.0..400.0,
                        &mut self.cond.min_mass_to_rain,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Drizzle max prob",
                        0.0..1.0,
                        &mut self.cond.max_prob_per_tick,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Drizzle mass / drop",
                        4.0..200.0,
                        &mut self.cond.mass_per_droplet,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Drizzle full-mass (rate cap)",
                        32.0..2_500.0,
                        &mut self.cond.full_mass,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Drizzle events / tick (0=unlimited)",
                        0.0..256.0,
                        &mut cond_events,
                    );
                    labeled_slider(ui, hash!(), "Evap rate / pulse", 0.0..8.0, &mut evap_rate);
                    labeled_slider(ui, hash!(), "Evap period (ticks)", 1.0..30.0, &mut evap_period);
                    labeled_slider(ui, hash!(), "Evap dry-above max", 0.0..255.0, &mut dry_above);
                    labeled_slider(ui, hash!(), "Oro tall above sea", 4.0..60.0, &mut tall_above);
                    labeled_slider(
                        ui,
                        hash!(),
                        "Oro ascent scale",
                        4.0..80.0,
                        &mut self.oro.ascent_scale,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Oro max prob mult",
                        1.0..6.0,
                        &mut self.oro.max_prob_mult,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Oro mass mult",
                        1.0..4.0,
                        &mut self.oro.mass_mult,
                    );
                    ui.separator();
                    ui.label(None, "W climatic faucet — optional; keep closed-loop to avoid minting.");
                    ui.checkbox(hash!(), "W climatic rain (hotkey W)", &mut self.climatic_rain_on);
                    ui.checkbox(
                        hash!(),
                        "W closed-loop (drain humidity; no mint)",
                        &mut self.rain.closed_loop,
                    );
                    let mut flood = self.rain.max_flood_above_sea as f32;
                    labeled_slider(
                        ui,
                        hash!(),
                        "W flood guard (cells above sea; 0=off)",
                        0.0..48.0,
                        &mut flood,
                    );
                    self.rain.max_flood_above_sea = flood.round().clamp(0.0, 64.0) as i32;
                    labeled_slider(
                        ui,
                        hash!(),
                        "W rain prob / column",
                        0.0..0.2,
                        &mut self.rain.prob_per_col_per_tick,
                    );
                    labeled_slider(ui, hash!(), "W droplet sat", 1.0..255.0, &mut droplet);
                });
                } // Climate (wind/clouds/rain)
                if self.page == SettingsPage::World {
                ui.separator();

                ui.tree_node(hash!(), "Ground look (wetness / porosity)", |ui| {
                    ui.label(
                        None,
                        "Wet rock darkens (vs its own capacity, not /255).",
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Waterlogged darkening",
                        0.0..0.9,
                        &mut self.wet_darken,
                    );
                    ui.label(None, "Porous rock is stippled — only open cells.");
                    labeled_slider(
                        ui,
                        hash!(),
                        "Pore stipple strength (0 = off)",
                        0.0..0.9,
                        &mut self.pore_stipple,
                    );
                });
                ui.separator();

                ui.tree_node(hash!(), "Material permeability / porosity", |ui| {
                    ui.label(None, "Each cell samples inside these ranges; 0–0 is sealed.");
                    for id in MaterialId::ALL_SOLIDS {
                        let i = id as usize;
                        let name = material_short_name(id);
                        labeled_slider(
                            ui,
                            hash!(name, "perm_min"),
                            &format!("{name} permeability min"),
                            0.0..255.0,
                            &mut self.mat_perm_min[i],
                        );
                        labeled_slider(
                            ui,
                            hash!(name, "perm_max"),
                            &format!("{name} permeability max"),
                            0.0..255.0,
                            &mut self.mat_perm_max[i],
                        );
                        labeled_slider(
                            ui,
                            hash!(name, "poro_min"),
                            &format!("{name} porosity min"),
                            0.0..255.0,
                            &mut self.mat_poro_min[i],
                        );
                        labeled_slider(
                            ui,
                            hash!(name, "poro_max"),
                            &format!("{name} porosity max"),
                            0.0..255.0,
                            &mut self.mat_poro_max[i],
                        );
                    }
                    if ui.button(None, "Reset materials to defaults") {
                        reset_materials = true;
                    }
                });
                ui.separator();

                ui.tree_node(hash!(), "Karst", |ui| {
                    ui.checkbox(
                        hash!(),
                        "K limestone / groundwater dissolve (hotkey K)",
                        &mut self.karst_on,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Dissolve prob / wet neighbour",
                        0.0..0.05,
                        &mut self.karst.prob_per_wet_neighbour,
                    );
                    labeled_slider(ui, hash!(), "Min wet neighbour sat", 1.0..255.0, &mut min_sat);
                    labeled_slider(ui, hash!(), "Karst period (ticks)", 1.0..128.0, &mut karst_period);
                    labeled_slider(
                        ui,
                        hash!(),
                        "Pore / cave scale (vs surface)",
                        0.0..1.0,
                        &mut self.karst.pore_scale,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Stone scale (vs limestone, underground)",
                        0.0..1.0,
                        &mut self.karst.stone_scale,
                    );
                });
                } // World (materials/karst)
                if self.page == SettingsPage::Life {
                ui.separator();

                ui.tree_node(hash!(), "Creatures / population caps", |ui| {
                    ui.label(
                        None,
                        "Entity caps — one plant/fungus/Atom = 1, not body pixels.",
                    );
                    ui.label(
                        None,
                        "Tissue growth ceilings are under Plant growth caps.",
                    );
                    ui.label(
                        None,
                        "Lowering a cap does not cull existing — it only blocks new spawns.",
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Max living creatures (entities)",
                        8.0..2048.0,
                        &mut self.max_atoms,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Max corpses (entities)",
                        8.0..2048.0,
                        &mut self.max_corpses,
                    );
                    if ui.button(None, "Reset pop caps to defaults (256)") {
                        self.max_atoms = MAX_ATOMS as f32;
                        self.max_corpses = MAX_CORPSES as f32;
                    }
                });
                ui.separator();

                ui.tree_node(hash!(), "Plant growth ceilings (safety)", |ui| {
                    ui.label(
                        None,
                        "Size is meant to be capped by cost, not by these.",
                    );
                    ui.label(
                        None,
                        "Upkeep rises per pixel while a self-shading canopy earns less,",
                    );
                    ui.label(
                        None,
                        "so plants settle at an energy-limited size on their own.",
                    );
                    ui.label(
                        None,
                        "These only stop a runaway. Lower them for the old tight look.",
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        &format!("Max roots / plant (default {MAX_ROOT_MODULES})"),
                        1.0..128.0,
                        &mut self.max_roots,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        &format!("Max stems / plant (default {MAX_STEM_MODULES})"),
                        0.0..128.0,
                        &mut self.max_stems,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        &format!("Max leaves / plant (default {MAX_PHOTO_MODULES})"),
                        1.0..128.0,
                        &mut self.max_photos,
                    );
                    if ui.button(None, "Reset growth caps to defaults") {
                        self.max_roots = MAX_ROOT_MODULES as f32;
                        self.max_stems = MAX_STEM_MODULES as f32;
                        self.max_photos = MAX_PHOTO_MODULES as f32;
                    }
                });
                ui.separator();

                ui.tree_node(hash!(), "Plants / fungi genes", |ui| {
                    ui.label(None, "Defaults for F2 spawn · optional apply to living.");
                    labeled_slider(
                        ui,
                        hash!(),
                        "Alloc stem",
                        0.0..1.0,
                        &mut self.plant_genes.alloc_stem,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Alloc leaf",
                        0.0..1.0,
                        &mut self.plant_genes.alloc_leaf,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Alloc root",
                        0.0..1.0,
                        &mut self.plant_genes.alloc_root,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Root depth bias (0=shallow 1=dive)",
                        0.0..1.0,
                        &mut self.plant_genes.root_depth_bias,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Leaf absorb (shade cast)",
                        0.05..1.0,
                        &mut self.plant_genes.leaf_absorb,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Shade efficiency (dim light)",
                        0.0..1.0,
                        &mut self.plant_genes.shade_efficiency,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Digest rate (fungi)",
                        0.05..2.0,
                        &mut self.plant_genes.digest_rate,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Clone fidelity",
                        0.05..1.0,
                        &mut self.plant_genes.clone_fidelity,
                    );
                    if ui.button(None, "Apply genes to living plants/fungi") {
                        self.apply_genes_to_living = true;
                    }
                    if ui.button(None, "Reset plant genes to defaults") {
                        self.plant_genes = PlantGeneSettings::default();
                    }
                });
                ui.separator();

                ui.tree_node(hash!(), "Fungi / compost", |ui| {
                    ui.label(
                        None,
                        "Mycelium humifies Organic → Soil. Lower odds = faster compost.                          Fruiting seats prefer Air on Organic/Soil (rhizomorph can still bury).",
                    );
                    let mut thresh = self.fungi.soil_mycelium_threshold as f32;
                    let mut odds = self.fungi.soil_convert_odds as f32;
                    labeled_slider(
                        ui,
                        hash!(),
                        "Soil mycelium threshold",
                        40.0..255.0,
                        &mut thresh,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Soil convert odds (1-in-N)",
                        50.0..8_000.0,
                        &mut odds,
                    );
                    self.fungi.soil_mycelium_threshold =
                        thresh.round().clamp(1.0, 255.0) as u8;
                    self.fungi.soil_convert_odds =
                        odds.round().clamp(1.0, 100_000.0) as u64;
                    if ui.button(None, "Reset fungi compost to defaults") {
                        self.fungi = FungiConfig::default();
                    }
                });
                ui.separator();

                ui.tree_node(hash!(), "Carbon (CO2 buckets)", |ui| {
                    ui.label(
                        None,
                        "Crude atmosphere + dissolved pools. Surface Organic oxidizes to Soil                          and credits atm C (not humidity). Lakes exchange atm ↔ dissolved.                          Algae draw dissolved (bloom throttles when empty); land plants lightly                          pull atm. Buckets persist in saves. O2 bucket later.",
                    );
                    ui.checkbox(hash!(), "Carbon enabled", &mut self.carbon.enabled);
                    let mut ox_period = self.carbon.oxidize_period as f32;
                    let mut ox_max = self.carbon.oxidize_max_events as f32;
                    let mut ex_period = self.carbon.exchange_period as f32;
                    labeled_slider(
                        ui,
                        hash!(),
                        "Oxidize period (ticks)",
                        8.0..512.0,
                        &mut ox_period,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Oxidize rate / cell",
                        0.0..0.05,
                        &mut self.carbon.oxidize_rate,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Oxidize max events",
                        0.0..128.0,
                        &mut ox_max,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "C per oxidized cell",
                        0.0..4.0,
                        &mut self.carbon.oxidize_c_per_cell,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Exchange period (ticks)",
                        8.0..256.0,
                        &mut ex_period,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Exchange rate",
                        0.0..0.5,
                        &mut self.carbon.exchange_rate,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Henry ratio (dissolved/atm)",
                        0.05..1.0,
                        &mut self.carbon.henry_ratio,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Algae C / energy",
                        0.0..0.5,
                        &mut self.carbon.algae_c_per_energy,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Algae half-sat (dissolved)",
                        1.0..200.0,
                        &mut self.carbon.algae_half_sat,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Plant C / energy",
                        0.0..0.2,
                        &mut self.carbon.plant_c_per_energy,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Plant half-sat (atm)",
                        10.0..500.0,
                        &mut self.carbon.plant_half_sat,
                    );
                    self.carbon.oxidize_period =
                        ox_period.round().clamp(1.0, 10_000.0) as u64;
                    self.carbon.oxidize_max_events =
                        ox_max.round().clamp(0.0, 512.0) as u32;
                    self.carbon.exchange_period =
                        ex_period.round().clamp(1.0, 10_000.0) as u64;
                    if ui.button(None, "Reset carbon knobs to defaults") {
                        self.carbon = CarbonConfig::default();
                    }
                    ui.separator();
                    ui.label(
                        None,
                        &format!(
                            "Live buckets: atm={carbon_atm:.0}  dissolved={carbon_diss:.0}  total={carbon_total:.0}"
                        ),
                    );
                });
                
                ui.separator();

                ui.tree_node(hash!(), "Spore bank", |ui| {
                    ui.label(
                        None,
                        "Spores that land dry / crowded / cold hibernate on that cell                          and may germinate much later when conditions improve.",
                    );
                    ui.checkbox(hash!(), "Spore bank enabled", &mut self.spore_bank.enabled);
                    let mut period = self.spore_bank.step_period as f32;
                    let mut per_cell = self.spore_bank.max_per_cell as f32;
                    let mut max_total = self.spore_bank.max_total as f32;
                    let mut max_age = self.spore_bank.max_age_ticks as f32;
                    let mut odds = self.spore_bank.germinate_odds as f32;
                    labeled_slider(ui, hash!(), "Wake period (ticks)", 8.0..512.0, &mut period);
                    labeled_slider(ui, hash!(), "Max per cell", 1.0..16.0, &mut per_cell);
                    labeled_slider(ui, hash!(), "Max total banked", 16.0..2048.0, &mut max_total);
                    labeled_slider(ui, hash!(), "Max age (ticks)", 1_000.0..500_000.0, &mut max_age);
                    labeled_slider(ui, hash!(), "Germinate odds (1-in-N)", 1.0..64.0, &mut odds);
                    labeled_slider(
                        ui,
                        hash!(),
                        "Min temp °C (cold gate)",
                        -10.0..20.0,
                        &mut self.spore_bank.min_temp_c,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Plant min bed moisture",
                        0.0..0.2,
                        &mut self.spore_bank.plant_min_moist,
                    );
                    self.spore_bank.step_period = period.round().clamp(1.0, 10_000.0) as u64;
                    self.spore_bank.max_per_cell = per_cell.round().clamp(1.0, 32.0) as u8;
                    self.spore_bank.max_total = max_total.round().clamp(1.0, 8_000.0) as u16;
                    self.spore_bank.max_age_ticks = max_age.round().clamp(100.0, 2_000_000.0) as u64;
                    self.spore_bank.germinate_odds = odds.round().clamp(1.0, 10_000.0) as u64;
                    if ui.button(None, "Reset spore bank knobs to defaults") {
                        self.spore_bank = SporeBankConfig::default();
                    }
                    ui.label(
                        None,
                        &format!("Live banked spores: {}", world.spore_bank.len()),
                    );
                });
} // Life
                ui.separator();
                ui.label(
                    None,
                    "Tip: Tab closes · F6 glossary · F2 creatures · F3 terrain · F5/F9 save/load",
                );
            });

        self.karst.min_wet_neighbour_sat = min_sat.round().clamp(1.0, 255.0) as u8;

        if reset_materials {
            self.reset_materials_to_defaults(world);
        } else {
            self.apply_material_overrides(world);
        }

        self.evap.rate_per_tick = evap_rate.round().clamp(0.0, 32.0) as u8;
        self.evap.period_ticks = evap_period.round().clamp(1.0, 120.0) as u64;
        self.evap.dry_above_max = dry_above.round().clamp(0.0, 255.0) as u8;
        self.rain.droplet_sat = droplet.round().clamp(1.0, 255.0) as u8;
        self.climate.day_ticks = day_ticks.round().clamp(30.0, 20_000.0) as u64;
        self.climate.night_ticks = night_ticks.round().clamp(30.0, 20_000.0) as u64;
        self.cloud.max_parcels = max_parcels.round().clamp(1.0, 96.0) as usize;
        self.cloud.cloud_alt_above_sea = cloud_alt.round().clamp(4.0, 200.0) as i32;
        self.cloud.coag_min_above_sea = coag_min_alt.round().clamp(2.0, 160.0) as i32;
        self.cloud.ridge_clearance = self.cloud.ridge_clearance.clamp(0.0, 48.0);
        self.oro.tall_above_sea = tall_above.round().clamp(2.0, 100.0) as i32;
        self.cond.max_events_per_tick = cond_events.round().clamp(0.0, 512.0) as u32;
        self.cond.full_mass = self.cond.full_mass.clamp(8.0, 4_000.0);
        self.karst.period_ticks = karst_period.round().clamp(1.0, 256.0) as u64;
        self.karst.pore_scale = self.karst.pore_scale.clamp(0.0, 1.0);
        self.karst.stone_scale = self.karst.stone_scale.clamp(0.0, 1.0);
        self.max_atoms = self.max_atoms.round().clamp(1.0, 4096.0);
        self.max_corpses = self.max_corpses.round().clamp(1.0, 4096.0);
        self.max_roots = self.max_roots.round().clamp(1.0, 256.0);
        self.max_stems = self.max_stems.round().clamp(0.0, 256.0);
        self.max_photos = self.max_photos.round().clamp(1.0, 256.0);
        self.max_roof_events = self.max_roof_events.round().clamp(1.0, 256.0);
        self.failure.max_roof_events = self.max_roof_events as u32;
        self.max_shear_events = self.max_shear_events.round().clamp(1.0, 128.0);
        self.failure.max_shear_events = self.max_shear_events as u32;
        self.max_compaction_events = self.max_compaction_events.round().clamp(1.0, 128.0);
        self.failure.max_compaction_events = self.max_compaction_events as u32;
        self.shear_chance_pct = self.shear_chance_pct.round().clamp(1.0, 100.0);
        self.failure.shear_chance_per_mille = (self.shear_chance_pct * 10.0) as u32;
        self.competent_max_drop = self.competent_max_drop.round().clamp(8.0, 128.0);
        self.competent_min_impact = self.competent_min_impact.round().clamp(0.0, 32.0);
        self.competent_max_rolls = self.competent_max_rolls.round().clamp(0.0, 16.0);
        self.competent_fall.max_passes = self.competent_max_drop as u32;
        self.competent_fall.min_impact_fall_cells = self.competent_min_impact as u32;
        self.competent_fall.max_roll_events = self.competent_max_rolls as u32;
        self.competent_fall.enable = self.failure.enable_competent_fall;
    }

    /// Push population ceilings onto the live organism store.
    pub fn apply_pop_caps(&self, organisms: &mut wk_voxel::OrganismStore) {
        organisms.max_atoms = self.max_atoms.round().clamp(1.0, 4096.0) as usize;
        organisms.max_corpses = self.max_corpses.round().clamp(1.0, 4096.0) as usize;
        organisms.growth_caps = PlantGrowthCaps {
            max_roots: self.max_roots.round().clamp(1.0, 256.0) as usize,
            max_stems: self.max_stems.round().clamp(0.0, 256.0) as usize,
            max_photos: self.max_photos.round().clamp(1.0, 256.0) as usize,
        }
        .clamp();
        organisms.fungi = self.fungi;
        organisms.spore_bank = self.spore_bank;
    }

    /// Pull growth + pop caps from a loaded organism store into the UI.
    pub fn sync_caps_from_organisms(&mut self, organisms: &wk_voxel::OrganismStore) {
        self.max_atoms = organisms.max_atoms as f32;
        self.max_corpses = organisms.max_corpses as f32;
        self.max_roots = organisms.growth_caps.max_roots as f32;
        self.max_stems = organisms.growth_caps.max_stems as f32;
        self.max_photos = organisms.growth_caps.max_photos as f32;
    }
}

fn material_short_name(id: MaterialId) -> &'static str {
    match id {
        MaterialId::Bedrock => "Bedrock",
        MaterialId::Stone => "Stone",
        MaterialId::Sand => "Sand",
        MaterialId::Clay => "Clay",
        MaterialId::Soil => "Soil",
        MaterialId::Organic => "Organic",
        MaterialId::LooseRock => "LooseRock",
        MaterialId::LooseLimestone => "LooseLimestone",
        MaterialId::Flowstone => "Flowstone",
        MaterialId::Gravel => "Gravel",
        MaterialId::Limestone => "Limestone",
        _ => "?",
    }
}

/// Full label on its own line; slider uses a blank label so text never
/// clips against the window's right edge (macroquad packs label after
/// the track).
fn labeled_slider(
    ui: &mut macroquad::ui::Ui,
    id: macroquad::ui::Id,
    label: &str,
    range: std::ops::Range<f32>,
    value: &mut f32,
) {
    ui.label(None, label);
    ui.slider(id, "", range, value);
}
