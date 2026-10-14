//! Phase 8 ship-client contract types + in-process / IPC **sim host**.
//!
//! Viewer binaries paint [`ViewFrame`]; host owns [`step_world`].
//! Out-of-process: [`ipc`] over localhost TCP. See
//! [`docs/PHASE8_CONTRACT.md`](../../docs/PHASE8_CONTRACT.md).

mod present;
pub mod ipc;

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use wk_material::MaterialId;
use wk_voxel::{
    apply_season_sky_tint, celestial_local_cfg, climate_with_season, day_night_factor_cfg,
    is_daytime_cfg, moon_phase_cfg, phase_fraction_cfg, season_fraction_cfg,
    cave_humidity_haze_wash, restore_all_from_store, sky_rgb_at_height_phased, sky_rgb_phased,
    stamp_world, steam_haze_wash, step_world, sync_residency, wake_floating_competent,
    Blueprint, CarbonBudget, CarbonConfig, Cell, ClimateConfig, CloudConfig, CloudStore,
    CompetentFallConfig, CondensationConfig, EvapConfig, FailureConfig, FungiConfig, GeotechMap,
    GrainConfig, Humidity, KarstConfig, LandscapeBodyStore, OrganismStore, PerfConfig,
    PhaseConfig, SimSnapshot, SpawnFail, SteamConfig, StreamWindow, SupportMap,
    SyncResidencyStats, Temperature, Wind, World, WorldStep, WorldStepConfig, WorldgenParams,
    CHUNK_CELLS_W, STRATOSPHERE_CELLS, TROPOSPHERE_TOP_Y,
};

/// Bump when [`ViewFrame`] field layout breaks.
pub const VIEW_SCHEMA: u32 = 10;
/// Bump when [`ClientCommand`] enum breaks.
pub const COMMAND_SCHEMA: u32 = 7;
/// Max paint brush radius (cells); diameter = 2·r+1.
pub const MAX_PAINT_RADIUS: i32 = 16;
/// Default disk slot for F5 / F9.
pub const DEFAULT_SAVE_SLOT: &str = "quick";

const HUMIDITY_TILE_COLS: i32 = 4;
const CLIMATE_WIND_VX: f32 = 0.05;
/// Default max sim rate while the viewer presents (~20 Hz).
pub const DEFAULT_STEP_PERIOD_MS: u64 = 50;
/// Camera window width for Compact stream (viewer default; was 320).
/// Wider so the strip fills a typical window; stream cost scales with cols.
pub const COMPACT_VIEW_COLS: i32 = 720;
/// Vertical extract band for Compact (~view × this).
pub const COMPACT_VIEW_ROWS: i32 = 384;
/// Clamp for viewer `[` / `]` view-width changes.
pub const MIN_VIEW_COLS: i32 = 96;
pub const MAX_VIEW_COLS: i32 = 1600;

/// Official Wild / recommended Shadow region width (cols).
/// See [`docs/REGION_CAPACITY.md`](../../docs/REGION_CAPACITY.md).
pub const REGION_WIDTH_COLS: i32 = (CHUNK_CELLS_W as i32) * 64;
/// Soft concurrent Presence peers per region process.
pub const REGION_SOFT_PEERS: u8 = ipc::DEFAULT_MAX_PEERS;
/// Hard peer cap (wire).
pub const REGION_HARD_PEERS: u8 = ipc::MAX_PEERS_CAP;
/// Planning floor: sim-FPS under Presence load before refusing more pressure.
pub const REGION_LOAD_SIM_FPS_FLOOR: u32 = 30;
/// WAN present cadence target once ViewFrame compression lands (Hz).
pub const REGION_WAN_PRESENT_HZ: u32 = 10;

/// Wire encoding for [`TerrainStrip::rgba`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[repr(u8)]
pub enum StripCodec {
    /// Uncompressed RGBA8 (`cols * rows * 4` bytes).
    #[default]
    RawRgba = 0,
    /// zstd of the raw RGBA8 blob (IPC WAN path).
    ZstdRgba = 1,
}

/// Camera-window RGBA strip (row-major, y=0 at top / high world-y).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerrainStrip {
    pub origin_col: i32,
    pub origin_y: i32,
    pub cols: u32,
    pub rows: u32,
    /// Encoding of [`Self::rgba`].
    pub codec: StripCodec,
    /// Payload: raw RGBA8 or zstd(raw), see [`Self::codec`].
    pub rgba: Vec<u8>,
}

impl TerrainStrip {
    pub fn expected_raw_len(&self) -> usize {
        (self.cols as usize).saturating_mul(self.rows as usize).saturating_mul(4)
    }

    pub fn byte_len_ok(&self) -> bool {
        let expected = self.expected_raw_len();
        match self.codec {
            StripCodec::RawRgba => self.rgba.len() == expected,
            StripCodec::ZstdRgba => expected == 0 || !self.rgba.is_empty(),
        }
    }

    /// Compress raw RGBA for the IPC wire (no-op if already zstd).
    pub fn compress_zstd(&mut self) -> Result<(), String> {
        if matches!(self.codec, StripCodec::ZstdRgba) {
            return Ok(());
        }
        if !self.byte_len_ok() {
            return Err("raw strip length mismatch".into());
        }
        if self.rgba.is_empty() {
            self.codec = StripCodec::ZstdRgba;
            return Ok(());
        }
        // Level 1 — present path cares about CPU at 10–20 Hz.
        let compressed = zstd::encode_all(self.rgba.as_slice(), 1).map_err(|e| e.to_string())?;
        self.rgba = compressed;
        self.codec = StripCodec::ZstdRgba;
        Ok(())
    }

