//! Live settings menu for wk-voxel-app (Tab to toggle).
//!
//! Isolation: wk-voxel + wk-material + macroquad only.

use macroquad::prelude::*;
use macroquad::ui::{hash, root_ui, widgets};
use wk_material::{MaterialId, MaterialRegistry};
use wk_voxel::{
    ClimateConfig, CloudConfig, CondensationConfig, EvapConfig, KarstConfig, OrographicConfig,
    RainConfig, TempConfig, WorldgenParams,
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

        widgets::Window::new(hash!(), vec2(16.0, 16.0), vec2(560.0, 700.0))
            .label("Settings (Tab to close)")
            .ui(&mut *root_ui(), |ui| {
                ui.tree_node(hash!(), "Day / night / temperature", |ui| {
                    ui.slider(hash!(), "Day length (ticks)", 60.0..6_000.0, &mut day_ticks);
                    ui.slider(hash!(), "Night length (ticks)", 60.0..6_000.0, &mut night_ticks);
                    ui.label(
                        None,
                        &format!(
                            "  cycle ≈ {:.1}s at 60 tick/s",
                            (day_ticks + night_ticks) / 60.0
                        ),
                    );
                    ui.slider(hash!(), "Base temp (C)", -20.0..40.0, &mut self.temp.base_temp_c);
                    ui.slider(hash!(), "Day/night swing (C)", 0.0..20.0, &mut self.temp.day_amp_c);
                    ui.slider(hash!(), "Lapse (C / cell elev)", 0.0..0.4, &mut self.temp.lapse_c);
                    ui.slider(hash!(), "Solar heat / step", 0.0..1.5, &mut self.temp.solar_heat_c);
                    ui.slider(hash!(), "Night cool / step", 0.0..1.5, &mut self.temp.night_cool_c);
                    ui.slider(hash!(), "Cloud shade", 0.0..1.0, &mut self.temp.cloud_shade);
                    ui.slider(hash!(), "Sea bias (C)", -10.0..5.0, &mut self.temp.sea_bias_c);
                });
                ui.separator();

                ui.tree_node(hash!(), "Wind + humidity", |ui| {
                    ui.slider(hash!(), "Wind (tiles/tick)", -0.5..0.5, &mut self.wind_vx);
                    ui.slider(
                        hash!(),
                        "Humidity diffuse alpha",
                        0.0..0.25,
                        &mut self.humidity_diffusion_alpha,
                    );
                    ui.slider(
                        hash!(),
                        "Buoyant rise / tick",
                        0.0..0.4,
                        &mut self.cloud.buoyant_rise,
                    );
                });
                ui.separator();

                ui.tree_node(hash!(), "Clouds", |ui| {
                    ui.slider(hash!(), "Max parcels", 1.0..64.0, &mut max_parcels);
                    ui.slider(hash!(), "Coag min humidity", 1.0..120.0, &mut self.cloud.coag_min_hum);
                    ui.slider(hash!(), "Coag rate", 0.005..0.25, &mut self.cloud.coag_rate);
                    ui.slider(hash!(), "Coag max take", 1.0..80.0, &mut self.cloud.coag_max_take);
                    ui.slider(hash!(), "Spawn radius", 4.0..48.0, &mut self.cloud.spawn_radius);
                    ui.slider(hash!(), "Merge distance", 2.0..30.0, &mut self.cloud.merge_dist);
                    ui.slider(hash!(), "Downpour mass", 40.0..500.0, &mut self.cloud.downpour_mass);
                    ui.slider(hash!(), "Downpour drain", 4.0..120.0, &mut self.cloud.downpour_drain);
                    ui.slider(
                        hash!(),
                        "Parcel wind scale",
                        0.05..1.5,
                        &mut self.cloud.parcel_wind_scale,
                    );
                    ui.slider(hash!(), "Cloud alt above sea", 8.0..80.0, &mut cloud_alt);
                    ui.slider(hash!(), "Coag min above sea", 4.0..60.0, &mut coag_min_alt);
                    ui.slider(
                        hash!(),
                        "Ridge clearance",
                        0.0..20.0,
                        &mut self.cloud.ridge_clearance,
                    );
                });
                ui.separator();

                ui.tree_node(hash!(), "Rain / drizzle / evap", |ui| {
                    ui.slider(
                        hash!(),
                        "Climatic rain prob",
                        0.0..0.2,
                        &mut self.rain.prob_per_col_per_tick,
                    );
                    ui.slider(hash!(), "Climatic droplet sat", 1.0..255.0, &mut droplet);
                    ui.slider(
                        hash!(),
                        "Drizzle min mass",
                        10.0..400.0,
                        &mut self.cond.min_mass_to_rain,
                    );
                    ui.slider(
                        hash!(),
                        "Drizzle max prob",
                        0.0..1.0,
                        &mut self.cond.max_prob_per_tick,
                    );
                    ui.slider(
                        hash!(),
                        "Drizzle mass / drop",
                        4.0..200.0,
                        &mut self.cond.mass_per_droplet,
                    );
                    ui.slider(hash!(), "Evap rate / pulse", 0.0..8.0, &mut evap_rate);
                    ui.slider(hash!(), "Evap period (ticks)", 1.0..30.0, &mut evap_period);
                    ui.slider(hash!(), "Evap dry-above max", 0.0..255.0, &mut dry_above);
                    ui.slider(hash!(), "Oro tall above sea", 4.0..60.0, &mut tall_above);
                    ui.slider(
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
                        ui.slider(
                            hash!(name, "perm"),
                            &format!("{name} permeability"),
                            0.0..255.0,
                            &mut self.mat_perm[i],
                        );
                        ui.slider(
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
                    ui.slider(
                        hash!(),
                        "Dissolve prob / wet nbr",
                        0.0..0.05,
                        &mut self.karst.prob_per_wet_neighbour,
                    );
                    let mut min_sat = self.karst.min_wet_neighbour_sat as f32;
                    ui.slider(hash!(), "Min wet neighbour sat", 1.0..255.0, &mut min_sat);
                    self.karst.min_wet_neighbour_sat = min_sat.round().clamp(1.0, 255.0) as u8;
                });
                ui.separator();
                ui.label(None, "Tip: Tab closes · F2 creature editor · F1 tools");
            });

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
        self.cloud.cloud_alt_above_sea = cloud_alt.round().clamp(4.0, 120.0) as i32;
        self.cloud.coag_min_above_sea = coag_min_alt.round().clamp(2.0, 100.0) as i32;
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
