//! Phase 8 ship-client contract types + in-process host.
//!
//! See [`docs/PHASE8_CONTRACT.md`]. The host owns the tick and terrain
//! extract; the client only queues [`ClientCommand`]s and paints
//! [`ViewFrame`]s. No sat/mass mutation lives in the present path.

mod present;

use wk_voxel::{
    apply_season_sky_tint, celestial_local_cfg, climate_with_season, day_night_factor_cfg,
    is_daytime_cfg, moon_phase_cfg, phase_fraction_cfg, season_fraction_cfg,
    sky_rgb_at_height_phased, sky_rgb_phased, stamp_world, tick, ClimateConfig, StreamWindow,
    World, WorldgenParams, CHUNK_CELLS_W,
};

/// Bump when [`ViewFrame`] field layout breaks.
pub const VIEW_SCHEMA: u32 = 3;
/// Bump when [`ClientCommand`] enum breaks.
pub const COMMAND_SCHEMA: u32 = 1;

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
    /// Horizon-ish sky colour from [`sky_rgb_phased`].
    pub sky_rgb: [u8; 3],
    /// `day_night_factor` mapped −1..+1 → 0..255 (128 ≈ dusk/dawn).
    pub day_night_u8: u8,
    /// Climate phase 0..1 → 0..255 (0 ≈ noon).
    pub phase_u8: u8,
    /// Sun/moon arc local 0..1 → 0..255.
    pub celestial_local_u8: u8,
    /// Moon phase 0..1 → 0..255.
    pub moon_phase_u8: u8,
    /// Season fraction 0..1 → 0..255.
    pub season_u8: u8,
    pub is_day: bool,
}

impl SkyPresent {
    pub fn day_night(&self) -> f32 {
        (self.day_night_u8 as f32 / 255.0) * 2.0 - 1.0
    }
}

/// Camera-window extract for presentation (not a durable `SimSnapshot`).
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

/// Client → host. Host drains between steps; never mid-step.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientCommand {
    SetCamera { focus_col: i32, view_cols: i32 },
    SetPaused(bool),
    StepOnce,
    Quit,
}

/// In-process host: stamped `wk-voxel` world + CA tick + terrain extract.
pub struct Host {
    world: World,
    params: WorldgenParams,
    climate: ClimateConfig,
    pub paused: bool,
    pub focus_col: i32,
    pub view_cols: i32,
    pub quit_requested: bool,
    next_ack_id: u64,
    pending_acks: Vec<CommandAck>,
    last_step_ms: u32,
}

/// Small ring world for the client host (fast stamp; not Compact soak size).
pub fn client_worldgen_params() -> WorldgenParams {
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

impl Default for Host {
    fn default() -> Self {
        Self::new(client_worldgen_params())
    }
}

impl Host {
    pub fn new(params: WorldgenParams) -> Self {
        let mut world = World::new(params.seed);
        stamp_world(&mut world, &params);
        let focus_col = params.width_cols / 2;
        // Short cycle so dusk/dawn is visible while driving the spike.
        let climate = climate_with_season(
            &ClimateConfig {
                day_ticks: 180,
                night_ticks: 180,
            },
            0.35,
        );
        Self {
            world,
            params,
            climate,
            paused: false,
            focus_col,
            view_cols: 96,
            quit_requested: false,
            next_ack_id: 1,
            pending_acks: Vec::new(),
            last_step_ms: 0,
        }
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

    fn step_once(&mut self) {
        let t0 = std::time::Instant::now();
        let half = self.view_cols / 2;
        self.world.stream_window = Some(StreamWindow::new(
            self.focus_col.wrapping_sub(half),
            self.view_cols.max(1),
        ));
        tick(&mut self.world);
        self.last_step_ms = t0.elapsed().as_millis().min(u32::MAX as u128) as u32;
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
        let rows = self.world_height() as u32;
        let half = self.view_cols / 2;
        let origin_col = self.focus_col.wrapping_sub(half);
        let origin_y = self.params.bedrock_floor_y;
        let dn = sky.day_night();
        let phase = sky.phase_u8 as f32 / 255.0;
        let mut rgba = vec![0u8; (cols as usize) * (rows as usize) * 4];
        for row in 0..rows {
            // y=0 at top of texture = high world-y (sky / zenith).
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

    /// One product frame: step if running; emit view with terrain strip.
    pub fn frame(&mut self) -> ViewFrame {
        if !self.paused {
            self.step_once();
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
        let mut host = Host::default();
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
        let mut host = Host::default();
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
        let mut host = Host::default();
        host.apply(ClientCommand::SetPaused(true));
        let frame = host.frame();
        assert!(frame.schema_ok());
        assert_eq!(frame.contract_version, VIEW_SCHEMA);
        assert_eq!(frame.terrain.cols, host.view_cols as u32);
        assert_eq!(frame.terrain.rows, host.world_height() as u32);
        assert!(frame.terrain.rgba.iter().any(|&b| b != 0));
        // Air cells are sky-filled (opaque), not left clear.
        assert!(frame.terrain.rgba.chunks(4).any(|c| c[3] == 255));
    }

    #[test]
    fn sky_tracks_tick() {
        let mut host = Host::default();
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
}