    /// Expand to raw RGBA for blitting / in-process use.
    pub fn decompress_to_raw(&mut self) -> Result<(), String> {
        match self.codec {
            StripCodec::RawRgba => {
                if !self.byte_len_ok() {
                    return Err("raw strip length mismatch".into());
                }
                Ok(())
            }
            StripCodec::ZstdRgba => {
                let expected = self.expected_raw_len();
                if expected == 0 {
                    self.rgba.clear();
                    self.codec = StripCodec::RawRgba;
                    return Ok(());
                }
                let raw = zstd::decode_all(self.rgba.as_slice()).map_err(|e| e.to_string())?;
                if raw.len() != expected {
                    return Err(format!(
                        "zstd strip {} bytes, expected {expected}",
                        raw.len()
                    ));
                }
                self.rgba = raw;
                self.codec = StripCodec::RawRgba;
                Ok(())
            }
        }
    }
}

/// Host-authored sky / climate presentation (packed for `Eq`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// Host-owned sim dials (viewer edits via [`ClientCommand::SetSimSettings`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimSettings {
    pub organisms_on: bool,
    pub evap_on: bool,
    pub cond_rain_on: bool,
    pub karst_on: bool,
    pub day_ticks: u64,
    pub night_ticks: u64,
}

impl Default for SimSettings {
    fn default() -> Self {
        Self {
            organisms_on: false,
            evap_on: true,
            cond_rain_on: true,
            karst_on: true,
            day_ticks: 180,
            night_ticks: 180,
        }
    }
}

/// Lightweight host HUD crumbs (not a mass audit).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct HostInfo {
    pub resident_chunks: u32,
    pub parked_chunks: u32,
    pub live_organisms: u32,
    pub parked_organisms: u32,
    pub last_evicted: u32,
    pub last_reloaded: u32,
}

/// Presentation dials stamped by the host (viewer may request via command).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PresentHints {
    /// Soft humidity wash on air / sky-fill cells in the terrain strip.
    pub humidity_overlay: bool,
    /// Soft steam haze (conduit vapour) on strip cells.
    pub steam_overlay: bool,
    /// Soft sealed-cave humidity wash on strip cells.
    pub cave_humidity_overlay: bool,
}

/// Camera-window extract for the viewer (not a durable `SimSnapshot`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ViewFrame {
    pub contract_version: u32,
    pub sim_tick: u64,
    pub paused: bool,
    pub world_width_cols: i32,
    pub world_height: i32,
    pub focus_col: i32,
    /// Mid-column of the vertical extract band (Compact); ignored on tiny rings.
    pub focus_y: i32,
    pub view_cols: i32,
    pub last_step_ms: u32,
    /// True when the last host advance used full [`step_world`].
    pub product_step: bool,
    pub sky: SkyPresent,
    pub sim: SimSettings,
    pub host: HostInfo,
    pub present: PresentHints,
    pub terrain: TerrainStrip,
    pub command_acks: Vec<CommandAck>,
}

impl ViewFrame {
    pub fn schema_ok(&self) -> bool {
        self.contract_version == VIEW_SCHEMA && self.terrain.byte_len_ok()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandAck {
    pub id: u64,
    pub ok: bool,
    pub detail: String,
}

/// Terrain paint brush (mirrors macroquad F3 palette; Operator only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[repr(u8)]
pub enum PaintBrush {
    Erase = 0,
    Water = 1,
    Ice = 2,
    Snow = 3,
    #[default]
    Sand = 4,
    Gravel = 5,
    LooseRock = 6,
    Rock = 7,
    Clay = 8,
    Organic = 9,
    LooseLimestone = 10,
    Soil = 11,
    Limestone = 12,
    Bedrock = 13,
}

impl PaintBrush {
    pub const ALL: [PaintBrush; 14] = [
        PaintBrush::Erase,
        PaintBrush::Water,
        PaintBrush::Ice,
        PaintBrush::Snow,
        PaintBrush::Sand,
        PaintBrush::Gravel,
        PaintBrush::LooseRock,
        PaintBrush::Rock,
        PaintBrush::Clay,
        PaintBrush::Organic,
        PaintBrush::LooseLimestone,
        PaintBrush::Soil,
        PaintBrush::Limestone,
        PaintBrush::Bedrock,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Erase => "Erase",
            Self::Water => "Water",
            Self::Ice => "Ice",
            Self::Snow => "Snow",
            Self::Sand => "Sand",
            Self::Gravel => "Gravel",
            Self::LooseRock => "LooseRock",
            Self::Rock => "Rock",
            Self::Clay => "Clay",
            Self::Organic => "Organic",
            Self::LooseLimestone => "LooseLime",
            Self::Soil => "Soil",
            Self::Limestone => "Limestone",
            Self::Bedrock => "Bedrock",
        }
    }

    pub fn to_cell(self) -> Cell {
        match self {
            Self::Erase => Cell::air(),
            Self::Water => Cell::water(),
            Self::Ice => Cell::solid(MaterialId::Ice),
            Self::Snow => Cell::solid(MaterialId::Snow),
            Self::Sand => Cell::solid(MaterialId::Sand),
            Self::Gravel => Cell::solid(MaterialId::Gravel),
            Self::LooseRock => Cell::solid(MaterialId::LooseRock),
            Self::LooseLimestone => Cell::solid(MaterialId::LooseLimestone),
            Self::Rock => Cell::solid(MaterialId::Stone),
            Self::Clay => Cell::solid(MaterialId::Clay),
            Self::Soil => Cell::solid(MaterialId::Soil),
            Self::Organic => Cell::solid(MaterialId::Organic),
            Self::Limestone => Cell::solid(MaterialId::Limestone),
            Self::Bedrock => Cell::solid(MaterialId::Bedrock),
        }
    }
}

/// Built-in organism presets for operator spawn (macroquad F2 templates).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[repr(u8)]
pub enum SpawnPreset {
    #[default]
    Atom = 0,
    Plant = 1,
    Seaweed = 2,
    Fungus = 3,
    Epiphyte = 4,
}

impl SpawnPreset {
    pub const ALL: [SpawnPreset; 5] = [
        SpawnPreset::Atom,
        SpawnPreset::Plant,
        SpawnPreset::Seaweed,
        SpawnPreset::Fungus,
        SpawnPreset::Epiphyte,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Atom => "Atom",
            Self::Plant => "Plant",
            Self::Seaweed => "Seaweed",
            Self::Fungus => "Fungus",
            Self::Epiphyte => "Epiphyte",
        }
    }

