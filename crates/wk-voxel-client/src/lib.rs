//! Phase 8 ship-client contract types + in-process **sim host**.
//!
//! The binary is a **viewer**: camera, HUD, limited [`ClientCommand`]s.
//! Host owns [`step_world`] + terrain extract. See
//! [`docs/PHASE8_CONTRACT.md`](../../docs/PHASE8_CONTRACT.md).

mod present;

use std::time::{Duration, Instant};

use wk_voxel::{
    apply_season_sky_tint, celestial_local_cfg, climate_with_season, day_night_factor_cfg,
    is_daytime_cfg, moon_phase_cfg, phase_fraction_cfg, season_fraction_cfg,
    sky_rgb_at_height_phased, sky_rgb_phased, stamp_world, step_world, sync_residency,
    CarbonBudget, CarbonConfig, ClimateConfig, CloudConfig, CloudStore, CompetentFallConfig,
    CondensationConfig, EvapConfig, FailureConfig, FungiConfig, GeotechMap, GrainConfig, Humidity,
    KarstConfig, LandscapeBodyStore, OrganismStore, PerfConfig, PhaseConfig, SteamConfig,
    StreamWindow, SupportMap, Temperature, Wind, World, WorldStep, WorldStepConfig, WorldgenParams,
    CHUNK_CELLS_W, STRATOSPHERE_CELLS, TROPOSPHERE_TOP_Y,
};

/// Bump when [`ViewFrame`] field layout breaks.
pub const VIEW_SCHEMA: u32 = 4;
/// Bump when [`ClientCommand`] enum breaks.
pub const COMMAND_SCHEMA: u32 = 1;

const HUMIDITY_TILE_COLS: i32 = 4;
const CLIMATE_WIND_VX: f32 = 0.05;
/// Default max sim rate while the viewer presents (~20 Hz).
pub const DEFAULT_STEP_PERIOD_MS: u64 = 50;
/// Camera window width for Compact stream (matches Phase 6 stream probes).
pub const COMPACT_VIEW_COLS: i32 = 320;

/// Camera-window RGBA strip (row-major, y=0 at top / high world-y).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TerrainStrip {
    pub origin_col: i32,
    pub origin_y: i32,
    pub cols: u32,
    pub rows: u32,
    /// `cols * rows * 4` bytes, RGBA8.
    pub rgba: Vec<u8>,
}

impl TerrainStrip {
    pub fn byte_len_ok(&self) -> bool {
        self.rgba.len() == (self.cols as usize) * (self.rows as usize) * 4
    }
}

/// Host-authored sky / climate presentation (packed for `Eq`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkyPresent {
    pub sky_rgb: [u8; 3],
    pub day_night_u8: u8,
    pub phase_u8: u8,
    pub celestial_local_u8: u8,
    pub moon_phase_u8: u8,
    pub season_u8: u8,
    pub is_day: bool,
}

impl SkyPresent {
    pub fn day_night(&self) -> f32 {
        (self.day_night_u8 as f32 / 255.0) * 2.0 - 1.0
    }
}

/// Camera-window extract for the viewer (not a durable `SimSnapshot`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewFrame {
    pub contract_version: u32,
    pub sim_tick: u64,
    pub paused: bool,
    pub world_width_cols: i32,
    pub world_height: i32,
    pub focus_col: i32,
    pub view_cols: i32,
    pub last_step_ms: u32,
    /// True when the last host advance used full [`step_world`].
    pub product_step: bool,
    pub sky: SkyPresent,
    pub terrain: TerrainStrip,
    pub command_acks: Vec<CommandAck>,
}

