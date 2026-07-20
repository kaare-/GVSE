//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Coarse **thermal field** (°C) on the same 4×4 tile grid as humidity /
//! wind. Climate sets a skin target; landscape and water supply
//! **heat capacity** and **albedo** so ponds and peaks lag snaps instead
//! of freezing/thawing the instant Tab moves base temp.
//!
//! Cadence matches humidity diffuse ([`TEMP_STEP_PERIOD`] = 20) — not
//! every physics tick. Organics can later read the same lagged field.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use wk_material::{MaterialId, MaterialRegistry};

use crate::cell::Cell;
use crate::climate::{day_night_factor_cfg, ClimateConfig};
use crate::grid::World;
use crate::humidity::{Humidity, TileBounds};
use crate::worldgen::continental_surface_y;

/// Cadence for temperature steps — same period as humidity diffuse,
/// phase 0 so the two don't always land on the same tick.
pub const TEMP_STEP_PERIOD: u64 = 20;
pub const TEMP_STEP_PHASE: u64 = 0;

pub fn temperature_step_due(tick: u64) -> bool {
    tick % TEMP_STEP_PERIOD == TEMP_STEP_PHASE
}

/// Live-tunable temperature / solar / inertia knobs.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct TempConfig {
    pub base_temp_c: f32,
    pub sea_bias_c: f32,
    pub land_day_bump_c: f32,
    pub lapse_c: f32,
    pub day_amp_c: f32,
    pub solar_heat_c: f32,
    pub night_cool_c: f32,
    pub cloud_shade: f32,
    pub hum_shade_ref: f32,
    /// Base relax rate toward climate skin (air-like surfaces).
    pub sky_relax: f32,
    pub diffuse_alpha: f32,
    /// Scales material [`MaterialProps::heat_capacity`] into inertia:
    /// `relax = sky_relax / (1 + capacity * inertia_scale)`.
    pub inertia_scale: f32,
    /// Floor / ceiling on per-tile relax so deep water still moves,
    /// and bare air doesn't stick.
    pub min_relax: f32,
    pub max_relax: f32,
    /// Extra capacity per standing-water / ice cell in the surface stack
    /// (lakes hold more heat than a 1-cell film).
    pub water_stack_cap: f32,
}

impl Default for TempConfig {
    fn default() -> Self {
        Self {
            base_temp_c: 18.0,
            sea_bias_c: -2.0,
            land_day_bump_c: 1.5,
            lapse_c: 0.08,
            day_amp_c: 6.0,
            solar_heat_c: 0.40,
            night_cool_c: 0.30,
            cloud_shade: 0.55,
            hum_shade_ref: 80.0,
            sky_relax: 0.10,
            diffuse_alpha: 0.12,
            inertia_scale: 0.55,
            min_relax: 0.012,
            max_relax: 0.22,
            water_stack_cap: 0.35,
        }
    }
}

/// Sparse (but usually dense-filled) temperature field in °C.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Temperature {
    pub tile_cols: i32,
    pub cells: HashMap<(i32, i32), f32>,
    pub bounds: Option<TileBounds>,
    pub wrap_x: bool,
    pub seed: u64,
    pub width_cols: i32,
    pub sea_level_y: i32,
    #[serde(default)]
    pub config: TempConfig,
    #[serde(default)]
    pub climate: ClimateConfig,
}

impl Temperature {
    pub fn with_world_bounds(
        tile_cols: i32,
        x0: i32,
        y0: i32,
        x1: i32,
        y1: i32,
        seed: u64,
        width_cols: i32,
        sea_level_y: i32,
        wrap_x: bool,
    ) -> Self {
        let tile_cols = tile_cols.max(1);
        let mut t = Self {
            tile_cols,
            cells: HashMap::new(),
            bounds: Some(TileBounds::from_world_cells(tile_cols, x0, y0, x1, y1)),
            wrap_x,
            seed,
            width_cols: width_cols.max(1),
            sea_level_y,
            config: TempConfig::default(),
            climate: ClimateConfig::default(),
        };
        t.fill_initial(0);
        t
    }

    fn accepts(&self, hx: i32, hy: i32) -> bool {
        self.bounds.map(|b| b.contains(hx, hy)).unwrap_or(true)
    }

