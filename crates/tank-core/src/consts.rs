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

pub const SAVE_VERSION: u32 = 5;

// ---- biology (cell-language §a, §b, §c) ----

/// Protein units per biology pixel (handoff §5).
pub const P_UNITS: u8 = 8;

/// Phase 2 shape cap (hard cap is 6; Phase 2 uses 4).
pub const SHAPE_CAP: u8 = 4;

/// Fixed-point energy: `FP` = 1 g of glucose.
pub const FP: i32 = 1024;

/// One unit of the dissolved-glucose and soft-litter pixel fields (1/64 g,
/// so a pixel holds ≤ 4 g). Baseline uptake drinks one unit per pixel per
/// tick; UPTAKE(glucose) multiplies that (§a.2, §a.4).
pub const GLUCOSE_UNIT_FP: i32 = FP / 64;

/// Ticks between pixel growth steps toward the mode shape.
pub const T_GROW: u16 = 8;

/// Structure cost per pixel, g (§a.1).
pub const STRUCTURE_G_FP: i32 = FP;

/// Structure upkeep per pixel per tick (0.01 g/s).
pub const STRUCTURE_UPKEEP_FP_TICK: i32 = FP / 100 / TICK_HZ as i32;

/// PHOTO income per unit per tick at full light, milli-fp. Tuned so a 1×1
/// algae in full light divides every ≈10 s (d.1 pace).
pub const PHOTO_YIELD_MFP: i64 = 7200;

/// Store capacity: base per pixel plus 2 g per STORE unit (§a.4).
pub const STORE_BASE_FP: i32 = 12 * FP;
pub const STORE_PER_UNIT_FP: i32 = 2 * FP;

/// Energy a daughter starts with (bonded parts are topped up by the body).
pub const BIRTH_ENERGY_FP: i32 = 3 * FP;

/// Metabolic activity floor (Q8): an idle cell burns and ages at ¼ rate.
pub const ACTIVITY_FLOOR_Q8: i64 = 64;

/// Default `base_age`, seconds; the pace law scales it (§b.6).
pub const BASE_AGE_S: u8 = 60;

/// Reference body cost for the pace law: the 1×1 algae (6 g).
pub const ALGAE_COST_FP: i32 = 6 * FP;

/// λ0 per division, milli. §c.2 proposes 0.3, which with the strategy
/// term puts the seed algae above 0.5 mutations per division and makes
/// nothing breed true; 0.1 (≈0.19 for the algae at rest) is our default.
pub const LAMBDA0_MILLI: u32 = 100;

/// Mode-program caps (§e.4).
pub const MAX_MODES: usize = 16;
pub const MAX_CHILDREN: usize = 4;
pub const MAX_TRANSITIONS: usize = 3;
pub const MAX_ENTRIES: usize = 6;
/// Intra-cell links per mode (§a.6).
pub const MAX_LINKS: usize = 4;

/// Realised-map slots per organism.
pub const MAX_SLOTS: usize = 64;

/// Live-cell soft cap (handoff lever).
pub const LIVE_CAP: usize = 30_000;

/// No mitosis below this tank temperature (°C).
pub const MITOSIS_FLOOR_C: f32 = 4.0;

/// Heat damage above this tank temperature (°C).
pub const HEAT_DAMAGE_C: f32 = 38.0;

/// Fraction of bond energy difference moved per tick (1/N).
pub const BOND_SHARE_DIV: i32 = 8;

/// Hard living pixels (of 9) that turn a physics cell solid to flow (§b.3).
pub const TISSUE_DAM_PIXELS: u8 = 5;

/// Light absorbed per PHOTO pixel row (×0.5) and per other tissue (×0.75), Q8.
pub const TISSUE_PASS_PHOTO_Q8: u32 = 128;
pub const TISSUE_PASS_OTHER_Q8: u32 = 192;