impl ViewFrame {
    pub fn schema_ok(&self) -> bool {
        self.contract_version == VIEW_SCHEMA && self.terrain.byte_len_ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandAck {
    pub id: u64,
    pub ok: bool,
    pub detail: String,
}

/// Viewer → host. Host drains between steps; never mid-step.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientCommand {
    /// Pan / zoom the camera window (viewer owns UX; host owns residency).
    SetCamera { focus_col: i32, view_cols: i32 },
    SetPaused(bool),
    StepOnce,
    Quit,
}

/// In-process sim host for the viewer binary.
pub struct Host {
    world: World,
    params: WorldgenParams,
    humidity: Humidity,
    wind: Wind,
    temperature: Temperature,
    clouds: CloudStore,
    carbon: CarbonBudget,
    organisms: OrganismStore,
    landscape: LandscapeBodyStore,
    geotech: GeotechMap,
    support: SupportMap,
    climate: ClimateConfig,
    // Product-step knobs (defaults; viewer has no settings panel yet).
    perf: PerfConfig,
    failure: FailureConfig,
    evap: EvapConfig,
    cond: CondensationConfig,
    karst: KarstConfig,
    cloud: CloudConfig,
    phase: PhaseConfig,
    steam: SteamConfig,
    carbon_cfg: CarbonConfig,
    grain: GrainConfig,
    fungi: FungiConfig,
    competent: CompetentFallConfig,
    pub paused: bool,
    pub focus_col: i32,
    pub view_cols: i32,
    pub quit_requested: bool,
    /// Min wall time between auto product steps (present may be faster).
    pub step_period: Duration,
    next_ack_id: u64,
    pending_acks: Vec<CommandAck>,
    last_step_ms: u32,
    last_step_wall: Instant,
    product_step: bool,
}

/// Tiny ring for unit tests / fast CI (not Compact).
pub fn client_dev_worldgen_params() -> WorldgenParams {
    WorldgenParams {
        seed: 0xC1A_5E08,
        width_cols: (CHUNK_CELLS_W as i32) * 4,
        bedrock_floor_y: 0,
        sea_level_y: 48,
        sky_ceiling_y: 96,
        bedrock_thickness: 4,
        stone_thickness: 8,
        sand_cap_thickness: 2,
        limestone_in_shelf_and_coast: true,
        wrap_x: true,
    }
}

/// WORLDGEN Compact ring — 64 chunks / 4096 cols × tropopause (~1 km).
///
/// Same shape as the soak / perf Compact stretch. Viewer streams a
/// [`COMPACT_VIEW_COLS`] window and parks far chunks via `sync_residency`.
pub fn compact_worldgen_params() -> WorldgenParams {
    WorldgenParams {
        width_cols: (CHUNK_CELLS_W as i32) * 64,
        sky_ceiling_y: TROPOSPHERE_TOP_Y + STRATOSPHERE_CELLS,
        ..WorldgenParams::default()
    }
}

impl Default for Host {
    fn default() -> Self {
        Self::new(compact_worldgen_params())
    }
}

impl Host {
    /// Fast host for unit tests (small stamp; no Compact RAM).
    pub fn for_tests() -> Self {
        let mut host = Self::new(client_dev_worldgen_params());
        host.step_period = Duration::ZERO;
        host
    }

    pub fn new(params: WorldgenParams) -> Self {
        let mut world = World::new(params.seed);
        stamp_world(&mut world, &params);
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
        let mut geotech = GeotechMap::new();
        geotech.rebuild(&world);
        let mut support = SupportMap::new();
        support.rebuild(&world);
        let focus_col = params.width_cols / 2;
        let compact = params.width_cols >= (CHUNK_CELLS_W as i32) * 64;
        let climate = climate_with_season(
            &ClimateConfig {
                day_ticks: 180,
                night_ticks: 180,
            },
            0.35,
        );
        let view_cols = if compact {
            COMPACT_VIEW_COLS.min(params.width_cols)
        } else {
            96.min(params.width_cols)
        };
        let mut host = Self {
            world,
            params,
            humidity,
            wind,
            temperature,
            clouds: CloudStore::new(),
            carbon: CarbonBudget::default(),
            organisms: OrganismStore::new(),
            landscape: LandscapeBodyStore::new(),
            geotech,
            support,
            climate,
            perf: PerfConfig::default(),
            failure: FailureConfig::default(),
            evap: EvapConfig::default(),
            cond: CondensationConfig::default(),
            karst: KarstConfig::default(),
            cloud: CloudConfig::default(),
            phase: PhaseConfig::default(),
            steam: SteamConfig::default(),
            carbon_cfg: CarbonConfig::default(),
            grain: GrainConfig::default(),
            fungi: FungiConfig::default(),
            competent: CompetentFallConfig::default(),
            paused: false,
            focus_col,
            view_cols,
            quit_requested: false,
            step_period: Duration::from_millis(DEFAULT_STEP_PERIOD_MS),
            next_ack_id: 1,
            pending_acks: Vec::new(),
            last_step_ms: 0,
            last_step_wall: Instant::now()
                .checked_sub(Duration::from_secs(1))
                .unwrap_or_else(Instant::now),
            product_step: true,
        };
        // Compact stamp is full-width; park far chunks before the first frame.
        if compact {
            host.arm_stream_window();
            let _ = sync_residency(&mut host.world);
        }
        host
    }

    fn arm_stream_window(&mut self) {
        let half = self.view_cols / 2;
        self.world.stream_window = Some(StreamWindow::new(
            self.focus_col.wrapping_sub(half),
            self.view_cols.max(1),
        ));
    }

    pub fn world_height(&self) -> i32 {
        (self.params.sky_ceiling_y - self.params.bedrock_floor_y).max(1)
    }