    fn wrap_hx(&self, hx: i32) -> Option<i32> {
        match self.bounds {
            Some(b) if self.wrap_x => {
                let w = b.hx_max - b.hx_min + 1;
                if w <= 0 {
                    return None;
                }
                Some(b.hx_min + (hx - b.hx_min).rem_euclid(w))
            }
            Some(b) => {
                if hx >= b.hx_min && hx <= b.hx_max {
                    Some(hx)
                } else {
                    None
                }
            }
            None => Some(hx),
        }
    }

    pub fn tile_of(&self, gx: i32, gy: i32) -> (i32, i32) {
        (gx.div_euclid(self.tile_cols), gy.div_euclid(self.tile_cols))
    }

    pub fn at_tile(&self, hx: i32, hy: i32) -> f32 {
        *self
            .cells
            .get(&(hx, hy))
            .unwrap_or(&self.config.base_temp_c)
    }

    pub fn at_cell(&self, gx: i32, gy: i32) -> f32 {
        let (hx, hy) = self.tile_of(gx, gy);
        self.at_tile(hx, hy)
    }

    pub fn mean(&self) -> f32 {
        if self.cells.is_empty() {
            return self.config.base_temp_c;
        }
        self.cells.values().sum::<f32>() / self.cells.len() as f32
    }

    /// Fill every in-bounds tile from climate skin at `tick`.
    pub fn fill_initial(&mut self, tick: u64) {
        let Some(b) = self.bounds else {
            return;
        };
        self.cells.clear();
        for hy in b.hy_min..=b.hy_max {
            for hx in b.hx_min..=b.hx_max {
                self.cells.insert((hx, hy), self.skin_temp(hx, hy, tick));
            }
        }
    }

    fn land_factor(&self, hx: i32) -> f32 {
        let tc = self.tile_cols.max(1);
        let gx = hx * tc + tc / 2;
        let s = continental_surface_y(self.seed, gx, self.sea_level_y, self.width_cols);
        let d = (s - self.sea_level_y) as f32;
        ((d + 2.0) / 4.0).clamp(0.0, 1.0)
    }

    fn elev_cells(&self, hx: i32) -> f32 {
        let tc = self.tile_cols.max(1);
        let gx = hx * tc + tc / 2;
        let s = continental_surface_y(self.seed, gx, self.sea_level_y, self.width_cols);
        (s - self.sea_level_y).max(0) as f32
    }

    /// Target skin temperature for a tile at climate phase `tick`.
    pub fn skin_temp(&self, hx: i32, hy: i32, tick: u64) -> f32 {
        let _ = hy;
        let cfg = &self.config;
        let dn = day_night_factor_cfg(tick, &self.climate);
        let land = self.land_factor(hx);
        let elev = self.elev_cells(hx);
        let sea_land =
            cfg.sea_bias_c * (1.0 - land) + cfg.land_day_bump_c * land * dn.max(0.0);
        cfg.base_temp_c + sea_land - cfg.lapse_c * elev + cfg.day_amp_c * dn
    }

    /// Climate-driven thermal step with material inertia + light diffusion.
    ///
    /// `world` supplies surface heat capacity / albedo (water, ice, sand…).
    /// Pass `None` only in unit tests that exercise the air-like path.
    pub fn step(&mut self, world: Option<&World>, humidity: &Humidity, tick: u64) {
        if self.cells.is_empty() {
            self.fill_initial(tick);
        }
        let dn = day_night_factor_cfg(tick, &self.climate);
        let cfg = self.config;
        let keys: Vec<(i32, i32)> = self.cells.keys().copied().collect();
        for (hx, hy) in keys {
            let (cap, albedo) = tile_thermal_props(self, world, hx, hy);
            let shade = (humidity.at_tile(hx, hy) / cfg.hum_shade_ref.max(1.0)).clamp(0.0, 1.0);
            let solar = cfg.solar_heat_c
                * dn.max(0.0)
                * (1.0 - cfg.cloud_shade * shade)
                * (1.0 - albedo.clamp(0.0, 0.95));
            let cool = cfg.night_cool_c * (-dn).max(0.0);
            let skin = self.skin_temp(hx, hy, tick);
            let t = self.at_tile(hx, hy);
            let relax = (cfg.sky_relax / (1.0 + cap.max(0.05) * cfg.inertia_scale))
                .clamp(cfg.min_relax, cfg.max_relax);
            let mut next = t + solar - cool;
            next = next + (skin - next) * relax;
            self.cells.insert((hx, hy), next);
        }
        self.diffuse(cfg.diffuse_alpha);
    }

