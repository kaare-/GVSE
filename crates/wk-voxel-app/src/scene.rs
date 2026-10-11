//! wk-voxel-app scene state.
//!
//! Isolation: only wk-voxel + wk-material dependencies. No column
//! stack imports.

use wk_voxel::{
    restore_all_from_store, stamp_world, CarbonBudget, CloudStore, GeotechMap, Humidity,
    LandscapeBodyStore, OrganismStore, SimSnapshot, SupportMap, Temperature, Wind, World,
    WorldgenParams,
};

/// Humidity / wind / temp tile side (world cells per sample).
const HUMIDITY_TILE_COLS: i32 = 4;
/// Prevailing wind — tiles per tick. Parcels use a fraction of this
/// so the sky crawls left→right instead of streaking.
const CLIMATE_WIND_VX: f32 = 0.05;

pub struct Scene {
    pub world: World,
    pub params: WorldgenParams,
    pub humidity: Humidity,
    pub wind: Wind,
    pub clouds: CloudStore,
    pub temperature: Temperature,
    pub organisms: OrganismStore,
    /// Crude atmosphere + dissolved CO₂ buckets (saved in sim snapshot).
    pub carbon: CarbonBudget,
    /// Slow derived shear/wet/hydro face map (not saved — rebuilds).
    pub geotech: GeotechMap,
    /// Canonical surface + grounded maps (not saved — rebuilds).
    pub support: SupportMap,
    /// Detached landscape rigid bodies falling as whole pieces.
    pub landscape: LandscapeBodyStore,
}

impl Scene {
    pub fn new(params: WorldgenParams) -> Self {
        let mut world = World::new(params.seed);
        stamp_world(&mut world, &params);
        // Clamp atmospheric mass to the stamped cell rectangle so
        // diffusion can't grow an unbounded sparse haze off-map.
        // Ring worlds also wrap humidity in x so the atmosphere joins.
        let mut humidity = Humidity::with_world_bounds(
            HUMIDITY_TILE_COLS,
            0,
            params.bedrock_floor_y,
            params.width_cols,
            params.sky_ceiling_y,
        );
        humidity.wrap_x = params.wrap_x;
        let wind = Wind::climate(
            HUMIDITY_TILE_COLS,
            CLIMATE_WIND_VX,
            params.seed,
            params.width_cols,
            params.sea_level_y,
            params.bedrock_floor_y,
            params.sky_ceiling_y,
            params.wrap_x,
        );
        let temperature = Temperature::with_world_bounds(
            HUMIDITY_TILE_COLS,
            0,
            params.bedrock_floor_y,
            params.width_cols,
            params.sky_ceiling_y,
            params.seed,
            params.width_cols,
            params.sea_level_y,
            params.wrap_x,
        );
        let clouds = CloudStore::new();
        // Empty organism store — place Atoms via the F2 creature editor
        // (Enter, then click a wet cell). No auto-seeded demo life.
        let organisms = OrganismStore::new();
        let carbon = CarbonBudget::default();
        let mut geotech = GeotechMap::new();
        geotech.rebuild(&world);
        let mut support = SupportMap::new();
        support.rebuild(&world);
        Self {
            world,
            params,
            humidity,
            wind,
            clouds,
            temperature,
            organisms,
            carbon,
            geotech,
            support,
            landscape: LandscapeBodyStore::new(),
        }
    }

    /// Pull serde-skip streaming parks into serializable fields before F5.
    ///
    /// ChunkStore / SideMapStore / FrozenBacklog / organism `parked*` are
    /// not postcard'd. Restoring into `World` maps + live atoms/corpses
    /// makes the snapshot complete; the next stream tick re-parks.
    pub fn flush_streaming_for_save(&mut self) {
        let _ = restore_all_from_store(&mut self.world);
        let _ = self
            .organisms
            .sync_organism_residency(self.world.wrap_width, None);
        self.world.stream_window = None;
    }