    pub fn blueprint(self) -> Blueprint {
        match self {
            Self::Atom => Blueprint::atom(),
            Self::Plant => Blueprint::minimal_plant(),
            Self::Seaweed => Blueprint::minimal_seaweed(),
            Self::Fungus => Blueprint::minimal_fungus(),
            Self::Epiphyte => Blueprint::minimal_epiphyte(),
        }
    }
}

/// Viewer → host. Host drains between steps; never mid-step.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ClientCommand {
    /// Pan / zoom the camera window (viewer owns UX; host owns residency).
    SetCamera {
        focus_col: i32,
        view_cols: i32,
        /// Mid of Compact vertical band; clamped to world height.
        focus_y: i32,
    },
    SetPaused(bool),
    StepOnce,
    /// Durable [`SimSnapshot`] to `saves/<slot>.gvsesim` (host-only I/O).
    Save { slot: String },
    /// Load snapshot from disk; replaces live world on the host.
    Load { slot: String },
    /// Re-stamp worldgen (same shape); `None` seed bumps current seed.
    ResetWorld { seed: Option<u64> },
    /// Replace host sim dials (climate lengths + pass enables).
    SetSimSettings(SimSettings),
    /// Presentation-only dials (host stamps into ViewFrame / strip).
    SetPresentHints(PresentHints),
    /// Operator terrain paint disk (macroquad F3 parity).
    PaintTerrain {
        col: i32,
        y: i32,
        brush: PaintBrush,
        radius: u8,
    },
    /// Operator spawn built-in blueprint at click (macroquad F2 presets).
    SpawnOrganism {
        col: i32,
        y: i32,
        preset: SpawnPreset,
    },
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
    sim: SimSettings,
    present: PresentHints,
    pub paused: bool,
    pub focus_col: i32,
    /// Mid-row of Compact strip band (world y).
    pub focus_y: i32,
    pub view_cols: i32,
    pub quit_requested: bool,
    /// Min wall time between auto product steps (present may be faster).
    pub step_period: Duration,
    next_ack_id: u64,
    pending_acks: Vec<CommandAck>,
    last_step_ms: u32,
    last_step_wall: Instant,
    product_step: bool,
    last_residency: SyncResidencyStats,
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

