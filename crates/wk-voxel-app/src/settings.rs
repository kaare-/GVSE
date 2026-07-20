//! Live settings menu for wk-voxel-app (Tab to toggle).
//!
//! Isolation: wk-voxel + wk-material + macroquad only.

use macroquad::prelude::*;
use macroquad::ui::{hash, root_ui, widgets};
use wk_material::{MaterialId, MaterialRegistry};
use wk_voxel::{
    ClimateConfig, CloudConfig, CondensationConfig, EvapConfig, GrainConfig, KarstConfig,
    OrographicConfig, PhaseConfig, RainConfig, TempConfig, WorldgenParams,
};

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
    pub wind_vx: f32,
    pub humidity_diffusion_alpha: f32,
    /// Scratch f32s for material sliders (synced → MaterialRegistry overrides).
    pub mat_perm: [f32; 12],
    pub mat_poro: [f32; 12],
}

impl SimSettings {
    pub fn new(params: &WorldgenParams) -> Self {
        let rain = RainConfig {
            top_y: params.sky_ceiling_y - 1,
            x_range: (0, params.width_cols - 1),
            prob_per_col_per_tick: 0.02,
            droplet_sat: 64,
            seed_salt: 0xC10D_5EED,
        };
        let mut cond = CondensationConfig {
            top_y: params.sky_ceiling_y - 2,
            ..CondensationConfig::default()
        };
        // Match previous demo drizzle defaults.
        cond.min_mass_to_rain = 140.0;
        cond.max_prob_per_tick = 0.10;
        cond.mass_per_droplet = 40.0;

        let mut mat_perm = [0.0f32; 12];
        let mut mat_poro = [0.0f32; 12];
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
            wind_vx: 0.05,
            humidity_diffusion_alpha: 0.15,
            mat_perm,
            mat_poro,
        }
    }

    pub fn on_world_reseed(&mut self, params: &WorldgenParams) {
        self.rain.top_y = params.sky_ceiling_y - 1;
        self.rain.x_range = (0, params.width_cols - 1);
        self.cond.top_y = params.sky_ceiling_y - 2;
        self.oro.seed = params.seed;
        self.oro.width_cols = params.width_cols;
        self.oro.sea_level_y = params.sea_level_y;
    }

    /// Push material slider values into the shared registry overrides.
    pub fn apply_material_overrides(&self) {
        for id in MaterialId::ALL_SOLIDS {
            let i = id as usize;
            MaterialRegistry::set_permeability_override(id, self.mat_perm[i].round() as u8);
            MaterialRegistry::set_porosity_override(id, self.mat_poro[i].round() as u8);
        }
    }

    pub fn reset_materials_to_defaults(&mut self) {
        MaterialRegistry::clear_hydro_overrides();
        for id in MaterialId::ALL_SOLIDS {
            let i = id as usize;
            let base = MaterialRegistry::base_props(id);
            self.mat_perm[i] = base.permeability as f32;
            self.mat_poro[i] = base.porosity as f32;
        }
    }

    pub fn draw(&mut self) {
        if !self.open {
            return;
        }

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
                    let mut period = self.phase.period_ticks as f32;
                    labeled_slider(ui, hash!(), "Min sat to freeze", 1.0..255.0, &mut min_freeze);
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
                        "Min precip budget to snow",
                        1.0..255.0,
                        &mut self.phase.min_budget_to_snow,
                    );
                    labeled_slider(ui, hash!(), "Snow spread radius (cols)", 0.0..24.0, &mut spread);
                    labeled_slider(ui, hash!(), "Snow blanket prefer depth", 0.0..12.0, &mut blanket);
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
                    self.phase.period_ticks = period.round().clamp(1.0, 120.0) as u64;
                    self.phase.min_budget_to_snow =
                        self.phase.min_budget_to_snow.clamp(1.0, 255.0);
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
                });
                ui.separator();

                ui.tree_node(hash!(), "Wind + humidity", |ui| {
                    labeled_slider(ui, hash!(), "Wind (tiles/tick)", -0.5..0.5, &mut self.wind_vx);
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
                });
                ui.separator();

                ui.tree_node(hash!(), "Rain / drizzle / evap", |ui| {
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
                ui.separator();
                ui.label(None, "Tip: Tab closes · F2 creature editor · F1 tools");
            });

        self.karst.min_wet_neighbour_sat = min_sat.round().clamp(1.0, 255.0) as u8;

        if reset_materials {
            self.reset_materials_to_defaults();
        } else {
            self.apply_material_overrides();
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
    }
}

fn material_short_name(id: MaterialId) -> &'static str {
    match id {
        MaterialId::Bedrock => "Bedrock",
        MaterialId::Stone => "Stone",
        MaterialId::Sand => "Sand",
        MaterialId::Clay => "Clay",
        MaterialId::Organic => "Organic",
        MaterialId::LooseRock => "LooseRock",
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
