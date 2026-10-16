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

pub const SAVE_VERSION: u32 = 8;

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

/// Suspended fines settle out of the water column: every
/// `FINES_SETTLE_EVERY` ticks a column loses `1/FINES_SETTLE_DIV` of its
/// fines to soft litter on its floor (τ ≈ 60 s), so a die-off clouds the
/// water for minutes, not forever (K2#8: matter stays in the tank).
pub const FINES_SETTLE_EVERY: u64 = 30;
pub const FINES_SETTLE_DIV: f32 = 60.0;
/// `cloud[x] = exp(−CLOUD_K · fines[x])` (fines in grams; §4).
pub const CLOUD_K: f32 = 0.004;
/// Settled fines pile this many pixels up a column before they wait.
pub const FINES_PILE_PX: usize = 72;
/// Share of open-water lysis that drifts off as fines (Q8, 10 %).
pub const LYSIS_FINES_Q8: u32 = 26;
/// Neighbouring columns share fines (3-tap blur) this often.
pub const FINES_BLUR_EVERY: u64 = 16;
/// "Water is cloudy" when the mean cloud factor falls under this, with a
/// cooldown before it can fire again.
pub const CLOUDY_MEAN: f32 = 0.6;
pub const CLOUDY_COOLDOWN: u64 = 5400;
/// Swirl resuspension: `fines[x] += SWIRL_RESUSPEND · |tilt| · litter_g`
/// per second (§4), applied every `FINES_SETTLE_EVERY` ticks.
pub const SWIRL_RESUSPEND: f32 = 0.02;

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
pub const ACTIVITY_FLOOR_Q8: i64 = 24;

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

// ---- Phase 4: thermal cycle (`evolution-system.md` §4), THERMO (§a.7) ----

/// Solar heating at full light: `T_direct = ΔT_light · L` (≈ 25 °C at
/// midday on the 15 °C default).
pub const DT_LIGHT_C: f32 = 10.0;
/// `T_tank = T_base + 0.4·T_direct + 0.6·S`.
pub const T_DIRECT_WEIGHT: f32 = 0.4;
pub const T_STORED_WEIGHT: f32 = 0.6;
/// Stored-heat time constant per material class, as a fraction of the
/// full tank day (`τ = CYCLE_TICKS · Σ fraction · weight`): air, water,
/// sand / soil, rock / glass, biomass.
pub const TAU_AIR: f32 = 0.02;
pub const TAU_WATER: f32 = 0.08;
pub const TAU_SAND: f32 = 0.12;
pub const TAU_ROCK: f32 = 0.20;
pub const TAU_BIO: f32 = 0.10;
/// Material fractions are re-counted this often (ticks).
pub const THERMAL_MASS_EVERY: u64 = 64;
/// `T_local` is stored in quarter degrees; THERMO may lift a coarse cell
/// at most this far over ambient (+12 °C).
pub const T_LOCAL_CAP_Q2: i8 = 48;
/// A THERMO unit adds one quarter degree to its coarse cell every this
/// many ticks (+0.75 °C per unit per second, §a.7).
pub const THERMO_INJECT_EVERY: u64 = 10;
/// `T_local` leaks toward ambient: every `T_LOCAL_RELAX_EVERY` ticks a
/// cell loses `max(1, v / T_LOCAL_RELAX_DIV)` (τ ≈ 4 s), and diffuses to
/// its four neighbours every `T_LOCAL_DIFFUSE_EVERY` ticks.
pub const T_LOCAL_RELAX_EVERY: u64 = 4;
pub const T_LOCAL_RELAX_DIV: i16 = 30;
pub const T_LOCAL_DIFFUSE_EVERY: u64 = 16;
/// Running THERMO costs 0.04 g per unit per second (§a.7).
pub const THERMO_COST_FP_TICK_PER_UNIT: i32 = FP * 4 / 100 / TICK_HZ as i32;
/// "Warm-blooded": a THERMO cell that has held ≥ +3 °C (12 quarter
/// degrees) over ambient for this many ticks.
pub const WARM_BLOODED_Q2: i8 = 12;
pub const WARM_BLOODED_TICKS: u8 = 60;

// ---- Phase 4: humidity, evaporation, condensation (K2#5, K2#8) ----

/// The air-side humidity field updates this often (ticks).
pub const HUMIDITY_EVERY: u64 = 8;
/// Humidity an air cell over free water is driven toward by day at
/// 15 °C, plus `HUMIDITY_PER_C` per degree (warm water evaporates more).
pub const HUMIDITY_EVAP_BASE: f32 = 176.0;
pub const HUMIDITY_PER_C: f32 = 4.0;
/// A substrate cell counts as moist (habitability, humidity source) from
/// this `sat` / film (≈ 0.3 of full).
pub const MOIST_SAT: u8 = 80;
/// Each update an air cell leaks `h / HUMIDITY_LEAK_DIV` (the lid is not
/// perfectly tight), so humidity falls off gently with distance from
/// water instead of saturating the whole air side.
pub const HUMIDITY_LEAK_DIV: u16 = 1024;
/// Night: humid air (≥ this) condenses on glass, rock and sand it touches,
/// depositing `DEW_GAIN` per update and losing half of that itself
/// (a pane fogs over in ≈ 15 min of night).
pub const DEW_H: u8 = 200;
pub const DEW_GAIN: u8 = 4;
/// Glass dew fades by this much per update when the air beside it is dry.
pub const DEW_FADE: u8 = 2;
/// Evaporation (K2#8, very slow; water only): every `EVAP_EVERY` ticks the
/// free surface of each column loses `EVAP_SAT_STEP` sat unless the air
/// above is saturated (≥ `EVAP_STOP_H`). 255 sat / 10 s ≈ 42 min per
/// physics row — the waterline drops visibly over hours.
pub const EVAP_EVERY: u64 = 300;
pub const EVAP_SAT_STEP: u8 = 1;
pub const EVAP_STOP_H: u8 = 250;

// ---- Phase 4: hydration, roots, desiccation (`cell-language.md` §h.1) ----

/// Hydration is recomputed this often (ticks); also on topology change.
pub const HYDRATION_EVERY: u64 = 32;
/// Drop per division-bond hop when the conductor is soft-walled.
pub const HYDRATION_DROP: u8 = 16;
/// Drop when the conductor has WALL h ≥ 0.5 (hard-walled xylem).
pub const HYDRATION_DROP_HARD: u8 = 8;
/// Baseline hydration from contact with moist substrate (no UPTAKE).
pub const HYDRATION_CONTACT: u8 = 128;
/// Air humidity (or substrate moisture) at or above this is habitable.
pub const HABITABLE_H: u8 = 80;
/// Desiccation: every `DESICCATE_EVERY` ticks a dry air-side cell takes
/// `DESICCATE_BASE · (1 − h_wall) · (1 − max(humidity, hydration)/255)`
/// damage (Q8 factors).
pub const DESICCATE_EVERY: u64 = 8;
pub const DESICCATE_BASE: u8 = 4;
/// A physics cell with this many root pixels is *bound* (no slump).
pub const ROOT_BOUND_PIXELS: u8 = 5;