/// WORLDGEN Compact ring — [`REGION_WIDTH_COLS`] × tropopause (~1 km).
///
/// Same shape as the soak / perf Compact stretch. Viewer streams a
/// [`COMPACT_VIEW_COLS`] window and parks far chunks via `sync_residency`.
/// Envelope contract: [`docs/REGION_CAPACITY.md`](../../docs/REGION_CAPACITY.md).
pub fn compact_worldgen_params() -> WorldgenParams {
    WorldgenParams {
        width_cols: REGION_WIDTH_COLS,
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

    /// Compact ring with an explicit worldgen seed (host CLI `--seed`).
    pub fn compact_with_seed(seed: u64) -> Self {
        let mut params = compact_worldgen_params();
        params.seed = seed;
        Self::new(params)
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
        let compact = params.width_cols >= REGION_WIDTH_COLS;
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
        let focus_y = params.sea_level_y;
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
            sim: SimSettings::default(),
            present: PresentHints::default(),
            paused: false,
            focus_col,
            focus_y,
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
            last_residency: SyncResidencyStats::default(),
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

    pub fn world_seed(&self) -> u64 {
        self.params.seed
    }

    /// Whether `role` at `tier` may apply `cmd`.
    ///
    /// Operator = full tools. Viewer = camera / present / quit; pause/step only
    /// on [`ipc::WorldTier::Lab`] (Shadow/Wild guests share one clock).
    pub fn command_allowed(
        role: ipc::PeerRole,
        tier: ipc::WorldTier,
        cmd: &ClientCommand,
    ) -> bool {
        use ipc::{PeerRole::*, WorldTier};
        match cmd {
            ClientCommand::SetCamera { .. }
            | ClientCommand::SetPresentHints(_)
            | ClientCommand::Quit => true,
            ClientCommand::SetPaused(_) | ClientCommand::StepOnce => {
                matches!(role, Operator) || matches!(tier, WorldTier::Lab)
            }
            ClientCommand::Save { .. }
            | ClientCommand::Load { .. }
            | ClientCommand::ResetWorld { .. }
            | ClientCommand::SetSimSettings(_)
            | ClientCommand::PaintTerrain { .. }
            | ClientCommand::SpawnOrganism { .. } => matches!(role, Operator),
        }
    }

    /// Apply with a peer role + world-tier gate (IPC multiplayer). Local in-process uses [`Self::apply`].
    pub fn apply_as(
        &mut self,
        role: ipc::PeerRole,
        tier: ipc::WorldTier,
        cmd: ClientCommand,
    ) -> CommandAck {
        if !Self::command_allowed(role, tier, &cmd) {
            let id = self.next_ack_id;
            self.next_ack_id += 1;
            let ack = CommandAck {
                id,
                ok: false,
                detail: format!("role {role:?} tier {tier:?} cannot {cmd:?}"),
            };
            self.pending_acks.push(ack.clone());
            return ack;
        }
        self.apply(cmd)
    }

    pub fn apply(&mut self, cmd: ClientCommand) -> CommandAck {
        let id = self.next_ack_id;
        self.next_ack_id += 1;
        let ack = match cmd {
            ClientCommand::SetCamera {
                focus_col,
                view_cols,
                focus_y,
            } => {
                if view_cols <= 0 {
                    CommandAck {
                        id,
                        ok: false,
                        detail: "view_cols must be > 0".into(),
                    }
                } else {
                    self.focus_col = focus_col;
                    self.view_cols = view_cols
                        .clamp(MIN_VIEW_COLS, MAX_VIEW_COLS)
                        .min(self.params.width_cols);
                    self.focus_y = focus_y.clamp(
                        self.params.bedrock_floor_y,
                        self.params.sky_ceiling_y - 1,
                    );
                    if self.is_compact() {
                        self.arm_stream_window();
                    }
                    CommandAck {
                        id,
                        ok: true,
                        detail: format!(
                            "camera col={} y={} view={}",
                            self.focus_col, self.focus_y, self.view_cols
                        ),
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
            ClientCommand::Save { slot } => match self.save_slot(&slot) {
                Ok(path) => CommandAck {
                    id,
                    ok: true,
                    detail: format!("saved {}", path.display()),
                },
                Err(e) => CommandAck {
                    id,
                    ok: false,
                    detail: format!("save: {e}"),
                },
            },
            ClientCommand::Load { slot } => match self.load_slot(&slot) {
                Ok(()) => CommandAck {
                    id,
                    ok: true,
                    detail: format!("loaded {slot}"),
                },
                Err(e) => CommandAck {
                    id,
                    ok: false,
                    detail: format!("load: {e}"),
                },
            },
            ClientCommand::ResetWorld { seed } => {
                self.reset_world(seed);
                CommandAck {
                    id,
                    ok: true,
                    detail: format!("reset seed={}", self.params.seed),
                }
            }
            ClientCommand::SetSimSettings(settings) => {
                if settings.day_ticks == 0 || settings.night_ticks == 0 {
                    CommandAck {
                        id,
                        ok: false,
                        detail: "day_ticks and night_ticks must be > 0".into(),
                    }
                } else {
                    self.apply_sim_settings(settings);
                    CommandAck {
                        id,
                        ok: true,
                        detail: format!(
                            "sim org={} evap={} rain={} karst={} day={} night={}",
                            self.sim.organisms_on,
                            self.sim.evap_on,
                            self.sim.cond_rain_on,
                            self.sim.karst_on,
                            self.sim.day_ticks,
                            self.sim.night_ticks
                        ),
                    }
                }
            }
            ClientCommand::SetPresentHints(hints) => {
                self.present = hints;
                CommandAck {
                    id,
                    ok: true,
                    detail: format!(
                        "present H={} steam={} caveH={}",
                        self.present.humidity_overlay,
                        self.present.steam_overlay,
                        self.present.cave_humidity_overlay
                    ),
                }
            }
            ClientCommand::PaintTerrain {
                col,
                y,
                brush,
                radius,
            } => {
                let r = (radius as i32).clamp(0, MAX_PAINT_RADIUS);
                let cell = brush.to_cell();
                let mut painted = 0u32;
                for dy in -r..=r {
                    for dx in -r..=r {
                        if dx * dx + dy * dy > r * r {
                            continue;
                        }
                        let gx = self.world.wrap_x(col + dx);
                        let gy = y + dy;
                        if gy < self.params.bedrock_floor_y || gy >= self.params.sky_ceiling_y {
                            continue;
                        }
                        self.world.set_cell(gx, gy, cell);
                        painted += 1;
                    }
                }
                if matches!(brush, PaintBrush::Erase) {
                    wake_floating_competent(&mut self.world);
                }
                self.geotech.rebuild(&self.world);
                self.support.rebuild(&self.world);
                if self.is_compact() {
                    self.arm_stream_window();
                    let _ = sync_residency(&mut self.world);
                }
                CommandAck {
                    id,
                    ok: true,
                    detail: format!("paint {:?} r={r} n={painted}", brush),
                }
            }
            ClientCommand::SpawnOrganism { col, y, preset } => {
                let bp = preset.blueprint();
                // Operator presets are Lab-gate (built-ins always pass).
                if let Err(detail) = bp.validate_for_spawn(wk_voxel::SpawnGate::Lab) {
                    CommandAck {
                        id,
                        ok: false,
                        detail: format!("preset {:?}: {detail}", preset),
                    }
                } else {
                    let body = bp.modules_relative_to_nucleus();
                    match self.organisms.spawn_blueprint_free(
                        &self.world,
                        col,
                        y,
                        body,
                        40.0,
                        bp.genome,
                    ) {
                        Ok(()) => CommandAck {
                            id,
                            ok: true,
                            detail: format!(
                                "spawned {} at ({},{}) living={}",
                                bp.name,
                                col,
                                y,
                                self.organisms.living_count()
                            ),
                        },
                        Err(SpawnFail::PopCap) => CommandAck {
                            id,
                            ok: false,
                            detail: format!(
                                "pop cap {}/{}",
                                self.organisms.living_count(),
                                self.organisms.atom_cap()
                            ),
                        },
                        Err(e) => CommandAck {
                            id,
                            ok: false,
                            detail: format!("spawn {:?}: {e:?}", preset),
                        },
                    }
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

    fn flush_streaming_for_save(&mut self) {
        let _ = restore_all_from_store(&mut self.world);
        let _ = self
            .organisms
            .sync_organism_residency(self.world.wrap_width, None);
        self.world.stream_window = None;
    }

    fn to_snapshot(&mut self) -> SimSnapshot {
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

    fn apply_snapshot(&mut self, snap: SimSnapshot) {
        self.params = snap.params;
        self.world = snap.world;
        self.humidity = snap.humidity;
        self.wind = snap.wind;
        self.temperature = snap.temperature;
        self.clouds = snap.clouds;
        self.organisms = snap.organisms;
        self.carbon = snap.carbon;
        self.landscape = LandscapeBodyStore::new();
        self.geotech.rebuild(&self.world);
        self.support.rebuild(&self.world);
        self.view_cols = self
            .view_cols
            .clamp(MIN_VIEW_COLS, MAX_VIEW_COLS)
            .min(self.params.width_cols.max(1));
        self.focus_col = self.focus_col.rem_euclid(self.params.width_cols.max(1));
        self.focus_y = self.focus_y.clamp(
            self.params.bedrock_floor_y,
            self.params.sky_ceiling_y - 1,
        );
        if self.is_compact() {
            self.arm_stream_window();
            let _ = sync_residency(&mut self.world);
        }
    }

    fn save_slot(&mut self, slot: &str) -> Result<std::path::PathBuf, String> {
        let snap = self.to_snapshot();
        let path = snap.save_to_disk(slot)?;
        // Resume streaming under the camera after the full-resident flush.
        if self.is_compact() {
            self.arm_stream_window();
            let _ = sync_residency(&mut self.world);
        }
        Ok(path)
    }

    fn load_slot(&mut self, slot: &str) -> Result<(), String> {
        let path = SimSnapshot::save_path(slot);
        let snap = SimSnapshot::load_from_disk(&path)?;
        self.apply_snapshot(snap);
        Ok(())
    }

    fn reset_world(&mut self, seed: Option<u64>) {
        let mut params = self.params;
        params.seed = seed.unwrap_or(params.seed.wrapping_add(1));
        let paused = self.paused;
        let view_cols = self.view_cols;
        let step_period = self.step_period;
        let next_ack_id = self.next_ack_id;
        let sim = self.sim.clone();
        let present = self.present.clone();
        let pending = std::mem::take(&mut self.pending_acks);
        *self = Host::new(params);
        self.paused = paused;
        self.view_cols = view_cols
            .clamp(MIN_VIEW_COLS, MAX_VIEW_COLS)
            .min(self.params.width_cols);
        self.focus_col = self.params.width_cols / 2;
        self.focus_y = self.params.sea_level_y;
        self.step_period = step_period;
        self.next_ack_id = next_ack_id;
        self.pending_acks = pending;
        self.apply_sim_settings(sim);
        self.present = present;
    }

    fn apply_sim_settings(&mut self, settings: SimSettings) {
        self.sim = settings;
        self.climate.day_ticks = self.sim.day_ticks.max(1);
        self.climate.night_ticks = self.sim.night_ticks.max(1);
    }

    pub fn sim_settings(&self) -> &SimSettings {
        &self.sim
    }

    pub fn apply_all(&mut self, cmds: impl IntoIterator<Item = ClientCommand>) {
        for c in cmds {
            let _ = self.apply(c);
        }
    }

    /// Pop the most recent ack (IPC peers keep acks out-of-band per session).
    pub fn take_last_ack(&mut self) -> Option<CommandAck> {
        self.pending_acks.pop()
    }

    pub fn push_acks(&mut self, mut acks: Vec<CommandAck>) {
        self.pending_acks.append(&mut acks);
    }

    pub fn is_compact(&self) -> bool {
        self.params.width_cols >= (CHUNK_CELLS_W as i32) * 64
    }

    /// Arm `stream_window` to cover all peer cameras (wrap-aware span).
    pub fn arm_union_stream_window(&mut self, cams: &[(i32, i32)]) {
        if cams.is_empty() {
            self.arm_stream_window();
            return;
        }
        if cams.len() == 1 {
            let (focus, view) = cams[0];
            self.world.stream_window = Some(StreamWindow::new(focus, view.max(1)));
            return;
        }
        let width = self.params.width_cols.max(1);
        // Cover each peer's [focus-view/2, focus+view/2] on the wrap ring.
        let mut intervals: Vec<(i32, i32)> = cams
            .iter()
            .map(|&(focus, view)| {
                let half = view.max(1) / 2;
                let lo = focus.wrapping_sub(half).rem_euclid(width);
                (lo, view.max(1))
            })
            .collect();
        intervals.sort_by_key(|i| i.0);
        // Greedy merge on unwrapped line; if total coverage ≥ width, keep all.
        let mut cover = 0i32;
        let mut start = intervals[0].0;
        let mut end = start + intervals[0].1;
        for &(lo, view) in intervals.iter().skip(1) {
            let hi = lo + view;
            if lo <= end {
                end = end.max(hi);
            } else {
                cover += end - start;
                start = lo;
                end = hi;
            }
        }
        cover += end - start;
        if cover >= width {
            self.world.stream_window = Some(StreamWindow::new(0, width));
        } else {
            let view = (end - start).clamp(1, width);
            let focus = start.wrapping_add(view / 2);
            self.world.stream_window = Some(StreamWindow::new(focus, view));
        }
    }

    fn step_once(&mut self) {
        let t0 = Instant::now();
        if self.world.stream_window.is_none() {
            self.arm_stream_window();
        }
        // Park far chunks under the camera window (Compact RAM).
        if let Ok(stats) = sync_residency(&mut self.world) {
            self.last_residency = stats;
        }
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
                evap_on: self.sim.evap_on,
                cond_rain_on: self.sim.cond_rain_on,
                karst_on: self.sim.karst_on,
                organisms_on: self.sim.organisms_on,
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
        // around focus_y so the strip stays viewer-sized (~view×384).
        let (origin_y, rows) = if self.is_compact() {
            let band = COMPACT_VIEW_ROWS;
            let mid = self.focus_y;
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
        let steam_map: HashMap<(i32, i32), (u8, u8)> = if self.present.steam_overlay {
            steam_haze_wash(&self.world, Some(&self.temperature))
                .into_iter()
                .map(|s| ((s.gx, s.gy), (s.density, s.warmth)))
                .collect()
        } else {
            HashMap::new()
        };
        let cave_map: HashMap<(i32, i32), u8> = if self.present.cave_humidity_overlay {
            cave_humidity_haze_wash(&self.world)
                .into_iter()
                .map(|s| ((s.gx, s.gy), s.density))
                .collect()
        } else {
            HashMap::new()
        };
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
                if self.present.humidity_overlay {
                    // Soft cool wash from vapour mass (diagnostic, not cartoon banks).
                    let h = self.humidity.at_cell(gx, gy);
                    let t = (h / 6.0).clamp(0.0, 1.0);
                    if t > 0.02 {
                        let wash = (t * 70.0) as u8;
                        px[0] = px[0].saturating_sub(wash / 3);
                        px[1] = px[1].saturating_sub(wash / 5);
                        px[2] = px[2].saturating_add(wash).min(255);
                    }
                }
                if let Some(&(d, warm)) = steam_map.get(&(gx, gy)) {
                    let a = d as f32 / 255.0;
                    if a > 0.02 {
                        let lift = (a * 90.0) as u8;
                        let warm_add = ((warm as f32 / 255.0) * a * 40.0) as u8;
                        px[0] = px[0].saturating_add(lift).saturating_add(warm_add).min(255);
                        px[1] = px[1].saturating_add(lift).min(255);
                        px[2] = px[2].saturating_add(lift.saturating_sub(warm_add / 2)).min(255);
                    }
                }
                if let Some(&d) = cave_map.get(&(gx, gy)) {
                    let a = d as f32 / 255.0;
                    if a > 0.02 {
                        let lift = (a * 55.0) as u8;
                        px[0] = px[0].saturating_add(lift).min(255);
                        px[1] = px[1].saturating_add(lift).min(255);
                        px[2] = px[2].saturating_add(lift).min(255);
                    }
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
            codec: StripCodec::RawRgba,
            rgba,
        }
    }

    /// Present a view frame. Auto-steps at [`Self::step_period`] when running.
    pub fn frame(&mut self) -> ViewFrame {
        self.arm_stream_window();
        self.frame_inner()
    }

    /// Present for one IPC peer camera; `peer_cams` drives union residency before step.
    pub fn frame_as(
        &mut self,
        focus_col: i32,
        view_cols: i32,
        focus_y: i32,
        present: PresentHints,
        peer_cams: &[(i32, i32)],
    ) -> ViewFrame {
        let saved = (
            self.focus_col,
            self.view_cols,
            self.focus_y,
            self.present.clone(),
        );
        self.focus_col = focus_col;
        self.view_cols = view_cols
            .clamp(MIN_VIEW_COLS, MAX_VIEW_COLS)
            .min(self.params.width_cols.max(1));
        self.focus_y = focus_y.clamp(
            self.params.bedrock_floor_y,
            self.params.sky_ceiling_y - 1,
        );
        self.present = present;
        if peer_cams.is_empty() {
            self.arm_stream_window();
        } else {
            self.arm_union_stream_window(peer_cams);
        }
        let frame = self.frame_inner();
        self.focus_col = saved.0;
        self.view_cols = saved.1;
        self.focus_y = saved.2;
        self.present = saved.3;
        frame
    }

    fn frame_inner(&mut self) -> ViewFrame {
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
        let host = HostInfo {
            resident_chunks: self.world.chunks.len() as u32,
            parked_chunks: self.world.chunk_store.len() as u32,
            live_organisms: (self.organisms.atoms.len() + self.organisms.corpses.len()) as u32,
            parked_organisms: (self.organisms.parked.len() + self.organisms.parked_corpses.len())
                as u32,
            last_evicted: self.last_residency.evicted as u32,
            last_reloaded: self.last_residency.reloaded as u32,
        };
        ViewFrame {
            contract_version: VIEW_SCHEMA,
            sim_tick: self.world.tick,
            paused: self.paused,
            world_width_cols: self.params.width_cols,
            world_height: self.world_height(),
            focus_col: self.focus_col,
            focus_y: self.focus_y,
            view_cols: self.view_cols,
            last_step_ms: self.last_step_ms,
            product_step: self.product_step,
            sky,
            sim: self.sim.clone(),
            host,
            present: self.present.clone(),
            terrain,
            command_acks: acks,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terrain_strip_zstd_round_trip_and_ratio() {
        let mut strip = TerrainStrip {
            origin_col: 0,
            origin_y: 0,
            cols: 64,
            rows: 32,
            codec: StripCodec::RawRgba,
            rgba: vec![0u8; 64 * 32 * 4],
        };
        // Sky-like: mostly constant with a few bright pixels (compresses well).
        for i in (0..strip.rgba.len()).step_by(4) {
            strip.rgba[i] = 40;
            strip.rgba[i + 1] = 80;
            strip.rgba[i + 2] = 160;
            strip.rgba[i + 3] = 255;
        }
        for x in 0..8 {
            let i = (x * 4) as usize;
            strip.rgba[i] = 255;
            strip.rgba[i + 1] = 220;
            strip.rgba[i + 2] = 40;
        }
        let raw_len = strip.rgba.len();
        strip.compress_zstd().unwrap();
        assert_eq!(strip.codec, StripCodec::ZstdRgba);
        let ratio = raw_len as f32 / strip.rgba.len().max(1) as f32;
        assert!(
            ratio >= 10.0,
            "expected ≥10× on flat sky strip, got {ratio:.1}× ({} → {})",
            raw_len,
            strip.rgba.len()
        );
        strip.decompress_to_raw().unwrap();
        assert_eq!(strip.codec, StripCodec::RawRgba);
        assert_eq!(strip.rgba.len(), raw_len);
        assert_eq!(strip.rgba[0], 255); // first bright pixel
        assert_eq!(strip.rgba[8 * 4], 40); // past the bright run
    }

    #[test]
    fn region_envelope_matches_compact_worldgen() {
        let p = compact_worldgen_params();
        assert_eq!(p.width_cols, REGION_WIDTH_COLS);
        assert_eq!(REGION_WIDTH_COLS, 4096);
        assert_eq!(REGION_SOFT_PEERS, 4);
        assert_eq!(REGION_HARD_PEERS, 8);
        assert_eq!(REGION_LOAD_SIM_FPS_FLOOR, 30);
        assert_eq!(REGION_WAN_PRESENT_HZ, 10);
        assert_eq!(COMPACT_VIEW_COLS, 720);
        assert_eq!(COMPACT_VIEW_ROWS, 384);
        assert_eq!(DEFAULT_STEP_PERIOD_MS, 50);
        assert!(p.sky_ceiling_y > p.bedrock_floor_y);
        assert_eq!(
            p.sky_ceiling_y,
            TROPOSPHERE_TOP_Y + STRATOSPHERE_CELLS
        );
    }

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
            focus_y: host.focus_y,
        });
        assert!(!ack.ok);
        assert_eq!(host.view_cols, before);
    }

    #[test]
    fn camera_sets_focus_y() {
        let mut host = Host::for_tests();
        host.apply(ClientCommand::SetPaused(true));
        let target = (host.params.bedrock_floor_y + host.params.sky_ceiling_y) / 2;
        let ack = host.apply(ClientCommand::SetCamera {
            focus_col: host.focus_col,
            view_cols: host.view_cols,
            focus_y: target,
        });
        assert!(ack.ok, "{}", ack.detail);
        assert_eq!(host.focus_y, target);
        assert_eq!(host.frame().focus_y, target);
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

    #[test]
    fn save_load_roundtrip_preserves_tick() {
        let slot = format!("client_test_{}", std::process::id());
        let mut host = Host::for_tests();
        host.apply(ClientCommand::SetPaused(true));
        for _ in 0..3 {
            host.apply(ClientCommand::StepOnce);
        }
        let tick = host.frame().sim_tick;
        let ack = host.apply(ClientCommand::Save {
            slot: slot.clone(),
        });
        assert!(ack.ok, "{}", ack.detail);
        host.apply(ClientCommand::ResetWorld { seed: Some(999) });
        let mid = host.frame().sim_tick;
        assert_eq!(mid, 0);
        let ack = host.apply(ClientCommand::Load { slot: slot.clone() });
        assert!(ack.ok, "{}", ack.detail);
        let loaded = host.frame().sim_tick;
        assert_eq!(loaded, tick);
        let _ = std::fs::remove_file(SimSnapshot::save_path(&slot));
    }

    #[test]
    fn reset_world_bumps_seed() {
        let mut host = Host::for_tests();
        let s0 = host.world_seed();
        host.apply(ClientCommand::ResetWorld { seed: None });
        assert_eq!(host.world_seed(), s0.wrapping_add(1));
        host.apply(ClientCommand::ResetWorld { seed: Some(42) });
        assert_eq!(host.world_seed(), 42);
    }

    #[test]
    fn set_sim_settings_gates_and_climate() {
        let mut host = Host::for_tests();
        host.apply(ClientCommand::SetPaused(true));
        let mut s = host.sim_settings().clone();
        s.organisms_on = true;
        s.evap_on = false;
        s.day_ticks = 90;
        s.night_ticks = 270;
        let ack = host.apply(ClientCommand::SetSimSettings(s.clone()));
        assert!(ack.ok, "{}", ack.detail);
        assert_eq!(host.sim_settings(), &s);
        let frame = host.frame();
        assert_eq!(frame.contract_version, VIEW_SCHEMA);
        assert_eq!(frame.sim, s);
        let bad = host.apply(ClientCommand::SetSimSettings(SimSettings {
            day_ticks: 0,
            ..s
        }));
        assert!(!bad.ok);
    }

    #[test]
    fn host_info_reports_resident_chunks() {
        let mut host = Host::for_tests();
        host.apply(ClientCommand::SetPaused(true));
        let frame = host.frame();
        assert!(frame.host.resident_chunks > 0);
        assert_eq!(frame.host.parked_chunks, 0);
    }

    #[test]
    fn present_hints_humidity_toggle() {
        let mut host = Host::for_tests();
        host.apply(ClientCommand::SetPaused(true));
        assert!(!host.frame().present.humidity_overlay);
        let ack = host.apply(ClientCommand::SetPresentHints(PresentHints {
            humidity_overlay: true,
            steam_overlay: true,
            cave_humidity_overlay: true,
        }));
        assert!(ack.ok, "{}", ack.detail);
        let p = host.frame().present;
        assert!(p.humidity_overlay && p.steam_overlay && p.cave_humidity_overlay);
    }

    #[test]
    fn operator_role_gates_mutating_cmds() {
        use ipc::{PeerRole, WorldTier};
        let mut host = Host::for_tests();
        host.apply(ClientCommand::SetPaused(true));
        let deny = host.apply_as(
            PeerRole::Viewer,
            WorldTier::Lab,
            ClientCommand::ResetWorld { seed: None },
        );
        assert!(!deny.ok, "{}", deny.detail);
        let allow = host.apply_as(
            PeerRole::Operator,
            WorldTier::Lab,
            ClientCommand::ResetWorld { seed: Some(9) },
        );
        assert!(allow.ok, "{}", allow.detail);
        assert!(Host::command_allowed(
            PeerRole::Viewer,
            WorldTier::Wild,
            &ClientCommand::SetCamera {
                focus_col: 0,
                view_cols: 96,
                focus_y: 40,
            }
        ));
        assert!(!Host::command_allowed(
            PeerRole::Viewer,
            WorldTier::Wild,
            &ClientCommand::SetSimSettings(SimSettings::default())
        ));
        assert!(!Host::command_allowed(
            PeerRole::Viewer,
            WorldTier::Wild,
            &ClientCommand::PaintTerrain {
                col: 0,
                y: 40,
                brush: PaintBrush::Sand,
                radius: 0,
            }
        ));
    }

    #[test]
    fn allowlist_role_tier_matrix_pause_step() {
        use ipc::{PeerRole, WorldTier};
        let pause = ClientCommand::SetPaused(true);
        let step = ClientCommand::StepOnce;
        // Lab: Viewer may clock-control (sandbox).
        assert!(Host::command_allowed(PeerRole::Viewer, WorldTier::Lab, &pause));
        assert!(Host::command_allowed(PeerRole::Viewer, WorldTier::Lab, &step));
        // Shadow guest / Wild player: no shared-clock control.
        for tier in [WorldTier::Shadow, WorldTier::Wild] {
            assert!(!Host::command_allowed(PeerRole::Viewer, tier, &pause));
            assert!(!Host::command_allowed(PeerRole::Viewer, tier, &step));
            assert!(Host::command_allowed(PeerRole::Operator, tier, &pause));
            assert!(Host::command_allowed(PeerRole::Operator, tier, &step));
        }
        let mut host = Host::for_tests();
        let deny = host.apply_as(PeerRole::Viewer, WorldTier::Wild, pause.clone());
        assert!(!deny.ok, "{}", deny.detail);
        assert!(deny.detail.contains("Wild"));
        let allow = host.apply_as(PeerRole::Operator, WorldTier::Wild, pause);
        assert!(allow.ok, "{}", allow.detail);
    }

    #[test]
    fn paint_terrain_writes_disk() {
        let mut host = Host::for_tests();
        host.apply(ClientCommand::SetPaused(true));
        let col = host.focus_col;
        let y = host.focus_y;
        let ack = host.apply(ClientCommand::PaintTerrain {
            col,
            y,
            brush: PaintBrush::Rock,
            radius: 1,
        });
        assert!(ack.ok, "{}", ack.detail);
        let c = host
            .world
            .get_cell(col, y)
            .expect("painted cell");
        assert_eq!(c.material, MaterialId::Stone);
    }

    #[test]
    fn spawn_organism_preset_atom() {
        let mut host = Host::for_tests();
        host.apply(ClientCommand::SetPaused(true));
        // Free spawn needs Air near the click — paint a wet column seat.
        let col = host.focus_col;
        let y = host.params.sea_level_y;
        host.apply(ClientCommand::PaintTerrain {
            col,
            y: y - 1,
            brush: PaintBrush::Sand,
            radius: 0,
        });
        host.apply(ClientCommand::PaintTerrain {
            col,
            y,
            brush: PaintBrush::Water,
            radius: 0,
        });
        let before = host.organisms.living_count();
        let ack = host.apply(ClientCommand::SpawnOrganism {
            col,
            y,
            preset: SpawnPreset::Atom,
        });
        assert!(ack.ok, "{}", ack.detail);
        assert!(host.organisms.living_count() > before);
        let deny = host.apply_as(
            ipc::PeerRole::Viewer,
            ipc::WorldTier::Wild,
            ClientCommand::SpawnOrganism {
                col,
                y,
                preset: SpawnPreset::Atom,
            },
        );
        assert!(!deny.ok);
    }
}