    pub fn apply(&mut self, cmd: ClientCommand) -> CommandAck {
        let id = self.next_ack_id;
        self.next_ack_id += 1;
        let ack = match cmd {
            ClientCommand::SetCamera {
                focus_col,
                view_cols,
            } => {
                if view_cols <= 0 {
                    CommandAck {
                        id,
                        ok: false,
                        detail: "view_cols must be > 0".into(),
                    }
                } else {
                    self.focus_col = focus_col;
                    self.view_cols = view_cols.min(self.params.width_cols);
                    CommandAck {
                        id,
                        ok: true,
                        detail: "camera".into(),
                    }
                }
            }
            ClientCommand::SetPaused(p) => {
                self.paused = p;
                CommandAck {
                    id,
                    ok: true,
                    detail: if p { "paused" } else { "resumed" }.into(),
                }
            }
            ClientCommand::StepOnce => {
                self.step_once();
                CommandAck {
                    id,
                    ok: true,
                    detail: "step_once".into(),
                }
            }
            ClientCommand::Quit => {
                self.quit_requested = true;
                CommandAck {
                    id,
                    ok: true,
                    detail: "quit".into(),
                }
            }
        };
        self.pending_acks.push(ack.clone());
        ack
    }

    pub fn apply_all(&mut self, cmds: impl IntoIterator<Item = ClientCommand>) {
        for c in cmds {
            let _ = self.apply(c);
        }
    }

    pub fn is_compact(&self) -> bool {
        self.params.width_cols >= (CHUNK_CELLS_W as i32) * 64
    }

    fn step_once(&mut self) {
        let t0 = Instant::now();
        self.arm_stream_window();
        // Park far chunks under the camera window (Compact RAM).
        let _ = sync_residency(&mut self.world);
        let _ = step_world(
            WorldStep {
                world: &mut self.world,
                humidity: &mut self.humidity,
                wind: &mut self.wind,
                temperature: &mut self.temperature,
                clouds: &mut self.clouds,
                carbon: &mut self.carbon,
                organisms: Some(&mut self.organisms),
                landscape: Some(&mut self.landscape),
                geotech: Some(&mut self.geotech),
                support: Some(&mut self.support),
            },
            &WorldStepConfig {
                perf: &self.perf,
                failure: &self.failure,
                evap: &self.evap,
                cond: &self.cond,
                oro: None,
                karst: &self.karst,
                cloud: &self.cloud,
                phase: &self.phase,
                steam: &self.steam,
                climate: &self.climate,
                carbon: &self.carbon_cfg,
                grain: &self.grain,
                fungi: &self.fungi,
                competent: &self.competent,
                humidity_diffusion_alpha: 0.15,
                sea_level_y: self.params.sea_level_y,
                sky_ceiling_y: self.params.sky_ceiling_y,
                evap_on: true,
                cond_rain_on: true,
                karst_on: true,
                organisms_on: false,
            },
            None,
        );
        self.last_step_ms = t0.elapsed().as_millis().min(u32::MAX as u128) as u32;
        self.last_step_wall = Instant::now();
        self.product_step = true;
    }

    fn sky_present(&self) -> SkyPresent {
        let tick = self.world.tick;
        let dn = day_night_factor_cfg(tick, &self.climate);
        let phase = phase_fraction_cfg(tick, &self.climate);
        let season = season_fraction_cfg(tick, &self.climate);
        let sky = apply_season_sky_tint(sky_rgb_phased(dn, phase), season, dn);
        SkyPresent {
            sky_rgb: sky,
            day_night_u8: ((dn.clamp(-1.0, 1.0) + 1.0) * 0.5 * 255.0).round() as u8,
            phase_u8: (phase.clamp(0.0, 1.0) * 255.0).round() as u8,
            celestial_local_u8: (celestial_local_cfg(tick, &self.climate).clamp(0.0, 1.0) * 255.0)
                .round() as u8,
            moon_phase_u8: (moon_phase_cfg(tick, &self.climate).clamp(0.0, 1.0) * 255.0).round()
                as u8,
            season_u8: (season.clamp(0.0, 1.0) * 255.0).round() as u8,
            is_day: is_daytime_cfg(tick, &self.climate),
        }
    }

