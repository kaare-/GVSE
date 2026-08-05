//! Live settings menu for wk-voxel-app (Tab to toggle).
//!
//! Isolation: wk-voxel + wk-material + macroquad only.

use macroquad::prelude::*;
use macroquad::ui::{hash, root_ui, widgets};
use wk_material::{MaterialId, MaterialRegistry, MATERIAL_COUNT};
use wk_voxel::{
    CarbonBudget, CarbonConfig, ClimateConfig, CloudConfig, CondensationConfig, EvapConfig,
    FailureConfig, FungiConfig, Genome, GrainConfig, KarstConfig, OrographicConfig, PerfConfig,
    PhaseConfig, PlantGrowthCaps, RainConfig, TempConfig, World, WorldgenParams, CHUNK_CELLS_W,
    MAX_ATOMS, MAX_CORPSES, MAX_PHOTO_MODULES, MAX_ROOT_MODULES, MAX_STEM_MODULES,
};

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
}

/// All live-tunable knobs for the voxel demo.
#[derive(Debug, Clone)]
pub struct SimSettings {
    pub open: bool,
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
    /// Mycelium compost knobs (Tab → Life → Fungi / compost).
    pub fungi: FungiConfig,
    /// Crude CO₂ buckets (Tab → Life → Carbon).
    pub carbon: CarbonConfig,
    /// Which settings page is open.
    pub page: SettingsPage,
    /// Physics trade-offs (Tab → Performance). Defaults preserve water feel.
    pub perf: PerfConfig,
    /// Geotech failure (Tab → Geotech). Roof collapse on by default.
    pub failure: FailureConfig,
    /// Scratch f32 for max roof events slider.
    pub max_roof_events: f32,
    /// Scratch f32 for max shear events slider.
    pub max_shear_events: f32,
    /// Scratch f32 for max compaction events slider.
    pub max_compaction_events: f32,
    /// Scratch f32 for shear chance (percent UI → per-mille).
    pub shear_chance_pct: f32,
    pub wind_vx: f32,
    /// Natural variance 0..1 — wind force and direction wander around the mean.
    pub wind_variance: f32,
    pub humidity_diffusion_alpha: f32,
    /// Scratch f32s for material sliders (synced → world hydro overrides).
    pub mat_perm: [f32; MATERIAL_COUNT],
    pub mat_poro: [f32; MATERIAL_COUNT],
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

