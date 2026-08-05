//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Hibernating spore bank — spores that land where they cannot germinate
//! (crowded, dry, cold, buried) stay associated with the landing cell and
//! may sprout far later when conditions improve.
//!
//! Crude by design: sparse `(gx, gy)` map, slow step cadence, reuse existing
//! seat / moisture / density predicates. Not a per-tick chemistry field.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::blueprint::Genome;
use crate::fungi::{
    count_fungi_near, is_fungus_seated, organic_cells_near, seed_mycelium_near, soft_litter_at,
    FUNGUS_SPORE_LOCAL_MAX, FUNGUS_SPORE_LOCAL_RADIUS, FUNGUS_STARVE_UNITS,
};
use crate::grid::World;
use crate::organism::{Atom, BodyModule};
use crate::plant::{
    apply_genome, cell_moisture_frac, count_plants_near, crown_clearance_ok, find_fungus_slot,
    find_plant_slot, is_anchored, is_land_plant, pin_plant_pose, sync_alloc_to_body,
    SPROUT_LOCAL_MAX, SPROUT_LOCAL_RADIUS,
};
use crate::temperature::Temperature;

/// Cadence for bank wake attempts (independent of living organism step).
pub const SPORE_BANK_PERIOD: u64 = 64;
/// Default max dormant packets per landing cell.
pub const SPORE_BANK_MAX_PER_CELL: u8 = 4;
/// Default global dormancy ceiling (perf / anti-flood).
pub const SPORE_BANK_MAX_TOTAL: u16 = 512;
/// Default viability window (~a few long soaks).
pub const SPORE_BANK_MAX_AGE: u64 = 200_000;
/// 1-in-N chance an eligible dormant spore germinates on a wake pulse.
pub const SPORE_BANK_GERMINATE_ODDS: u64 = 7;
/// Below this °C, plant/fungus spores stay dormant (crude cold gate).
pub const SPORE_BANK_MIN_TEMP_C: f32 = 2.0;

/// Habit carried by a dormant packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SporeKind {
    Plant,
    Fungus,
}

/// One hibernating spore tied to a world cell.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DormantSpore {
    pub kind: SporeKind,
    pub genome: Genome,
    pub body: Vec<BodyModule>,
    /// Starter energy when it finally germinates.
    pub energy: f32,
    pub deposited_tick: u64,
    /// Fungus: prefer surface Air seats on wake (stalks).
    #[serde(default)]
    pub prefer_surface: bool,
}

/// Live Tab knobs for the spore bank.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SporeBankConfig {
    pub enabled: bool,
    pub step_period: u64,
    pub max_per_cell: u8,
    pub max_total: u16,
    pub max_age_ticks: u64,
    pub germinate_odds: u64,
    pub min_temp_c: f32,
    pub plant_min_moist: f32,
}

impl Default for SporeBankConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            step_period: SPORE_BANK_PERIOD,
            max_per_cell: SPORE_BANK_MAX_PER_CELL,
            max_total: SPORE_BANK_MAX_TOTAL,
            max_age_ticks: SPORE_BANK_MAX_AGE,
            germinate_odds: SPORE_BANK_GERMINATE_ODDS,
            min_temp_c: SPORE_BANK_MIN_TEMP_C,
            plant_min_moist: 0.02,
        }
    }
}

/// Sparse cell-tied dormant spores (saved with the world).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SporeBank {
    /// Keyed by `(wrap_x(gx), gy)` — stays with the landing block through
    /// burial; wake looks for a nearby seat when the cell is covered.
    #[serde(default)]
    pub cells: HashMap<(i32, i32), Vec<DormantSpore>>,
}

