//! Phase 8 ship-client contract types + in-process host (spike).
//!
//! See [`docs/PHASE8_CONTRACT.md`]. The host owns the tick; the client only
//! queues [`ClientCommand`]s and paints [`ViewFrame`]s. No sat/mass mutation
//! lives here.

/// Bump when [`ViewFrame`] field layout breaks.
pub const VIEW_SCHEMA: u32 = 1;
/// Bump when [`ClientCommand`] enum breaks.
pub const COMMAND_SCHEMA: u32 = 1;

/// Camera-window extract for presentation (not a durable [`SimSnapshot`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ViewFrame {
    pub contract_version: u32,
    pub sim_tick: u64,
    pub paused: bool,
    pub world_width_cols: i32,
    pub world_height: i32,
    pub focus_col: i32,
    pub view_cols: i32,
    /// Wall-clock ms of the last host step (spike: synthetic).
    pub last_step_ms: u32,
    pub command_acks: Vec<CommandAck>,
}

impl ViewFrame {
    pub fn schema_ok(&self) -> bool {
        self.contract_version == VIEW_SCHEMA
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

/// Minimal in-process host for the empty-window spike.
///
/// Advances a tick counter only — no `wk-voxel` world yet. Terrain strip
/// lands after this loop is stable.
#[derive(Debug)]
pub struct Host {
    pub tick: u64,
    pub paused: bool,
    pub focus_col: i32,
    pub view_cols: i32,
    pub world_width_cols: i32,
    pub world_height: i32,
    pub quit_requested: bool,
    next_ack_id: u64,
    pending_acks: Vec<CommandAck>,
}

impl Default for Host {
    fn default() -> Self {
        Self {
            tick: 0,
            paused: false,
            focus_col: 0,
            view_cols: 64,
            world_width_cols: 4096,
            world_height: 1064,
            quit_requested: false,
            next_ack_id: 1,
            pending_acks: Vec::new(),
        }
    }
}

impl Host {
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
                    self.view_cols = view_cols;
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
                self.tick = self.tick.saturating_add(1);
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

    /// One product frame: drain already applied; step if running; emit view.
    pub fn frame(&mut self) -> ViewFrame {
        if !self.paused {
            self.tick = self.tick.saturating_add(1);
        }
        let acks = std::mem::take(&mut self.pending_acks);
        ViewFrame {
            contract_version: VIEW_SCHEMA,
            sim_tick: self.tick,
            paused: self.paused,
            world_width_cols: self.world_width_cols,
            world_height: self.world_height,
            focus_col: self.focus_col,
            view_cols: self.view_cols,
            last_step_ms: 0,
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
        assert_eq!(t1, t0 + 1);
        host.apply(ClientCommand::SetPaused(true));
        let t2 = host.frame().sim_tick;
        assert_eq!(t2, t1);
        host.apply(ClientCommand::StepOnce);
        let t3 = host.frame().sim_tick;
        // StepOnce bumps once; frame while paused does not bump again.
        assert_eq!(t3, t1 + 1);
    }

    #[test]
    fn camera_rejects_non_positive_view() {
        let mut host = Host::default();
        let ack = host.apply(ClientCommand::SetCamera {
            focus_col: 10,
            view_cols: 0,
        });
        assert!(!ack.ok);
        assert_eq!(host.view_cols, 64);
    }

    #[test]
    fn view_schema_matches() {
        let frame = Host::default().frame();
        assert!(frame.schema_ok());
        assert_eq!(frame.contract_version, VIEW_SCHEMA);
        assert_eq!(COMMAND_SCHEMA, 1);
    }
}