        let mut mat_perm = [0.0f32; MATERIAL_COUNT];
        let mut mat_poro = [0.0f32; MATERIAL_COUNT];
        for id in MaterialId::ALL_SOLIDS {
            let i = id as usize;
            let base = MaterialRegistry::base_props(id);
            mat_perm[i] = base.permeability as f32;
            mat_poro[i] = base.porosity as f32;
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
            oro: OrographicConfig {
                seed: params.seed,
                width_cols: params.width_cols,
                sea_level_y: params.sea_level_y,
                ..OrographicConfig::default()
            },
            karst: KarstConfig::default(),
            cloud: CloudConfig::default(),
            climate: ClimateConfig::default(),
            temp: TempConfig::default(),
            phase: PhaseConfig::default(),
            grain: GrainConfig::default(),
            fungi: FungiConfig::default(),
            carbon: CarbonConfig::default(),
            page: SettingsPage::World,
            perf: PerfConfig::default(),
            failure: FailureConfig::default(),
            max_roof_events: FailureConfig::default().max_roof_events as f32,
            max_shear_events: FailureConfig::default().max_shear_events as f32,
            max_compaction_events: FailureConfig::default().max_compaction_events as f32,
            shear_chance_pct: FailureConfig::default().shear_chance_per_mille as f32 / 10.0,
            wind_vx: 0.05,
            wind_variance: 0.55,
            humidity_diffusion_alpha: 0.15,
            mat_perm,
            mat_poro,
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
        }
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
            world
                .hydro
                .set_permeability(id, self.mat_perm[i].round() as u8);
            world
                .hydro
                .set_porosity(id, self.mat_poro[i].round() as u8);
        }
    }

    pub fn reset_materials_to_defaults(&mut self, world: &mut World) {
        world.hydro.clear();
        for id in MaterialId::ALL_SOLIDS {
            let i = id as usize;
            let base = MaterialRegistry::base_props(id);
            self.mat_perm[i] = base.permeability as f32;
            self.mat_poro[i] = base.porosity as f32;
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
                        SettingsPage::Climate => "Climate — day/night, ice, wind, clouds, rain",
                        SettingsPage::Physics => "Physics — performance, geotech, grain",
                        SettingsPage::Life => "Life — creatures, plants, fungi compost, carbon",
                    },
                );
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
                    labeled_slider(ui, hash!(), "Cloud shade", 0.0..1.0, &mut self.temp.cloud_shade);
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
                        "Snow precip (air cold; melts on warm ground)",
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
                        "Defaults = full water feel. Toggle to A/B cost vs cascade/leveling.",
                    );
                    ui.checkbox(
                        hash!(),
                        "Flow every other substep (gravity still ×12)",
                        &mut self.perf.flow_every_other_substep,
                    );
                    ui.label(
                        None,
                        "  Off = tuned feel. On ≈ half surface-flow work — watch shores.",
                    );
                    ui.checkbox(
                        hash!(),
                        "Quiet flow early-out (tiny dirty halo)",
                        &mut self.perf.flow_quiet_early_out,
                    );
                    ui.label(
                        None,
                        "  Can stall hill drains / shelf cascades — compare carefully.",
                    );
                    ui.checkbox(
                        hash!(),
                        "Parallel physics (rayon checkerboard)",
                        &mut self.perf.parallel_physics,
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
                         bind radius dilates root rafts (0 = body span only).",
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

                ui.tree_node(hash!(), "Clouds", |ui| {
                    labeled_slider(ui, hash!(), "Max parcels", 1.0..64.0, &mut max_parcels);
                    labeled_slider(ui, hash!(), "Coag min humidity", 1.0..120.0, &mut self.cloud.coag_min_hum);
                    labeled_slider(ui, hash!(), "Coag rate", 0.005..0.25, &mut self.cloud.coag_rate);
                    labeled_slider(ui, hash!(), "Coag max take", 1.0..80.0, &mut self.cloud.coag_max_take);
                    labeled_slider(ui, hash!(), "Spawn radius", 4.0..48.0, &mut self.cloud.spawn_radius);
                    labeled_slider(ui, hash!(), "Merge distance", 2.0..30.0, &mut self.cloud.merge_dist);
                    labeled_slider(ui, hash!(), "Downpour mass", 40.0..500.0, &mut self.cloud.downpour_mass);
                    labeled_slider(ui, hash!(), "Downpour drain", 4.0..120.0, &mut self.cloud.downpour_drain);
                    labeled_slider(
                        ui,
                        hash!(),
                        "Parcel wind scale",
                        0.05..1.5,
                        &mut self.cloud.parcel_wind_scale,
                    );
                    labeled_slider(ui, hash!(), "Cloud alt above sea", 8.0..160.0, &mut cloud_alt);
                    labeled_slider(ui, hash!(), "Coag min above sea", 4.0..120.0, &mut coag_min_alt);
                    labeled_slider(
                        ui,
                        hash!(),
                        "Ridge clearance",
                        0.0..36.0,
                        &mut self.cloud.ridge_clearance,
                    );
                    let mut snow_cells = self.cloud.snow_cells_per_tick as f32;
                    let mut rain_cells = self.cloud.rain_cells_per_tick as f32;
                    labeled_slider(
                        ui,
                        hash!(),
                        "Snow footprint × radius",
                        0.5..4.0,
                        &mut self.cloud.snow_footprint_mult,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Rain footprint × radius",
                        0.5..3.0,
                        &mut self.cloud.rain_footprint_mult,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Snow landing span × radius",
                        0.2..3.0,
                        &mut self.cloud.snow_span_mult,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Rain landing span × radius",
                        0.2..2.0,
                        &mut self.cloud.rain_span_mult,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Snow cells / parcel / tick",
                        1.0..12.0,
                        &mut snow_cells,
                    );
                    labeled_slider(
                        ui,
                        hash!(),
                        "Rain cell retries / parcel / tick",
                        1.0..8.0,
                        &mut rain_cells,
                    );
                    self.cloud.snow_cells_per_tick = snow_cells.round().clamp(1.0, 24.0) as u8;
                    self.cloud.rain_cells_per_tick = rain_cells.round().clamp(1.0, 16.0) as u8;
                });
                ui.separator();

                ui.tree_node(hash!(), "Rain / drizzle / evap", |ui| {
                    ui.checkbox(
                        hash!(),
                        "Climatic rain closed-loop (drain humidity; no mint)",
                        &mut self.rain.closed_loop,
                    );
                    let mut flood = self.rain.max_flood_above_sea as f32;
                    labeled_slider(
                        ui,
                        hash!(),
                        "Flood guard (cells above sea; 0=off)",
                        0.0..48.0,
                        &mut flood,
                    );
                    self.rain.max_flood_above_sea = flood.round().clamp(0.0, 64.0) as i32;
                    labeled_slider(
                        ui,
                        hash!(),
                        "Climatic rain prob",
                        0.0..0.2,
                        &mut self.rain.prob_per_col_per_tick,
                    );
                    labeled_slider(ui, hash!(), "Climatic droplet sat", 1.0..255.0, &mut droplet);
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
                });
                } // Climate (wind/clouds/rain)
                if self.page == SettingsPage::World {
                ui.separator();

                ui.tree_node(hash!(), "Material permeability / porosity", |ui| {
                    ui.label(None, "Affects seepage rate and water capacity.");
                    for id in MaterialId::ALL_SOLIDS {
                        let i = id as usize;
                        let name = material_short_name(id);
                        labeled_slider(
                            ui,
                            hash!(name, "perm"),
                            &format!("{name} permeability"),
                            0.0..255.0,
                            &mut self.mat_perm[i],
                        );
                        labeled_slider(
                            ui,
                            hash!(name, "poro"),
                            &format!("{name} porosity"),
                            0.0..255.0,
                            &mut self.mat_poro[i],
                        );
                    }
                    if ui.button(None, "Reset materials to defaults") {
                        reset_materials = true;
                    }
                });
                ui.separator();

                ui.tree_node(hash!(), "Karst", |ui| {
                    labeled_slider(
                        ui,
                        hash!(),
                        "Dissolve prob / wet neighbour",
                        0.0..0.05,
                        &mut self.karst.prob_per_wet_neighbour,
                    );
                    labeled_slider(ui, hash!(), "Min wet neighbour sat", 1.0..255.0, &mut min_sat);
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

                ui.tree_node(hash!(), "Plant growth caps", |ui| {
                    ui.label(
                        None,
                        "Per-plant tissue pixel ceilings (not entity pop).",
                    );
                    ui.label(
                        None,
                        "Raising these lets individuals grow denser; lowering only blocks new pixels.",
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
                        "Crude atmosphere + dissolved pools. Surface Organic oxidizes to Soil                          and credits atm C (not humidity). Lakes exchange atm ↔ dissolved.                          Later: algae draw dissolved; O2 bucket when animals land.",
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
                } // Life
                ui.separator();
                ui.label(
                    None,
                    "Tip: Tab closes · pages above · F2 creatures · F3 terrain · F5/F9 save/load",
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
        self.cloud.downpour_stop_frac = self.cloud.downpour_stop_frac.clamp(0.05, 0.95);
        self.cloud.snow_footprint_mult = self.cloud.snow_footprint_mult.clamp(0.1, 6.0);
        self.cloud.rain_footprint_mult = self.cloud.rain_footprint_mult.clamp(0.1, 4.0);
        self.cloud.snow_span_mult = self.cloud.snow_span_mult.clamp(0.05, 4.0);
        self.cloud.rain_span_mult = self.cloud.rain_span_mult.clamp(0.05, 3.0);
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