impl SporeBank {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.cells.values().map(|v| v.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Deposit a spore on the landing cell. `gx` should already be
    /// [`World::wrap_x`]'d. Drops oldest in-cell when full; refuses when
    /// the global ceiling is hit.
    pub fn deposit(
        &mut self,
        gx: i32,
        gy: i32,
        spore: DormantSpore,
        cfg: &SporeBankConfig,
    ) -> bool {
        if !cfg.enabled || cfg.max_total == 0 || cfg.max_per_cell == 0 {
            return false;
        }
        if self.len() >= cfg.max_total as usize {
            return false;
        }
        let key = (gx, gy);
        let slot = self.cells.entry(key).or_default();
        let cap = cfg.max_per_cell as usize;
        if slot.len() >= cap {
            // Keep the youngest bank — oldest leaves first.
            slot.remove(0);
        }
        slot.push(spore);
        true
    }

    /// Wake pulse: try to germinate a few eligible dormant spores.
    ///
    /// Returns newly living atoms (caller inserts into [`OrganismStore`]).
    pub fn step(
        &mut self,
        world: &mut World,
        tick: u64,
        cfg: &SporeBankConfig,
        temp: Option<&Temperature>,
        plant_cols: &[i32],
        fungus_cols: &[i32],
        pop_room: usize,
    ) -> Vec<Atom> {
        if !cfg.enabled || self.cells.is_empty() {
            return Vec::new();
        }
        let period = cfg.step_period.max(1);
        if tick % period != 0 {
            return Vec::new();
        }
        let odds = cfg.germinate_odds.max(1);
        let max_age = cfg.max_age_ticks;
        let mut births = Vec::new();
        let mut room = pop_room;
        if room == 0 {
            return births;
        }

        // Snapshot keys so we can mutate the map while iterating.
        let mut keys: Vec<(i32, i32)> = self.cells.keys().copied().collect();
        keys.sort_unstable();

        let mut empty_keys = Vec::new();
        for key in keys {
            if room == 0 {
                break;
            }
            let Some(stack) = self.cells.get_mut(&key) else {
                continue;
            };
            // Expire from the front; try at most one germinate per cell / pulse.
            let mut i = 0;
            while i < stack.len() {
                let age = tick.saturating_sub(stack[i].deposited_tick);
                if max_age > 0 && age > max_age {
                    stack.remove(i);
                    continue;
                }
                let h = hash_u64(world.seed.0, tick, key.0 as u64, key.1 as u64 ^ 0x5B0A);
                if h % odds != 0 {
                    i += 1;
                    continue;
                }
                let spore = stack[i].clone();
                if let Some(child) = try_wake_spore(
                    world,
                    key.0,
                    key.1,
                    &spore,
                    cfg,
                    temp,
                    plant_cols,
                    fungus_cols,
                ) {
                    stack.remove(i);
                    births.push(child);
                    room = room.saturating_sub(1);
                    // One germinate per cell per pulse.
                    break;
                } else {
                    i += 1;
                }
            }
            if stack.is_empty() {
                empty_keys.push(key);
            }
        }
        for k in empty_keys {
            self.cells.remove(&k);
        }
        births
    }
}

fn try_wake_spore(
    world: &mut World,
    gx: i32,
    gy: i32,
    spore: &DormantSpore,
    cfg: &SporeBankConfig,
    temp: Option<&Temperature>,
    plant_cols: &[i32],
    fungus_cols: &[i32],
) -> Option<Atom> {
    // Buried: solid where we landed and no free Air seat nearby → wait.
    // Cold: stay dormant through freezes / winter snaps.
    if let Some(t) = temp {
        if t.at_cell(gx, gy) < cfg.min_temp_c {
            return None;
        }
    }
    match spore.kind {
        SporeKind::Plant => wake_plant(world, gx, gy, spore, cfg, plant_cols),
        SporeKind::Fungus => wake_fungus(world, gx, gy, spore, fungus_cols),
    }
}

fn wake_plant(
    world: &World,
    gx: i32,
    gy: i32,
    spore: &DormantSpore,
    cfg: &SporeBankConfig,
    plant_cols: &[i32],
) -> Option<Atom> {
    if !crown_clearance_ok(plant_cols, gx, world.wrap_width) {
        return None;
    }
    let local = count_plants_near(plant_cols, gx, SPROUT_LOCAL_RADIUS, world.wrap_width);
    if local >= SPROUT_LOCAL_MAX {
        return None;
    }
    let seat_y = find_plant_slot(world, gx, gy)?;
    let moist = cell_moisture_frac(world, gx, seat_y - 1);
    if moist < cfg.plant_min_moist {
        return None;
    }
    let tank = spore.energy.max(8.0);
    let mut genome = spore.genome;
    sync_alloc_to_body(&mut genome, &spore.body);
    let mut child = Atom::from_body(gx, seat_y, tank, spore.body.clone());
    apply_genome(&mut child, genome);
    child.energy = spore.energy.clamp(1.0, child.energy_max);
    pin_plant_pose(&mut child);
    if !is_land_plant(&child) || !is_anchored(world, &child) {
        return None;
    }
    Some(child)
}

fn wake_fungus(
    world: &mut World,
    gx: i32,
    gy: i32,
    spore: &DormantSpore,
    fungus_cols: &[i32],
) -> Option<Atom> {
    if fungus_cols
        .iter()
        .any(|&ox| world.wrap_x(ox) == world.wrap_x(gx))
    {
        return None;
    }
    let local = count_fungi_near(
        fungus_cols,
        gx,
        FUNGUS_SPORE_LOCAL_RADIUS,
        world.wrap_width,
    );
    if local >= FUNGUS_SPORE_LOCAL_MAX {
        return None;
    }
    let seat_y = if spore.prefer_surface {
        find_fungus_slot(world, gx, gy)?
    } else {
        crate::plant::find_fungus_slot_biased(world, gx, gy, false)?
    };
    let organic = organic_cells_near(world, gx, seat_y) as f32;
    let litter = soft_litter_at(world, gx) as f32;
    if organic < 1.0 && litter < FUNGUS_STARVE_UNITS {
        return None;
    }
    let tank = spore.energy.max(8.0);
    let mut child = Atom::from_body(gx, seat_y, tank, spore.body.clone());
    apply_genome(&mut child, spore.genome);
    child.energy = spore.energy.clamp(1.0, child.energy_max);
    pin_plant_pose(&mut child);
    if !is_fungus_seated(world, &child) {
        return None;
    }
    seed_mycelium_near(world, gx, seat_y, 16);
    Some(child)
}

/// Result of a live dispersal attempt (immediate germinate vs bank vs no-op).
#[derive(Debug)]
pub enum DispersalResult {
    Germinated(Atom),
    /// Paid the dispersal cost; packet sleeps at `(gx, gy)`.
    Banked { gx: i32, gy: i32 },
    /// No attempt (cooldown / rarity / no launch tissue).
    None,
}

fn hash_u64(a: u64, b: u64, c: u64, salt: u64) -> u64 {
    let mut x = a
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(b)
        .wrapping_add(c)
        .wrapping_add(salt);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x
}

/// Helper: build a dormant packet from a would-be child atom.
pub fn packet_from_child(kind: SporeKind, child: &Atom, tick: u64, prefer_surface: bool) -> DormantSpore {
    DormantSpore {
        kind,
        genome: child.genome,
        body: child.body.clone(),
        energy: child.energy.max(1.0),
        deposited_tick: tick,
        prefer_surface,
    }
}

/// True when a plant seat at `(gx, gy)` is currently germinable.
pub fn plant_seat_ready(
    world: &World,
    gx: i32,
    gy: i32,
    plant_cols: &[i32],
    min_moist: f32,
) -> bool {
    if !crown_clearance_ok(plant_cols, gx, world.wrap_width) {
        return false;
    }
    let local = count_plants_near(plant_cols, gx, SPROUT_LOCAL_RADIUS, world.wrap_width);
    if local >= SPROUT_LOCAL_MAX {
        return false;
    }
    let Some(seat_y) = find_plant_slot(world, gx, gy) else {
        return false;
    };
    cell_moisture_frac(world, gx, seat_y - 1) >= min_moist
}

/// Count dormant spores (inspector / HUD).
pub fn spore_bank_len(world: &World) -> usize {
    world.spore_bank.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::{Cell, Sat};
    use crate::chunk::ChunkCoord;

    fn plot() -> World {
        let mut w = World::new(11);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..24 {
            w.set_cell(x, 0, Cell::solid(wk_material::MaterialId::Bedrock));
            let mut sand = Cell::solid(wk_material::MaterialId::Sand);
            sand.sat = Sat(180);
            w.set_cell(x, 1, sand);
            for y in 2..10 {
                w.set_cell(x, y, Cell::air());
            }
        }
        w
    }

    fn plant_packet(tick: u64) -> DormantSpore {
        let body = crate::blueprint::Blueprint::minimal_plant().modules_relative_to_nucleus();
        DormantSpore {
            kind: SporeKind::Plant,
            genome: Genome::default(),
            body,
            energy: 20.0,
            deposited_tick: tick,
            prefer_surface: true,
        }
    }

    fn wake(
        w: &mut World,
        tick: u64,
        cfg: &SporeBankConfig,
        temp: Option<&Temperature>,
        plants: &[i32],
        fungi: &[i32],
        room: usize,
    ) -> Vec<Atom> {
        let mut bank = std::mem::take(&mut w.spore_bank);
        let out = bank.step(w, tick, cfg, temp, plants, fungi, room);
        w.spore_bank = bank;
        out
    }

    #[test]
    fn deposit_associates_with_landing_cell() {
        let mut w = plot();
        let cfg = SporeBankConfig::default();
        assert!(w.spore_bank.deposit(8, 2, plant_packet(0), &cfg));
        assert_eq!(w.spore_bank.len(), 1);
        assert!(w.spore_bank.cells.contains_key(&(8, 2)));
    }

    #[test]
    fn dry_seat_stays_banked_until_moist() {
        let mut w = plot();
        // Bone-dry sand under the would-be crown.
        w.set_cell(8, 1, Cell::solid(wk_material::MaterialId::Sand));
        let cfg = SporeBankConfig {
            step_period: 1,
            germinate_odds: 1,
            plant_min_moist: 0.02,
            ..SporeBankConfig::default()
        };
        w.spore_bank.deposit(8, 2, plant_packet(0), &cfg);
        w.tick = 64;
        let births = wake(&mut w, 64, &cfg, None, &[], &[], 8);
        assert!(births.is_empty(), "dry bed must not germinate");
        assert_eq!(w.spore_bank.len(), 1);

        let mut wet = Cell::solid(wk_material::MaterialId::Sand);
        wet.sat = Sat(200);
        w.set_cell(8, 1, wet);
        let births = wake(&mut w, 128, &cfg, None, &[], &[], 8);
        assert_eq!(births.len(), 1, "moist bed should wake the bank");
        assert!(w.spore_bank.is_empty());
        assert!(is_anchored(&w, &births[0]));
    }

    #[test]
    fn crowded_seat_waits_for_clearance() {
        let mut w = plot();
        let mut wet = Cell::solid(wk_material::MaterialId::Sand);
        wet.sat = Sat(200);
        w.set_cell(8, 1, wet);
        let cfg = SporeBankConfig {
            step_period: 1,
            germinate_odds: 1,
            ..SporeBankConfig::default()
        };
        w.spore_bank.deposit(8, 2, plant_packet(0), &cfg);
        // Living crown on the same column blocks clearance.
        let occupied = [8i32];
        let births = wake(&mut w, 64, &cfg, None, &occupied, &[], 8);
        assert!(births.is_empty(), "crowded column must stay banked");
        let births = wake(&mut w, 128, &cfg, None, &[], &[], 8);
        assert_eq!(births.len(), 1, "clear column should germinate");
    }

    #[test]
    fn cold_gate_blocks_wake() {
        let mut w = plot();
        let mut wet = Cell::solid(wk_material::MaterialId::Sand);
        wet.sat = Sat(200);
        w.set_cell(8, 1, wet);
        let cfg = SporeBankConfig {
            step_period: 1,
            germinate_odds: 1,
            min_temp_c: 5.0,
            ..SporeBankConfig::default()
        };
        w.spore_bank.deposit(8, 2, plant_packet(0), &cfg);
        let mut temp = Temperature::with_world_bounds(4, 0, 0, 24, 16, 1, 24, 8, false);
        temp.config.base_temp_c = -10.0;
        // Force a cold tile reading without a full climate step.
        let (hx, hy) = temp.tile_of(8, 2);
        temp.cells.insert((hx, hy), -10.0);
        let births = wake(&mut w, 64, &cfg, Some(&temp), &[], &[], 8);
        assert!(births.is_empty(), "cold must keep spores dormant");
        temp.cells.insert((hx, hy), 12.0);
        let births = wake(&mut w, 128, &cfg, Some(&temp), &[], &[], 8);
        assert_eq!(births.len(), 1, "warm-up should wake the bank");
    }

    #[test]
    fn max_age_expires_dormant_spores() {
        let mut w = plot();
        let cfg = SporeBankConfig {
            step_period: 1,
            germinate_odds: 1_000_000, // never germinate
            max_age_ticks: 100,
            ..SporeBankConfig::default()
        };
        w.spore_bank.deposit(8, 2, plant_packet(0), &cfg);
        let _ = wake(&mut w, 200, &cfg, None, &[], &[], 8);
        assert!(w.spore_bank.is_empty(), "stale spores must expire");
    }

    #[test]
    fn buried_plant_finds_air_seat_above_pile() {
        let mut w = plot();
        let mut wet = Cell::solid(wk_material::MaterialId::Sand);
        wet.sat = Sat(200);
        w.set_cell(8, 1, wet);
        // Bury the landing Air cell under moist Organic.
        for y in 2..=3 {
            let mut org = Cell::solid(wk_material::MaterialId::Organic);
            org.sat = Sat(160);
            w.set_cell(8, y, org);
        }
        w.set_cell(8, 4, Cell::air());
        let cfg = SporeBankConfig {
            step_period: 1,
            germinate_odds: 1,
            ..SporeBankConfig::default()
        };
        // Spore associated with buried cell (8,2).
        w.spore_bank.deposit(8, 2, plant_packet(0), &cfg);
        // find_plant_slot should still see Air at y=4 above the pile — that's
        // "uncovered surface above the burial column", which we allow.
        let births = wake(&mut w, 64, &cfg, None, &[], &[], 8);
        assert_eq!(
            births.len(),
            1,
            "column with Air above burial should seat a plant at the surface"
        );
        assert!(births[0].gy >= 4);
    }
}