    fn extract_terrain(&self, sky: &SkyPresent) -> TerrainStrip {
        let cols = self.view_cols.max(1) as u32;
        // Full column height for small worlds; Compact extracts a band
        // around sea level so the strip stays viewer-sized (~view×384).
        let (origin_y, rows) = if self.is_compact() {
            let band = 384i32;
            let mid = self.params.sea_level_y;
            let lo = (mid - band / 2).max(self.params.bedrock_floor_y);
            let hi = (lo + band).min(self.params.sky_ceiling_y);
            (lo, (hi - lo).max(1) as u32)
        } else {
            (
                self.params.bedrock_floor_y,
                self.world_height() as u32,
            )
        };
        let half = self.view_cols / 2;
        let origin_col = self.focus_col.wrapping_sub(half);
        let dn = sky.day_night();
        let phase = sky.phase_u8 as f32 / 255.0;
        let mut rgba = vec![0u8; (cols as usize) * (rows as usize) * 4];
        for row in 0..rows {
            let gy = origin_y + (rows as i32 - 1 - row as i32);
            let height_01 = if rows <= 1 {
                0.0
            } else {
                row as f32 / (rows - 1) as f32
            };
            let season = sky.season_u8 as f32 / 255.0;
            let sky_row =
                apply_season_sky_tint(sky_rgb_at_height_phased(dn, height_01, phase), season, dn);
            let celestial = sky.celestial_local_u8 as f32 / 255.0;
            for col in 0..cols {
                let gx = self.world.wrap_x(origin_col + col as i32);
                let mut px = present::cell_rgba_lit(
                    &self.world,
                    gx,
                    gy,
                    self.world.tick,
                    celestial,
                    sky.is_day,
                    present::WATER_GLITTER_STRENGTH,
                );
                if px[3] == 0 {
                    px = [sky_row[0], sky_row[1], sky_row[2], 255];
                }
                let i = ((row as usize) * cols as usize + col as usize) * 4;
                rgba[i..i + 4].copy_from_slice(&px);
            }
        }
        TerrainStrip {
            origin_col,
            origin_y,
            cols,
            rows,
            rgba,
        }
    }

    /// Present a view frame. Auto-steps at [`Self::step_period`] when running.
    pub fn frame(&mut self) -> ViewFrame {
        if !self.paused {
            let due = self.step_period.is_zero()
                || self.last_step_wall.elapsed() >= self.step_period;
            if due {
                self.step_once();
            }
        }
        let acks = std::mem::take(&mut self.pending_acks);
        let sky = self.sky_present();
        let terrain = self.extract_terrain(&sky);
        ViewFrame {
            contract_version: VIEW_SCHEMA,
            sim_tick: self.world.tick,
            paused: self.paused,
            world_width_cols: self.params.width_cols,
            world_height: self.world_height(),
            focus_col: self.focus_col,
            view_cols: self.view_cols,
            last_step_ms: self.last_step_ms,
            product_step: self.product_step,
            sky,
            terrain,
            command_acks: acks,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pause_stops_auto_tick() {
        let mut host = Host::for_tests();
        let t0 = host.frame().sim_tick;
        let t1 = host.frame().sim_tick;
        assert!(t1 > t0);
        host.apply(ClientCommand::SetPaused(true));
        let t2 = host.frame().sim_tick;
        assert_eq!(t2, t1);
        host.apply(ClientCommand::StepOnce);
        let t3 = host.frame().sim_tick;
        assert!(t3 > t1);
    }

    #[test]
    fn camera_rejects_non_positive_view() {
        let mut host = Host::for_tests();
        let before = host.view_cols;
        let ack = host.apply(ClientCommand::SetCamera {
            focus_col: 10,
            view_cols: 0,
        });
        assert!(!ack.ok);
        assert_eq!(host.view_cols, before);
    }

    #[test]
    fn view_schema_and_strip() {
        let mut host = Host::for_tests();
        host.apply(ClientCommand::SetPaused(true));
        let frame = host.frame();
        assert!(frame.schema_ok());
        assert_eq!(frame.contract_version, VIEW_SCHEMA);
        assert!(frame.product_step);
        assert!(!host.is_compact());
        assert_eq!(frame.terrain.cols, host.view_cols as u32);
        assert_eq!(frame.terrain.rows, host.world_height() as u32);
        assert!(frame.terrain.rgba.iter().any(|&b| b != 0));
        assert!(frame.terrain.rgba.chunks(4).any(|c| c[3] == 255));
    }

    #[test]
    fn sky_tracks_tick() {
        let mut host = Host::for_tests();
        host.apply(ClientCommand::SetPaused(true));
        let a = host.frame().sky.clone();
        for _ in 0..90 {
            host.apply(ClientCommand::StepOnce);
        }
        let b = host.frame().sky;
        assert!(
            a.day_night_u8 != b.day_night_u8 || a.phase_u8 != b.phase_u8,
            "short climate cycle should move sky within 90 steps"
        );
    }

    #[test]
    fn product_step_advances_world_tick() {
        let mut host = Host::for_tests();
        host.apply(ClientCommand::SetPaused(true));
        let t0 = host.frame().sim_tick;
        host.apply(ClientCommand::StepOnce);
        let frame = host.frame();
        assert!(frame.product_step);
        assert!(frame.sim_tick > t0);
    }

    #[test]
    fn compact_params_shape() {
        let p = compact_worldgen_params();
        assert_eq!(p.width_cols, (CHUNK_CELLS_W as i32) * 64);
        assert!(p.sky_ceiling_y >= TROPOSPHERE_TOP_Y);
        assert!(p.wrap_x);
    }
}