    pub fn to_snapshot(&mut self) -> SimSnapshot {
        self.flush_streaming_for_save();
        SimSnapshot::new(
            self.params,
            self.world.clone(),
            self.humidity.clone(),
            self.wind.clone(),
            self.temperature.clone(),
            self.clouds.clone(),
            self.organisms.clone(),
            self.carbon,
        )
    }

    pub fn from_snapshot(snap: SimSnapshot) -> Self {
        let mut geotech = GeotechMap::new();
        geotech.rebuild(&snap.world);
        let mut support = SupportMap::new();
        support.rebuild(&snap.world);
        Self {
            world: snap.world,
            params: snap.params,
            humidity: snap.humidity,
            wind: snap.wind,
            clouds: snap.clouds,
            temperature: snap.temperature,
            organisms: snap.organisms,
            carbon: snap.carbon,
            geotech,
            support,
            landscape: LandscapeBodyStore::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_new_stamps_world_deterministically() {
        let params = WorldgenParams::default();
        let a = Scene::new(params);
        let b = Scene::new(params);
        let ca = a.world.get_cell(100, 20);
        let cb = b.world.get_cell(100, 20);
        assert_eq!(ca, cb);
    }

    #[test]
    fn scene_starts_with_empty_humidity() {
        let s = Scene::new(WorldgenParams::default());
        assert_eq!(s.humidity.total_mass(), 0.0);
    }

    #[test]
    fn scene_starts_with_no_organisms() {
        let s = Scene::new(WorldgenParams::default());
        assert!(
            s.organisms.is_empty(),
            "demo scene should not auto-seed Atoms — spawn via F2 editor"
        );
    }

    #[test]
    fn scene_humidity_is_clamped_to_stamped_world() {
        let s = Scene::new(WorldgenParams::default());
        let b = s.humidity.bounds.expect("demo scene sets bounds");
        assert_eq!(b.hx_min, 0);
        assert!(b.hx_max > 0);
        assert!(b.hy_max > b.hy_min);
        let cells_w = s.params.width_cols;
        let cells_h = s.params.sky_ceiling_y - s.params.bedrock_floor_y;
        let tiles_w = (cells_w + HUMIDITY_TILE_COLS - 1) / HUMIDITY_TILE_COLS;
        let tiles_h = (cells_h + HUMIDITY_TILE_COLS - 1) / HUMIDITY_TILE_COLS;
        assert_eq!(b.tile_capacity(), (tiles_w * tiles_h) as usize);
    }

    #[test]
    fn scene_has_prevailing_wind_and_temperature() {
        let s = Scene::new(WorldgenParams::default());
        assert!(s.wind.climate_vx > 0.0);
        assert!(!s.temperature.cells.is_empty());
    }

    #[test]
    fn to_snapshot_flushes_parked_organisms() {
        use wk_voxel::{Atom, StreamWindow, CHUNK_CELLS_W};
        // Default Compact (16 chunks) fits inside the resident band — use a
        // wider ring so far columns can actually park.
        let mut params = WorldgenParams::default();
        params.width_cols = 64 * CHUNK_CELLS_W as i32;
        let mut s = Scene::new(params);
        let far_gx = 32 * CHUNK_CELLS_W as i32 + 8;
        s.organisms.atoms.push(Atom::new(far_gx, 5, 40.0));
        s.world.stream_window = Some(StreamWindow::new(0, 128));
        s.organisms
            .sync_organism_residency(s.world.wrap_width, s.world.stream_window);
        assert_eq!(s.organisms.parked.len(), 1);
        assert!(s.organisms.atoms.is_empty());
        let snap = s.to_snapshot();
        assert!(
            s.organisms.parked.is_empty() && snap.organisms.parked.is_empty(),
            "flush must clear park before serialize"
        );
        assert_eq!(snap.organisms.atoms.len(), 1);
        assert_eq!(snap.organisms.atoms[0].gx, far_gx);
    }
}
