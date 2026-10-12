//! Phase 8 ship-client contract types + in-process host.
//!
//! See [`docs/PHASE8_CONTRACT.md`]. The host owns the tick and terrain
//! extract; the client only queues [`ClientCommand`]s and paints
//! [`ViewFrame`]s. No sat/mass mutation lives in the present path.

mod present;

use wk_voxel::{stamp_world, tick, StreamWindow, World, WorldgenParams, CHUNK_CELLS_W};

/// Bump when [`ViewFrame`] field layout breaks.
pub const VIEW_SCHEMA: u32 = 2;
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
        Self {
            world,
            params,
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

    fn extract_terrain(&self) -> TerrainStrip {
        let cols = self.view_cols.max(1) as u32;
        let rows = self.world_height() as u32;
        let half = self.view_cols / 2;
        let origin_col = self.focus_col.wrapping_sub(half);
        let origin_y = self.params.bedrock_floor_y;
        let mut rgba = vec![0u8; (cols as usize) * (rows as usize) * 4];
        for row in 0..rows {
            // y=0 at top of texture = high world-y (sky).
            let gy = origin_y + (rows as i32 - 1 - row as i32);
            for col in 0..cols {
                let gx = self.world.wrap_x(origin_col + col as i32);
                let px = present::cell_rgba(self.world.get_cell(gx, gy));
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
        ViewFrame {
            contract_version: VIEW_SCHEMA,
            sim_tick: self.world.tick,
            paused: self.paused,
            world_width_cols: self.params.width_cols,
            world_height: self.world_height(),
            focus_col: self.focus_col,
            view_cols: self.view_cols,
            last_step_ms: self.last_step_ms,
            terrain: self.extract_terrain(),
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
    }
}