    /// Pairwise temperature diffusion (does not prune tiles — cold air
    /// is still a real temperature).
    pub fn diffuse(&mut self, alpha: f32) {
        let alpha = alpha.clamp(0.0, 0.25);
        if alpha == 0.0 || self.cells.is_empty() {
            return;
        }
        let snap = self.cells.clone();
        let mut sources: Vec<(i32, i32)> = snap.keys().copied().collect();
        sources.sort_unstable();
        sources.dedup();
        let mut deltas: HashMap<(i32, i32), f32> = HashMap::new();
        let base = self.config.base_temp_c;
        for &(hx, hy) in &sources {
            let val = *snap.get(&(hx, hy)).unwrap_or(&base);
            if let Some(nx) = self.wrap_hx(hx + 1) {
                if self.accepts(nx, hy) && nx != hx {
                    let n_val = *snap.get(&(nx, hy)).unwrap_or(&base);
                    let flow = (val - n_val) * alpha;
                    if flow.abs() >= 1e-9 {
                        *deltas.entry((hx, hy)).or_insert(0.0) -= flow;
                        *deltas.entry((nx, hy)).or_insert(0.0) += flow;
                    }
                }
            }
            // Vertical mix is gentler so fast air tiles don't instantly
            // drain heat out of high-capacity surface / lake tiles.
            let n_key = (hx, hy + 1);
            if self.accepts(n_key.0, n_key.1) {
                let n_val = *snap.get(&n_key).unwrap_or(&base);
                let flow = (val - n_val) * alpha * 0.45;
                if flow.abs() >= 1e-9 {
                    *deltas.entry((hx, hy)).or_insert(0.0) -= flow;
                    *deltas.entry(n_key).or_insert(0.0) += flow;
                }
            }
        }
        for (k, d) in deltas {
            if self.accepts(k.0, k.1) {
                *self.cells.entry(k).or_insert(base) += d;
            }
        }
    }
}

/// Heat capacity + albedo for a temperature tile from voxel columns in
/// the tile (averaged — a 4-wide tile may mix shore and pond).
fn tile_thermal_props(
    temp: &Temperature,
    world: Option<&World>,
    hx: i32,
    hy: i32,
) -> (f32, f32) {
    let air = MaterialRegistry::props(MaterialId::Air);
    let Some(world) = world else {
        return (air.heat_capacity, air.albedo);
    };
    let tc = temp.tile_cols.max(1);
    let tile_mid_y = hy * tc + tc / 2;
    let (y_lo, y_hi) = match temp.bounds {
        Some(b) => (b.hy_min * tc - 2, b.hy_max * tc + tc + 2),
        None => {
            let gx0 = world.wrap_x(hx * tc);
            let rock = continental_surface_y(temp.seed, gx0, temp.sea_level_y, temp.width_cols);
            (rock - 8, rock + 64)
        }
    };
    let mut cap_sum = 0.0;
    let mut alb_sum = 0.0;
    let mut surf_sum = 0.0;
    let mut n = 0.0;
    for lx in 0..tc {
        let gx = world.wrap_x(hx * tc + lx);
        let rock = continental_surface_y(temp.seed, gx, temp.sea_level_y, temp.width_cols);
        let (surf_y, cap, albedo) =
            column_surface_thermal(world, gx, y_lo, y_hi, rock, &temp.config);
        cap_sum += cap;
        alb_sum += albedo;
        surf_sum += surf_y as f32;
        n += 1.0;
    }
    if n < 1.0 {
        return (air.heat_capacity, air.albedo);
    }
    let surf_y = (surf_sum / n).round() as i32;
    let cap = cap_sum / n;
    let albedo = alb_sum / n;
    // Free-air tiles above the column top track climate quickly.
    if tile_mid_y > surf_y + tc {
        return (air.heat_capacity, air.albedo);
    }
    // Buried rock tiles: solid capacity, no solar albedo.
    if tile_mid_y + tc < surf_y {
        let stone = MaterialRegistry::props(MaterialId::Stone);
        return (stone.heat_capacity * 1.15, 0.0);
    }
    (cap, albedo)
}

