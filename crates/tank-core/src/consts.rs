//! Tuning seeds from `docs/implementation-handoff.md` §5.
//! Change them here; the rest of the core reads these names.

/// Simulation steps per second. One tick is 1/30 s of tank time.
pub const TICK_HZ: u64 = 30;

/// Daylight half of the tank clock (20 minutes).
pub const DAY_TICKS: u64 = 20 * 60 * TICK_HZ;

/// Dark half of the tank clock (20 minutes).
pub const NIGHT_TICKS: u64 = 20 * 60 * TICK_HZ;

/// Full tank day, noon to the next noon.
pub const CYCLE_TICKS: u64 = DAY_TICKS + NIGHT_TICKS;

/// Light level through the dark half. K4#1.
pub const NIGHT_FLOOR: f32 = 0.1;

/// Default base temperature, °C.
pub const T_BASE_DEFAULT: f32 = 15.0;

/// Visible physics cells for the phone preset.
pub const PRESET_W: u32 = 128;
pub const PRESET_H: u32 = 256;

/// Biology pixels per physics cell edge.
pub const PX_PER_CELL: u32 = 3;

/// Free-water saturation at or above this is a standing surface.
/// Same threshold as GvsE `STANDING_AIR_SAT`.
pub const STANDING_AIR_SAT: u8 = 160;

/// Air darker than this still renders and attenuates as air, not water.
pub const WATER_SAT_RENDER: u8 = 24;

/// Water substeps per tick (gravity, lateral flow, head).
pub const WATER_SUBSTEPS: u32 = 2;

/// Grain fall + repose passes per tick.
pub const GRAIN_PASSES: u32 = 2;

/// Hard cap on a visible tank. The phone preset is 128×256.
pub const MAX_VISIBLE_W: u32 = 256;
pub const MAX_VISIBLE_H: u32 = 512;
pub const MIN_VISIBLE: u32 = 4;

pub const SAVE_VERSION: u32 = 1;

/// Protein units per biology pixel (handoff §5).
pub const P_UNITS: u8 = 8;

/// Phase 2 shape cap (hard cap is 6; Phase 2 uses 4).
pub const SHAPE_CAP: u8 = 4;