/// Scan a column for the surface stack: pack / water / ground.
/// Returns `(surface_y, heat_capacity, albedo)`.
fn column_surface_thermal(
    world: &World,
    gx: i32,
    y_lo: i32,
    y_hi: i32,
    fallback_y: i32,
    cfg: &TempConfig,
) -> (i32, f32, f32) {
    let gx = world.wrap_x(gx);
    let mut top_y = fallback_y;
    let mut top_cell: Option<Cell> = None;
    // Top-down: first solid / wet / frozen cell is the free surface.
    for y in (y_lo..=y_hi).rev() {
        let Some(cell) = world.get_cell(gx, y) else {
            continue;
        };
        let wet_air = cell.material == MaterialId::Air && !cell.sat.is_empty();
        if cell.material != MaterialId::Air || wet_air {
            top_y = y;
            top_cell = Some(cell);
            break;
        }
    }
    let mut water_like = 0i32;
    for y in (y_lo..=top_y).rev() {
        let Some(cell) = world.get_cell(gx, y) else {
            continue;
        };
        let wet_air = cell.material == MaterialId::Air && !cell.sat.is_empty();
        let frozen = matches!(cell.material, MaterialId::Ice | MaterialId::Snow);
        if wet_air || cell.material == MaterialId::Water || frozen {
            water_like += 1;
        } else {
            break; // ground or empty under the stack
        }
    }
    let cell = top_cell.unwrap_or(Cell::solid(MaterialId::Stone));
    let mat = if cell.material == MaterialId::Air && !cell.sat.is_empty() {
        MaterialId::Water
    } else {
        cell.material
    };
    let props = MaterialRegistry::props(mat);
    let stack = (water_like.saturating_sub(1) as f32).max(0.0);
    let cap = props.heat_capacity + stack * cfg.water_stack_cap;
    (top_y, cap, props.albedo)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Cell;
    use crate::chunk::ChunkCoord;
    use crate::climate::DEMO_DAY_TICKS;
    use crate::worldgen::WorldgenParams;

    fn demo_temp() -> (Temperature, Humidity) {
        let p = WorldgenParams::default();
        let t = Temperature::with_world_bounds(
            4,
            0,
            p.bedrock_floor_y,
            p.width_cols,
            p.sky_ceiling_y,
            p.seed,
            p.width_cols,
            p.sea_level_y,
            true,
        );
        let mut h = Humidity::with_world_bounds(
            4,
            0,
            p.bedrock_floor_y,
            p.width_cols,
            p.sky_ceiling_y,
        );
        h.wrap_x = true;
        (t, h)
    }

    #[test]
    fn noon_mean_warmer_than_midnight_after_steps() {
        let (mut day, h) = demo_temp();
        let (mut night, _) = demo_temp();
        // tick 0 = noon, DEMO_DAY_TICKS/2 = midnight.
        for _ in 0..8 {
            day.step(None, &h, 0);
            night.step(None, &h, DEMO_DAY_TICKS / 2);
        }
        assert!(
            day.mean() > night.mean() + 1.0,
            "noon mean {:.1} should beat midnight {:.1}",
            day.mean(),
            night.mean()
        );
    }

    #[test]
    fn clouds_shade_daytime_heating() {
        let (mut clear, h_clear) = demo_temp();
        let (mut cloudy, mut h_cloud) = demo_temp();
        // Saturate one map with cloud mass.
        if let Some(b) = h_cloud.bounds {
            for hy in b.hy_min..=b.hy_max {
                for hx in b.hx_min..=b.hx_max {
                    h_cloud
                        .cells
                        .insert((hx, hy), TempConfig::default().hum_shade_ref * 2.0);
                }
            }
        }
        for _ in 0..6 {
            clear.step(None, &h_clear, 0); // noon
            cloudy.step(None, &h_cloud, 0);
        }
        assert!(
            clear.mean() > cloudy.mean() + 0.3,
            "clear {:.1} should warm more than cloudy {:.1}",
            clear.mean(),
            cloudy.mean()
        );
    }

    #[test]
    fn temperature_step_due_matches_schedule() {
        assert!(temperature_step_due(0));
        assert!(!temperature_step_due(3));
        assert!(temperature_step_due(20));
    }

    /// Fill every column of a 4-wide temperature tile so averages see it.
    fn fill_tile_surface(w: &mut World, tile_x0: i32, y_ground: i32, mat: MaterialId, water_h: i32) {
        for x in tile_x0..tile_x0 + 4 {
            for y in (y_ground - 1)..=(y_ground + water_h + 1) {
                w.ensure_chunk(ChunkCoord::new(
                    x.div_euclid(crate::chunk::CHUNK_CELLS_W as i32),
                    y.div_euclid(crate::chunk::CHUNK_CELLS_H as i32),
                ));
            }
            w.set_cell(x, y_ground, Cell::solid(MaterialId::Stone));
            if water_h > 0 {
                for y in 1..=water_h {
                    w.set_cell(x, y_ground + y, Cell::water());
                }
            } else {
                w.set_cell(x, y_ground + 1, Cell::solid(mat));
            }
        }
    }

    #[test]
    fn water_column_lags_cold_snap_more_than_dry_sand() {
        let p = WorldgenParams::default();
        let sea = p.sea_level_y;
        let pond_x0: i32 = 4; // tile hx=1
        let dry_x0: i32 = 20; // tile hx=5
        let mut world = World::new(7);
        fill_tile_surface(&mut world, pond_x0, sea, MaterialId::Water, 5);
        fill_tile_surface(&mut world, dry_x0, sea, MaterialId::Sand, 0);
        let mut t = Temperature::with_world_bounds(
            4,
            0,
            p.bedrock_floor_y,
            p.width_cols,
            p.sky_ceiling_y,
            p.seed,
            p.width_cols,
            sea,
            false,
        );
        for v in t.cells.values_mut() {
            *v = 12.0;
        }
        t.config.base_temp_c = -15.0;
        t.config.diffuse_alpha = 0.0; // isolate inertia from neighbour mixing
        let h = Humidity::with_world_bounds(
            4,
            0,
            p.bedrock_floor_y,
            p.width_cols,
            p.sky_ceiling_y,
        );
        for i in 0..5 {
            t.step(Some(&world), &h, i * TEMP_STEP_PERIOD);
        }
        let pond_t = t.at_cell(pond_x0 + 1, sea + 3);
        let dry_t = t.at_cell(dry_x0 + 1, sea + 1);
        assert!(
            pond_t > dry_t + 1.5,
            "pond {pond_t:.1}C should stay warmer than dry sand {dry_t:.1}C after a cold snap"
        );
        assert!(
            pond_t > -10.0,
            "deep water must not slam to skin in a few thermal steps (got {pond_t:.1})"
        );
    }

    #[test]
    fn snow_albedo_slows_daytime_warming_vs_bare_rock() {
        let p = WorldgenParams::default();
        let sea = p.sea_level_y;
        let x0: i32 = 8; // tile hx=2
        let mut snow_w = World::new(3);
        let mut rock_w = World::new(3);
        fill_tile_surface(&mut snow_w, x0, sea, MaterialId::Snow, 0);
        fill_tile_surface(&mut rock_w, x0, sea, MaterialId::Stone, 0);

        let mut t_snow = Temperature::with_world_bounds(
            4, 0, p.bedrock_floor_y, 32, p.sky_ceiling_y, 1, 32, sea, false,
        );
        let mut t_rock = Temperature::with_world_bounds(
            4, 0, p.bedrock_floor_y, 32, p.sky_ceiling_y, 1, 32, sea, false,
        );
        for v in t_snow.cells.values_mut().chain(t_rock.cells.values_mut()) {
            *v = 0.0;
        }
        t_snow.config.base_temp_c = 8.0;
        t_rock.config.base_temp_c = 8.0;
        t_snow.config.diffuse_alpha = 0.0;
        t_rock.config.diffuse_alpha = 0.0;
        let h = Humidity::with_world_bounds(4, 0, p.bedrock_floor_y, 32, p.sky_ceiling_y);
        for i in 0..6 {
            t_snow.step(Some(&snow_w), &h, i * TEMP_STEP_PERIOD);
            t_rock.step(Some(&rock_w), &h, i * TEMP_STEP_PERIOD);
        }
        let ts = t_snow.at_cell(x0 + 1, sea + 1);
        let tr = t_rock.at_cell(x0 + 1, sea + 1);
        assert!(
            tr > ts + 0.2,
            "bare rock {tr:.2} should warm faster than snow pack {ts:.2} under sun"
        );
    }
}
