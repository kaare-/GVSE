//! Interned genomes. One record per genotype; the table is the lineage
//! tree (parent pointer + live/born counts). Extinct leaves fold into
//! their parent edge, cascading up through extinct ancestors.
//! Spec: `cell-language.md` §c.1–§c.4.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::bio::profile::{Block, Entry, Mode, ModeCoeffs, SENSOR_LIGHT};
use crate::bio::species::{self, SpeciesRec, NO_SPECIES};
use crate::bio::program::{
    rewire_child, rewire_transition, ChildKind, ChildSpec, Detach, Dir, Gate, HeadingRule, Link,
    Scalar, Transition,
};
use crate::consts::{BASE_AGE_S, LAMBDA0_MILLI, MAX_MODES};
use crate::hash::hash_u64;

/// Stress weights (§c.1 `stress_def`): what counts as stress for this lineage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StressDef {
    pub hunger: u8,
    pub heat: u8,
    pub cold: u8,
    pub damage: u8,
    pub crowding: u8,
    pub waste: u8,
    pub toxin: u8,
    pub dark: u8,
}

/// Full genome: header, baseline, strategy, stress weights, keys, modes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Genome {
    /// 64..=192; 128 = 1.0 metabolic efficiency.
    pub metab_eff: u8,
    /// Buoyancy bias; positive floats toward the light.
    pub buoy: i8,
    pub temp_opt: u8,
    pub temp_width: u8,
    /// Seconds; pace law scales it (§b.6).
    pub base_age: u8,
    /// Body-store threshold below which extremities are shed (§b.5), /64 g per cell.
    pub sacrifice_threshold: u8,
    pub cheap: u8,
    pub fast: u8,
    pub accurate: u8,
    pub stress: StressDef,
    /// Living-wall substrate code, clamped to 64..=191 (§b.1).
    pub wall_key: u8,
    pub glue_key: u8,
    pub glue_width: u8,
    pub resist_key: u8,
    pub seed_mode: u8,
    pub modes: Vec<Mode>,
}

pub const WALL_KEY_MIN: u8 = 64;
pub const WALL_KEY_MAX: u8 = 191;

impl Genome {
    pub(crate) fn base() -> Self {
        Self {
            metab_eff: 128,
            buoy: 20,
            temp_opt: 22,
            temp_width: 8,
            base_age: BASE_AGE_S,
            sacrifice_threshold: 32,
            cheap: 100,
            fast: 100,
            accurate: 55,
            stress: StressDef {
                hunger: 180,
                heat: 160,
                cold: 160,
                damage: 160,
                crowding: 60,
                waste: 80,
                toxin: 200,
                dark: 120,
            },
            wall_key: 128,
            glue_key: 137,
            glue_width: 16,
            resist_key: 40,
            seed_mode: 0,
            modes: Vec::new(),
        }
    }

    /// d.1 — the seed algae.
    pub fn algae() -> Self {
        let mut g = Self::base();
        g.modes.push(Mode::algae());
        g
    }

    /// d.3 — 3-cell straw: mode 0 chain (Ahead, bonded, repeat 2) with a
    /// depth ≥ 2 transition into terminal mode 1, which releases propagules.
    pub fn straw() -> Self {
        let mut g = Self::base();
        g.buoy = 0;
        let mut stem = Mode::blank();
        stem.push_entry(Entry::new(Block::Photo, 4));
        stem.push_entry(Entry::with_params(Block::Wall, 1, 77, 0)); // h 0.3
        stem.push_child(ChildSpec::part(Dir::Ahead, 0, 200, 2));
        stem.push_transition(Transition {
            gate: Gate::above(Scalar::Depth, 2),
            mode: 1,
        });
        stem.sacrifice_order = 1;
        let mut tip = Mode::blank();
        tip.push_entry(Entry::new(Block::Photo, 5));
        tip.push_child(ChildSpec::propagule(0, 15));
        tip.sacrifice_order = 4;
        g.modes.push(stem);
        g.modes.push(tip);
        g
    }

    /// d.2 — algae that also expresses GLUE 1 (key 137, strength 60).
    pub fn sticky_algae() -> Self {
        let mut g = Self::algae();
        // 2 units: the first unit of every block entry is overhead (§a.1).
        g.modes[0].push_entry(Entry::with_params(Block::Glue, 2, 137, 60));
        g
    }

    /// d.6-pre — phototactic swimmer: PHOTO 3, CILIA 3, SENSOR(light) 2,
    /// with two intra-cell links into the motor drive: run while the
    /// light ahead beats the light here (`sense` ≥ 128), tumble when
    /// touch rises (then run on in the new direction).
    pub fn swimmer() -> Self {
        let mut g = Self::base();
        g.buoy = 0;
        let mut m = Mode::blank();
        m.push_entry(Entry::new(Block::Photo, 3));
        m.push_entry(Entry::new(Block::Cilia, 3));
        m.push_entry(Entry::with_params(Block::Sensor, 2, SENSOR_LIGHT, 0));
        m.push_link(Link::level(Scalar::Sense, 128, 96));
        m.push_link(Link::rise(Scalar::Touch, -64));
        m.push_child(ChildSpec::propagule(0, 15));
        g.modes.push(m);
        g
    }

    /// The same cell without eye or links: cilia on a plain gate (control).
    pub fn blind_swimmer() -> Self {
        let mut g = Self::base();
        g.buoy = 0;
        let mut m = Mode::blank();
        m.push_entry(Entry::new(Block::Photo, 3));
        m.push_entry(Entry::new(Block::Cilia, 3));
        m.push_child(ChildSpec::propagule(0, 15));
        g.modes.push(m);
        g
    }

    /// Phase 5 M0 — two-cell muscle twitcher: PHOTO + STORE + MUSCLE on a
    /// bonded pair; drive from energy so a fed body contracts its spring.
    pub fn twitcher() -> Self {
        use crate::bio::program::{ChildSpec, Dir, Link, Scalar};
        let mut g = Self::base();
        g.buoy = 0;
        let mut m = Mode::blank();
        m.push_entry(Entry::new(Block::Photo, 2));
        m.push_entry(Entry::new(Block::Store, 2));
        m.push_entry(Entry::new(Block::Muscle, 4));
        m.push_link(Link::level(Scalar::EnergyFrac, 32, 96));
        m.push_child(ChildSpec::part(Dir::Ahead, 0, 200, 1));
        g.modes.push(m);
        g
    }

    /// Phase 5 M3 — geometric eye: TRANSPARENT window with left/right
    /// light SENSOR children, NERVE + MUSCLE on the seed (§d.6).
    pub fn eyed() -> Self {
        use crate::bio::program::{ChildSpec, Dir, Link, Scalar};
        use crate::bio::profile::SENSOR_LIGHT;
        let mut g = Self::base();
        g.buoy = 0;
        let mut seed = Mode::blank();
        seed.shape_w = 2;
        seed.shape_h = 2;
        seed.push_entry(Entry::new(Block::Photo, 1));
        seed.push_entry(Entry::new(Block::Store, 1));
        seed.push_entry(Entry::new(Block::Nerve, 2));
        seed.push_entry(Entry::new(Block::Muscle, 2));
        seed.push_entry(Entry::new(Block::Transparent, 2));
        seed.push_link(Link::level(Scalar::Sense, 128, 80));
        seed.push_child(ChildSpec::part(Dir::Left, 1, 200, 1));
        seed.push_child(ChildSpec::part(Dir::Right, 1, 200, 1));
        let mut eye = Mode::blank();
        eye.push_entry(Entry::new(Block::Photo, 1));
        eye.push_entry(Entry::with_params(Block::Sensor, 3, SENSOR_LIGHT, 0));
        eye.push_entry(Entry::new(Block::Nerve, 1));
        g.modes.push(seed);
        g.modes.push(eye);
        g
    }

    /// Phase 5 M2 — nerved light-seeker: SENSOR(light) + NERVE + MUSCLE
    /// on a bonded pair; the StudioNet only exists because of NERVE.
    pub fn nerved() -> Self {
        use crate::bio::program::{ChildSpec, Dir, Link, Scalar};
        use crate::bio::profile::SENSOR_LIGHT;
        let mut g = Self::base();
        g.buoy = 0;
        let mut m = Mode::blank();
        // 2×2 footprint ⇒ 32 unit capacity (PHOTO1+STORE1+NERVE2+SENSOR2+MUSCLE2).
        m.shape_w = 2;
        m.shape_h = 2;
        m.push_entry(Entry::new(Block::Photo, 1));
        m.push_entry(Entry::new(Block::Store, 1));
        m.push_entry(Entry::new(Block::Nerve, 2));
        m.push_entry(Entry::with_params(Block::Sensor, 2, SENSOR_LIGHT, 0));
        m.push_entry(Entry::new(Block::Muscle, 2));
        m.push_link(Link::level(Scalar::Sense, 128, 80));
        m.push_child(ChildSpec::part(Dir::Ahead, 0, 200, 1));
        g.modes.push(m);
        g
    }

    /// Phase 5 M1 — hard–soft–hard hinge: hard bone ends with a muscle
    /// joint in the middle (studio hinge-chain shape, tank blocks).
    pub fn hinge() -> Self {
        use crate::bio::program::{ChildSpec, Dir, Link, Scalar};
        let mut g = Self::base();
        g.buoy = 0;
        // Mode 0: hard bone seed.
        let mut bone = Mode::blank();
        bone.push_entry(Entry::new(Block::Photo, 1));
        bone.push_entry(Entry::new(Block::Store, 1));
        bone.push_entry(Entry::with_params(Block::Wall, 4, 200, 0));
        bone.push_child(ChildSpec::part(Dir::Ahead, 1, 220, 1));
        // Mode 1: muscle joint.
        let mut joint = Mode::blank();
        joint.push_entry(Entry::new(Block::Photo, 1));
        joint.push_entry(Entry::new(Block::Store, 1));
        joint.push_entry(Entry::new(Block::Muscle, 5));
        joint.push_link(Link::level(Scalar::EnergyFrac, 24, 100));
        joint.push_child(ChildSpec::part(Dir::Ahead, 2, 220, 1));
        // Mode 2: hard tip.
        let mut tip = Mode::blank();
        tip.push_entry(Entry::new(Block::Photo, 1));
        tip.push_entry(Entry::with_params(Block::Wall, 4, 200, 0));
        g.modes.push(bone);
        g.modes.push(joint);
        g.modes.push(tip);
        g
    }

    /// 4×4 giant algae (K3#2 "big single cell").
    pub fn giant(shape: u8) -> Self {
        let mut g = Self::algae();
        let shape = shape.clamp(1, crate::consts::SHAPE_CAP);
        let m = &mut g.modes[0];
        let old = m.capacity();
        m.shape_w = shape;
        m.shape_h = shape;
        let new = m.capacity();
        m.rescale_units(old, new);
        // Long-lived (pace law) ⇒ needs EXCRETE (§a.4).
        m.push_entry(Entry::new(Block::Excrete, 2));
        m.divide_when.age = 30;
        g
    }

    /// 16-cell plate body: a 4-long spine (mode 0, tip mode 2) whose
    /// cells each grow a 3-cell rib to the right (modes 1 → 3 → 4); the
    /// spine tip releases propagules.
    pub fn plate16() -> Self {
        let mut g = Self::base();
        g.buoy = 0;
        let mut spine = Mode::blank();
        spine.push_entry(Entry::new(Block::Photo, 4));
        spine.push_entry(Entry::new(Block::Store, 2));
        spine.push_child(ChildSpec::part(Dir::Right, 1, 200, 1));
        spine.push_child(ChildSpec::part(Dir::Ahead, 0, 220, 1));
        spine.push_transition(Transition {
            gate: Gate::above(Scalar::Depth, 3),
            mode: 2,
        });
        let mut rib_a = Mode::blank();
        rib_a.push_entry(Entry::new(Block::Photo, 5));
        rib_a.push_child(ChildSpec::part(Dir::Ahead, 3, 180, 1));
        // Ribs turn to face the side they were placed on and grow outward.
        rib_a.heading_rule = HeadingRule::TurnRight;
        rib_a.sacrifice_order = 2;
        let mut tip = Mode::blank();
        tip.push_entry(Entry::new(Block::Photo, 4));
        tip.push_entry(Entry::new(Block::Store, 2));
        tip.push_child(ChildSpec::part(Dir::Right, 1, 200, 1));
        tip.push_child(ChildSpec::propagule(0, 15));
        tip.divide_when.energy_frac = 230;
        let mut rib_b = Mode::blank();
        rib_b.push_entry(Entry::new(Block::Photo, 5));
        rib_b.push_child(ChildSpec::part(Dir::Ahead, 4, 180, 1));
        rib_b.sacrifice_order = 3;
        let mut rib_end = Mode::blank();
        rib_end.push_entry(Entry::new(Block::Photo, 5));
        rib_end.sacrifice_order = 5;
        g.modes.push(spine);
        g.modes.push(rib_a);
        g.modes.push(tip);
        g.modes.push(rib_b);
        g.modes.push(rib_end);
        g
    }

    /// d.8 — brood body: a 2-cell base whose lining cell places a bonded
    /// seed-mode egg that detaches on `age_frac ≥ 0.3`.
    pub fn brooder() -> Self {
        let mut g = Self::base();
        g.buoy = 0;
        let mut base = Mode::blank();
        base.push_entry(Entry::new(Block::Photo, 4));
        base.push_entry(Entry::new(Block::Store, 3));
        base.push_child(ChildSpec::part(Dir::Ahead, 1, 200, 1));
        let mut lining = Mode::blank();
        lining.push_entry(Entry::new(Block::Photo, 3));
        lining.push_entry(Entry::new(Block::Store, 4));
        lining.push_child(crate::bio::profile::child_brood(
            Dir::Ahead,
            0,
            120,
            Gate::above(Scalar::AgeFrac, 77),
        ));
        lining.children[0].repeat = 8;
        lining.divide_when.energy_frac = 200;
        g.modes.push(base);
        g.modes.push(lining);
        g
    }

    /// §a.7 — a self-heating cell: PHOTO 1, STORE 1, THERMO 6 gated on
    /// `temp_dev ≥ 3 °C` (the halo switches on when the tank is cold or
    /// hot, off near 22 °C). A photosynthesiser can barely fund it — the
    /// harness "cold night" scenario feeds it glucose.
    pub fn heater() -> Self {
        let mut g = Self::base();
        g.buoy = 0;
        let mut m = Mode::blank();
        m.push_entry(Entry::new(Block::Photo, 1));
        m.push_entry(Entry::new(Block::Store, 1));
        let mut th = Entry::new(Block::Thermo, 6);
        th.gate = Gate::above(Scalar::TempDev, 24);
        m.push_entry(th);
        m.push_child(ChildSpec::propagule(0, 15));
        m.divide_when.energy_frac = 200;
        g.modes.push(m);
        g
    }

    /// Phase 4 moss: hard-walled, glued, UPTAKE(water) 1 — mats on damp sand.
    pub fn moss() -> Self {
        use crate::bio::program::{ChildSpec, Dir};
        let mut g = Self::base();
        g.buoy = 0;
        let mut m = Mode::blank();
        m.push_entry(Entry::new(Block::Photo, 2));
        m.push_entry(Entry::with_params(
            Block::Uptake,
            1,
            crate::bio::profile::UPTAKE_WATER,
            0,
        ));
        m.push_entry(Entry::with_params(Block::Wall, 2, 140, 0));
        m.push_entry(Entry::new(Block::Glue, 1));
        m.push_child(ChildSpec::propagule(0, 1));
        let mut left = ChildSpec::propagule(0, 1);
        left.dir = Dir::Left;
        m.push_child(left);
        let mut right = ChildSpec::propagule(0, 1);
        right.dir = Dir::Right;
        m.push_child(right);
        g.modes.push(m);
        g
    }

    /// Phase 4 rooting plant: seed mode transitions to a UPTAKE(water) 3
    /// root mode at `depth == 0`, so a cutting co-occupies moist sand.
    pub fn rooting() -> Self {
        use crate::bio::program::{Gate, Scalar, Transition};
        let mut g = Self::base();
        g.buoy = 0;
        let mut m = Mode::blank();
        m.push_entry(Entry::new(Block::Photo, 2));
        m.push_entry(Entry::new(Block::Store, 1));
        m.push_entry(Entry::with_params(
            Block::Uptake,
            2,
            crate::bio::profile::UPTAKE_WATER,
            0,
        ));
        m.push_entry(Entry::with_params(Block::Wall, 2, 160, 0));
        m.push_child(ChildSpec::propagule(0, 15));
        m.push_transition(Transition {
            gate: Gate::below(Scalar::Depth, 1),
            mode: 1,
        });
        let mut root = Mode::blank();
        root.push_entry(Entry::with_params(
            Block::Uptake,
            3,
            crate::bio::profile::UPTAKE_WATER,
            0,
        ));
        root.push_entry(Entry::new(Block::Chemo, 1));
        root.push_entry(Entry::with_params(Block::Wall, 1, 100, 0));
        // Grow behind (into the sand below).
        let down = ChildSpec::part(crate::bio::program::Dir::Behind, 1, 200, 2);
        root.push_child(down);
        g.modes.push(m);
        g.modes.push(root);
        g
    }

    /// Phase 4 plant: rooting base + hard stem chain + leaf pair.
    pub fn plant() -> Self {
        use crate::bio::program::{Dir, Gate, Scalar, Transition};
        let mut g = Self::rooting();
        let mut stem = Mode::blank();
        stem.push_entry(Entry::new(Block::Photo, 3));
        stem.push_entry(Entry::with_params(Block::Wall, 3, 180, 0));
        stem.push_entry(Entry::new(Block::Store, 1));
        stem.push_child(ChildSpec::part(Dir::Ahead, 2, 200, 4));
        stem.push_child(ChildSpec::part(Dir::Left, 3, 180, 1));
        stem.push_child(ChildSpec::part(Dir::Right, 3, 180, 1));
        stem.divide_when.energy_frac = 180;
        let mut leaf = Mode::blank();
        leaf.push_entry(Entry::new(Block::Photo, 4));
        leaf.push_entry(Entry::with_params(Block::Wall, 1, 120, 0));
        if let Some(seed) = g.modes.get_mut(0) {
            seed.push_transition(Transition {
                gate: Gate::above(Scalar::Depth, 0),
                mode: 2,
            });
            seed.push_child(ChildSpec::part(Dir::Ahead, 2, 200, 1));
        }
        g.modes.push(stem);
        g.modes.push(leaf);
        g
    }

    /// Phase 4 colour-crown plant: plant + pigment tip at depth ≥ 3.
    pub fn crown() -> Self {
        use crate::bio::program::{Dir, Gate, Scalar, Transition};
        let mut g = Self::plant();
        let mut crown = Mode::blank();
        crown.push_entry(Entry::with_params(Block::Pigment, 5, 40, 0));
        crown.push_entry(Entry::new(Block::Photo, 1));
        crown.push_child(ChildSpec::part(Dir::Left, 4, 150, 1));
        crown.push_child(ChildSpec::part(Dir::Right, 4, 150, 1));
        if let Some(stem) = g.modes.get_mut(2) {
            stem.push_transition(Transition {
                gate: Gate::above(Scalar::Depth, 2),
                mode: 4,
            });
        }
        g.modes.push(crown);
        g
    }

    /// d.4 — saprotroph: PHOTO 1, ENZYME 4 (key 30, width 24, litter
    /// band, contact only), UPTAKE(glucose) 2, the spare unit as STORE;
    /// divides at energy ≥ 0.6. Sinks onto the litter (`buoy` −5).
    pub fn saprotroph() -> Self {
        let mut g = Self::base();
        g.buoy = -5;
        let mut m = Mode::blank();
        m.push_entry(Entry::new(Block::Photo, 1));
        m.push_entry(Entry::with_params(Block::Enzyme, 4, 30, 24));
        m.push_entry(Entry::with_params(
            Block::Uptake,
            2,
            crate::bio::profile::UPTAKE_GLUCOSE,
            0,
        ));
        m.push_entry(Entry::new(Block::Store, 1));
        m.divide_when.energy_frac = 153;
        m.push_child(ChildSpec::propagule(0, 15));
        g.modes.push(m);
        g
    }

    /// Sediment feeder: CHEMO 3, STORE 2, the spare units as PHOTO; sinks
    /// onto the bed (`buoy` −5) and lives on sediment nutrient in the dark.
    pub fn sediment_feeder() -> Self {
        let mut g = Self::base();
        g.buoy = -5;
        let mut m = Mode::blank();
        m.push_entry(Entry::new(Block::Chemo, 3));
        m.push_entry(Entry::new(Block::Store, 2));
        m.push_entry(Entry::new(Block::Photo, 1));
        m.divide_when.energy_frac = 153;
        m.push_child(ChildSpec::propagule(0, 15));
        g.modes.push(m);
        g
    }

    /// d.7 — mouth-cavity predator: a hard-walled U. Mode 0 (base) grows a
    /// side-wall chain (mode 1) on each side; the lining secretes a
    /// living-wall enzyme (key 120, width 24) gated on touch; chains end in
    /// a lip mode (2) at depth 3. Walls are `h 0.6`, so the cavity holds
    /// its enzyme (§b.3). MUSCLE lips and the touch link arrive in 2.3.6.
    pub fn chamber_predator() -> Self {
        let mut g = Self::base();
        g.buoy = 0;
        g.wall_key = 170;
        // A 7-cell hard body needs a minute to build (pace law §b.6).
        g.base_age = 150;
        let mut base = Mode::blank();
        base.push_entry(Entry::with_params(Block::Wall, 2, 153, 0));
        base.push_entry(Entry::new(Block::Photo, 4));
        base.push_entry(Entry::new(Block::Store, 2));
        base.push_child(ChildSpec::part(Dir::Left, 1, 220, 1));
        base.push_child(ChildSpec::part(Dir::Right, 1, 220, 1));
        let mut wall = Mode::blank();
        wall.push_entry(Entry::with_params(Block::Wall, 2, 153, 0));
        wall.push_entry(Entry::new(Block::Photo, 3));
        let mut enz = Entry::with_params(Block::Enzyme, 2, 120, 24 | 0x80);
        enz.gate = Gate::above(Scalar::Touch, 1);
        wall.push_entry(enz);
        wall.push_entry(Entry::new(Block::Store, 1));
        wall.push_child(ChildSpec::part(Dir::Ahead, 1, 220, 1));
        wall.push_transition(Transition {
            gate: Gate::above(Scalar::Depth, 3),
            mode: 2,
        });
        wall.sacrifice_order = 1;
        // Lips: the body's propagules leave from the opening.
        let mut lip = Mode::blank();
        lip.push_entry(Entry::with_params(Block::Wall, 1, 153, 0));
        lip.push_entry(Entry::new(Block::Photo, 4));
        lip.push_entry(Entry::with_params(
            Block::Uptake,
            2,
            crate::bio::profile::UPTAKE_GLUCOSE,
            0,
        ));
        lip.push_child(ChildSpec::propagule(0, 15));
        lip.divide_when.energy_frac = 230;
        lip.sacrifice_order = 2;
        g.modes.push(base);
        g.modes.push(wall);
        g.modes.push(lip);
        g
    }

    /// Stinger: algae with TOXIN 2 (key 200) and NEEDLE 1 (sharpness 200),
    /// resistant to its own venom (`resist_key` 200).
    pub fn stinger() -> Self {
        let mut g = Self::base();
        g.resist_key = 200;
        g.buoy = 0;
        let mut m = Mode::blank();
        m.push_entry(Entry::new(Block::Photo, 4));
        m.push_entry(Entry::with_params(Block::Toxin, 2, 200, 0));
        m.push_entry(Entry::with_params(Block::Needle, 1, 200, 0));
        m.push_child(ChildSpec::propagule(0, 15));
        g.modes.push(m);
        g
    }

    /// Signaller: algae that emits SIGNAL 2 (key 90, source Always) and
    /// listens with RECEPTOR 1 (key 90) — kin hear each other.
    pub fn signaller() -> Self {
        let mut g = Self::base();
        g.buoy = 0;
        let mut m = Mode::blank();
        m.push_entry(Entry::new(Block::Photo, 4));
        m.push_entry(Entry::with_params(Block::Signal, 2, 90, Scalar::Always as u8));
        m.push_entry(Entry::with_params(Block::Receptor, 1, 90, 0));
        m.push_child(ChildSpec::propagule(0, 15));
        g.modes.push(m);
        g
    }

    /// Bone spine: PHOTO 4, STORE 2; lays three BONE pixels ahead
    /// (`1 g + 2 m` each, §a.4 S1) before releasing propagules.
    pub fn spiny() -> Self {
        let mut g = Self::base();
        g.buoy = 0;
        let mut m = Mode::blank();
        m.push_entry(Entry::new(Block::Photo, 4));
        m.push_entry(Entry::new(Block::Store, 2));
        m.push_child(ChildSpec {
            dir: Dir::Ahead,
            mode: 0,
            kind: ChildKind::SecreteBone,
            bond: 255,
            repeat: 3,
            detach: Detach::Never,
        });
        m.push_child(ChildSpec::propagule(0, 15));
        g.modes.push(m);
        g
    }

    /// d.8-lite egg layer: the base lays a brood daughter ahead, wraps her
    /// in SHELL (`0.8 g + 3 m` per pixel, §a.4 S2) and releases her when
    /// her `age_frac ≥ 0.3`; the hatchling (mode 1) then grows a 2-cell
    /// body, cracking the shell. Mode 1 = PHOTO 4, STORE 1, one part Ahead.
    pub fn egg_layer() -> Self {
        let mut g = Self::base();
        g.buoy = 0;
        let mut base = Mode::blank();
        base.push_entry(Entry::new(Block::Photo, 4));
        base.push_entry(Entry::new(Block::Store, 4));
        base.push_child(crate::bio::profile::child_brood(
            Dir::Ahead,
            1,
            120,
            Gate::above(Scalar::AgeFrac, 77),
        ));
        base.push_child(ChildSpec {
            dir: Dir::Ahead,
            mode: 0,
            kind: ChildKind::SecreteShell,
            bond: 255,
            repeat: 3,
            detach: Detach::Never,
        });
        base.divide_when.energy_frac = 200;
        let mut hatch = Mode::blank();
        hatch.push_entry(Entry::new(Block::Photo, 4));
        hatch.push_entry(Entry::new(Block::Store, 1));
        hatch.push_child(ChildSpec::part(Dir::Ahead, 1, 200, 1));
        hatch.divide_when.energy_frac = 100;
        g.modes.push(base);
        g.modes.push(hatch);
        g
    }

    pub fn n_modes(&self) -> u8 {
        self.modes.len() as u8
    }

    pub fn mode(&self, i: u8) -> &Mode {
        let i = (i as usize).min(self.modes.len().saturating_sub(1));
        &self.modes[i]
    }

    pub fn photo(&self) -> u8 {
        self.mode(self.seed_mode).units_of(Block::Photo)
    }

    /// Temperature efficiency curve (§a.2): `exp(−((T−opt)/width)²)` in Q8.
    pub fn temp_eff_q8(&self, t_c: f32) -> u8 {
        let w = self.temp_width.max(1) as f32;
        let d = (t_c - self.temp_opt as f32) / w;
        let e = (-(d * d)).exp();
        (e * 255.0).clamp(0.0, 255.0) as u8
    }

    /// λ per division in milli (§c.2):
    /// `λ0 · (1 + 2·cheap + fast − 1.5·accurate) · (1 + k_s·stress) · q10(T)`.
    pub fn lambda_milli(&self, stress: u8, t_c: f32) -> u32 {
        let strat = 1000 + 2 * self.cheap as u32 * 1000 / 255 + self.fast as u32 * 1000 / 255;
        let strat = strat.saturating_sub(3 * self.accurate as u32 * 1000 / (2 * 255));
        let stress_m = 1000 + (stress as u32) * 1000 / 255;
        let q10 = 2f32.powf((t_c - 22.0) / 10.0).clamp(0.25, 4.0);
        let q_m = (q10 * 1000.0) as u32;
        let l = LAMBDA0_MILLI as u64 * strat as u64 * stress_m as u64 * q_m as u64
            / 1_000_000_000;
        (l as u32).clamp(5, 2500)
    }

    /// Division cost multiplier from the pick-two strategy, Q8 (256 = ×1).
    pub fn division_cost_q8(&self) -> u32 {
        256 * (200 + self.accurate as u32) / (200 + self.cheap as u32)
    }

    /// Division wait in ticks from strategy (15–150 ticks, handoff §5).
    pub fn division_wait_ticks(&self) -> u16 {
        (150 - (self.fast as u32 * 135 / 255)) as u16
    }

    /// Whether every child/transition target is a valid mode and the seed
    /// mode can grow something.
    pub fn is_valid(&self) -> bool {
        let n = self.modes.len();
        (1..=MAX_MODES).contains(&n)
            && (self.seed_mode as usize) < n
            && self.metab_eff >= 64
            && self.metab_eff <= 192
            && self.temp_opt <= 40
            && self.temp_width >= 1
            && self.wall_key >= WALL_KEY_MIN
            && self.wall_key <= WALL_KEY_MAX
            && self.modes.iter().all(|m| m.is_valid(n as u8))
    }

    /// Apply one weighted operator (§c.2). Always returns a valid genome.
    pub fn mutate_once(&self, seed: u64, salt: u64) -> Self {
        let mut next = self.clone();
        let roll = hash_u64(seed, salt, 0, 0xC2_00) % 1000;
        let n_modes = next.modes.len() as u8;
        let mi = (hash_u64(seed, salt, 1, 0xC2_01) as usize) % next.modes.len();
        match roll {
            // point 0.55 — header or mode scalar
            0..=549 => {
                if hash_u64(seed, salt, 2, 0xC2_02) % 3 == 0 {
                    next.mutate_header_point(seed, salt);
                } else {
                    next.modes[mi].mutate_point(seed, salt);
                }
            }
            // shift units 0.10
            550..=649 => next.modes[mi].shift_units(seed, salt),
            // gain block 0.06
            650..=709 => next.modes[mi].gain_block(seed, salt),
            // loss block 0.04
            710..=749 => next.modes[mi].lose_block(seed, salt),
            // re-parameterise 0.08
            750..=829 => next.modes[mi].reparameterise(seed, salt),
            // re-wire child 0.05
            830..=879 => {
                let m = &mut next.modes[mi];
                if m.n_children > 0 {
                    let ci = (hash_u64(seed, salt, 3, 0xC2_03) as usize) % m.n_children as usize;
                    rewire_child(&mut m.children[ci], n_modes, seed, salt);
                } else {
                    let target = (hash_u64(seed, salt, 4, 0xC2_04) % n_modes as u64) as u8;
                    m.push_child(ChildSpec::part(Dir::Ahead, target, 150, 1));
                }
            }
            // re-wire transition 0.03
            880..=909 => {
                let m = &mut next.modes[mi];
                if m.n_transitions > 0 && hash_u64(seed, salt, 5, 0xC2_05) % 4 != 0 {
                    let ti =
                        (hash_u64(seed, salt, 6, 0xC2_06) as usize) % m.n_transitions as usize;
                    rewire_transition(&mut m.transitions[ti], n_modes, seed, salt);
                } else if (m.n_transitions as usize) < crate::consts::MAX_TRANSITIONS {
                    let mut gate = Gate::above(Scalar::Depth, 2);
                    gate.mutate(seed, salt);
                    if gate.scalar == Scalar::Always {
                        gate.scalar = Scalar::AgeFrac;
                    }
                    let target = (hash_u64(seed, salt, 7, 0xC2_07) % n_modes as u64) as u8;
                    m.push_transition(Transition { gate, mode: target });
                } else {
                    m.n_transitions -= 1;
                }
            }
            // duplicate mode 0.03
            910..=939 => {
                if next.modes.len() < MAX_MODES {
                    let copy = next.modes[mi];
                    next.modes.push(copy);
                }
            }
            // delete mode 0.02
            940..=959 => next.delete_mode(mi),
            // split mode 0.015
            960..=974 => next.split_mode(mi),
            // merge modes 0.01
            975..=984 => next.merge_modes(),
            // change child kind 0.005
            985..=989 => {
                let m = &mut next.modes[mi];
                if m.n_children > 0 {
                    let ci = (hash_u64(seed, salt, 8, 0xC2_08) as usize) % m.n_children as usize;
                    m.children[ci].kind =
                        ChildKind::from_u8(hash_u64(seed, salt, 9, 0xC2_09) as u8);
                }
            }
            // link op 0.01 — intra-cell links (§a.6); inter-cell links Phase 5
            _ => next.modes[mi].mutate_link(seed, salt),
        }
        next.repair();
        next
    }

    /// Poisson-ish count of operators from λ (cap 3), then apply.
    pub fn mutate(&self, seed: u64, salt: u64) -> Self {
        self.mutate_once(seed, salt)
    }

    fn mutate_header_point(&mut self, seed: u64, salt: u64) {
        let which = hash_u64(seed, salt, 10, 0xC2_10) % 12;
        let d8 = (hash_u64(seed, salt, 11, 0xC2_11) % 17) as i32 - 8;
        let d3 = (hash_u64(seed, salt, 12, 0xC2_12) % 7) as i32 - 3;
        match which {
            0 => self.metab_eff = (self.metab_eff as i32 + d8).clamp(64, 192) as u8,
            1 => self.buoy = (self.buoy as i32 + d8 * 2).clamp(-100, 100) as i8,
            2 => self.temp_opt = (self.temp_opt as i32 + d3).clamp(0, 40) as u8,
            3 => self.temp_width = (self.temp_width as i32 + d3).clamp(2, 20) as u8,
            4 => self.base_age = (self.base_age as i32 + d8).clamp(20, 255) as u8,
            5 => {
                self.sacrifice_threshold =
                    (self.sacrifice_threshold as i32 + d8).clamp(0, 255) as u8
            }
            6 => {
                let which = hash_u64(seed, salt, 13, 0xC2_13) % 3;
                let d = d8 * 2;
                match which {
                    0 => self.cheap = (self.cheap as i32 + d).clamp(0, 255) as u8,
                    1 => self.fast = (self.fast as i32 + d).clamp(0, 255) as u8,
                    _ => self.accurate = (self.accurate as i32 + d).clamp(0, 255) as u8,
                }
            }
            7 => {
                let which = hash_u64(seed, salt, 14, 0xC2_14) % 8;
                let s = &mut self.stress;
                let f = match which {
                    0 => &mut s.hunger,
                    1 => &mut s.heat,
                    2 => &mut s.cold,
                    3 => &mut s.damage,
                    4 => &mut s.crowding,
                    5 => &mut s.waste,
                    6 => &mut s.toxin,
                    _ => &mut s.dark,
                };
                *f = (*f as i32 + d8).clamp(0, 255) as u8;
            }
            8 => {
                self.wall_key = (self.wall_key as i32 + d3)
                    .clamp(WALL_KEY_MIN as i32, WALL_KEY_MAX as i32) as u8
            }
            9 => self.glue_key = (self.glue_key as i32 + d3).rem_euclid(256) as u8,
            10 => self.glue_width = (self.glue_width as i32 + d3).clamp(1, 64) as u8,
            _ => self.resist_key = (self.resist_key as i32 + d3).rem_euclid(256) as u8,
        }
    }

    /// Delete a mode; references splice to its first child mode or drop.
    fn delete_mode(&mut self, mi: usize) {
        if self.modes.len() <= 1 || mi == self.seed_mode as usize {
            return;
        }
        let removed = self.modes.remove(mi);
        let splice = (0..removed.n_children as usize)
            .map(|i| removed.children[i].mode)
            .find(|&m| m as usize != mi);
        let remap = |m: u8| -> Option<u8> {
            match (m as usize).cmp(&mi) {
                std::cmp::Ordering::Equal => {
                    splice.map(|s| if (s as usize) > mi { s - 1 } else { s })
                }
                std::cmp::Ordering::Greater => Some(m - 1),
                std::cmp::Ordering::Less => Some(m),
            }
        };
        if (self.seed_mode as usize) > mi {
            self.seed_mode -= 1;
        }
        for m in &mut self.modes {
            let mut i = 0;
            while i < m.n_children as usize {
                match remap(m.children[i].mode) {
                    Some(t) => {
                        m.children[i].mode = t;
                        i += 1;
                    }
                    None => {
                        for j in i..m.n_children as usize - 1 {
                            m.children[j] = m.children[j + 1];
                        }
                        m.n_children -= 1;
                    }
                }
            }
            let mut i = 0;
            while i < m.n_transitions as usize {
                match remap(m.transitions[i].mode) {
                    Some(t) => {
                        m.transitions[i].mode = t;
                        i += 1;
                    }
                    None => {
                        for j in i..m.n_transitions as usize - 1 {
                            m.transitions[j] = m.transitions[j + 1];
                        }
                        m.n_transitions -= 1;
                    }
                }
            }
        }
    }

    /// Split: copy the mode; move the second half of its children to the
    /// copy; the original's last child becomes "grow the copy".
    fn split_mode(&mut self, mi: usize) {
        if self.modes.len() >= MAX_MODES || self.modes[mi].n_children < 2 {
            return;
        }
        let new_idx = self.modes.len() as u8;
        let mut copy = self.modes[mi];
        let n = self.modes[mi].n_children as usize;
        let half = n / 2;
        copy.n_children = 0;
        for i in half..n {
            copy.children[i - half] = self.modes[mi].children[i];
            copy.n_children += 1;
        }
        for i in copy.n_children as usize..crate::consts::MAX_CHILDREN {
            copy.children[i] = ChildSpec::empty();
        }
        let orig = &mut self.modes[mi];
        orig.n_children = half as u8;
        orig.children[half] = ChildSpec::part(Dir::Ahead, new_idx, 200, 1);
        orig.n_children += 1;
        for i in orig.n_children as usize..crate::consts::MAX_CHILDREN {
            orig.children[i] = ChildSpec::empty();
        }
        self.modes.push(copy);
    }

    /// Merge two modes with identical profiles into one.
    fn merge_modes(&mut self) {
        let n = self.modes.len();
        for a in 0..n {
            for b in (a + 1)..n {
                let same = self.modes[a].n_entries == self.modes[b].n_entries
                    && (0..self.modes[a].n_entries as usize)
                        .all(|i| self.modes[a].entries[i] == self.modes[b].entries[i]);
                if same && b != self.seed_mode as usize {
                    // Redirect b → a, then delete b.
                    for m in &mut self.modes {
                        for i in 0..m.n_children as usize {
                            if m.children[i].mode as usize == b {
                                m.children[i].mode = a as u8;
                            }
                        }
                        for i in 0..m.n_transitions as usize {
                            if m.transitions[i].mode as usize == b {
                                m.transitions[i].mode = a as u8;
                            }
                        }
                    }
                    self.delete_mode(b);
                    return;
                }
            }
        }
    }

    /// Enforce the §c.2 constraints after any operator.
    fn repair(&mut self) {
        if self.modes.is_empty() {
            self.modes.push(Mode::algae());
        }
        self.modes.truncate(MAX_MODES);
        let n = self.modes.len() as u8;
        if self.seed_mode >= n {
            self.seed_mode = 0;
        }
        self.wall_key = self.wall_key.clamp(WALL_KEY_MIN, WALL_KEY_MAX);
        self.temp_width = self.temp_width.max(1);
        for m in &mut self.modes {
            m.shape_w = m.shape_w.clamp(1, crate::consts::SHAPE_CAP);
            m.shape_h = m.shape_h.clamp(1, crate::consts::SHAPE_CAP);
            m.compact();
            m.clamp_to_capacity();
            for i in 0..m.n_children as usize {
                if m.children[i].mode >= n {
                    m.children[i].mode %= n;
                }
            }
            for i in 0..m.n_transitions as usize {
                if m.transitions[i].mode >= n {
                    m.transitions[i].mode %= n;
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct GenotypeNode {
    pub genome: Genome,
    pub coeffs: Vec<ModeCoeffs>,
    pub parent: u32,
    pub first_child: u32,
    pub next_sibling: u32,
    pub live_cells: u32,
    pub born: u32,
    pub folded_nodes: u32,
    /// Mutation events since the root (§c.1 `depth`).
    pub depth: u16,
    pub tombstone: bool,
    /// Species cluster (`species.rs`), `NO_SPECIES` for tombstones.
    pub species: u16,
}

pub const NONE: u32 = u32::MAX;

#[derive(Debug, Clone)]
pub struct GenomeStore {
    nodes: Vec<GenotypeNode>,
    by_genome: HashMap<Genome, u32>,
    live_nodes: usize,
    /// Species table (§c.4); entries are never removed.
    species: Vec<SpeciesRec>,
    /// Species founded since the last `drain_new_species`.
    new_species: Vec<u16>,
    /// Current tick, for `SpeciesRec::first_tick`.
    now: u64,
}

impl GenomeStore {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            by_genome: HashMap::new(),
            live_nodes: 0,
            species: Vec::new(),
            new_species: Vec::new(),
            now: 0,
        }
    }

    pub fn set_tick(&mut self, tick: u64) {
        self.now = tick;
    }

    pub fn species(&self, id: u16) -> Option<&SpeciesRec> {
        self.species.get(id as usize)
    }

    pub fn species_mut(&mut self, id: u16) -> Option<&mut SpeciesRec> {
        self.species.get_mut(id as usize)
    }

    pub fn species_count(&self) -> usize {
        self.species.len()
    }

    pub fn species_table(&self) -> &[SpeciesRec] {
        &self.species
    }

    /// Species of a genotype (`NO_SPECIES` for unknown ids / tombstones).
    pub fn species_of(&self, genotype: u32) -> u16 {
        self.nodes.get(genotype as usize).map(|n| n.species).unwrap_or(NO_SPECIES)
    }

    /// Species founded since the last call (ids, oldest first).
    pub fn drain_new_species(&mut self) -> Vec<u16> {
        std::mem::take(&mut self.new_species)
    }

    /// Live cells across all genotypes of a species.
    pub fn species_live_cells(&self, species: u16) -> u32 {
        self.nodes
            .iter()
            .filter(|n| !n.tombstone && n.species == species)
            .map(|n| n.live_cells)
            .sum()
    }

    /// Living genotypes of a species.
    pub fn species_live_genotypes(&self, species: u16) -> u32 {
        self.nodes
            .iter()
            .filter(|n| !n.tombstone && n.species == species && n.live_cells > 0)
            .count() as u32
    }

    /// Assign a species to a fresh node (§c.4): the parent's species while
    /// the structural signature matches and the parametric distance to
    /// the founder stays within `D`, else a new species under it.
    fn assign_species(&mut self, id: u32, parent: u32) -> u16 {
        let genome = self.nodes[id as usize].genome.clone();
        let sig = species::signature(&genome);
        let parent_species = self.nodes.get(parent as usize).map(|p| p.species).unwrap_or(NO_SPECIES);
        if let Some(ps) = self.species.get(parent_species as usize) {
            if ps.sig == sig && species::param_distance_q8(&genome, &ps.founder_genome) <= species::SPECIES_D_Q8 {
                return parent_species;
            }
        }
        let (depth, parent_genome) = match self.species.get(parent_species as usize) {
            Some(ps) => (ps.depth.saturating_add(1), Some(ps.founder_genome.clone())),
            None => (1, None),
        };
        let base = species::species_name(&genome, parent_genome.as_ref(), depth);
        let same = self
            .species
            .iter()
            .filter(|s| s.parent == parent_species && s.name.starts_with(&base))
            .count() as u32;
        let name = species::disambiguate(&base, same + 1);
        let sid = self.species.len() as u16;
        if sid == NO_SPECIES {
            return parent_species;
        }
        self.species.push(SpeciesRec {
            founder: id,
            founder_genome: genome,
            parent: parent_species,
            depth,
            sig,
            name,
            first_tick: self.now,
            archived: false,
        });
        self.new_species.push(sid);
        sid
    }

    pub fn len(&self) -> usize {
        self.live_nodes
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    pub fn get(&self, id: u32) -> Option<&GenotypeNode> {
        self.nodes.get(id as usize)
    }

    /// The lineage founder: the root of `id`'s parent chain (a genotype
    /// the player placed).
    pub fn root_of(&self, id: u32) -> u32 {
        let mut cur = id;
        let mut hops = 0;
        while let Some(n) = self.nodes.get(cur as usize) {
            if n.parent == NONE || hops > self.nodes.len() {
                break;
            }
            cur = n.parent;
            hops += 1;
        }
        cur
    }

    /// Live cells in `root` and every genotype descended from it.
    pub fn lineage_live_cells(&self, root: u32) -> u32 {
        let mut total = 0;
        for n in &self.nodes {
            if n.live_cells == 0 || n.tombstone {
                continue;
            }
            let mut cur = n;
            let mut hops = 0;
            loop {
                if std::ptr::eq(cur, &self.nodes[root as usize]) {
                    total += n.live_cells;
                    break;
                }
                if cur.parent == NONE || hops > self.nodes.len() {
                    break;
                }
                cur = &self.nodes[cur.parent as usize];
                hops += 1;
            }
        }
        total
    }

    pub fn genome(&self, id: u32) -> &Genome {
        &self.nodes[(id as usize).min(self.nodes.len() - 1)].genome
    }

    pub fn coeffs(&self, id: u32, mode: u8) -> &ModeCoeffs {
        let n = &self.nodes[(id as usize).min(self.nodes.len() - 1)];
        let mi = (mode as usize).min(n.coeffs.len() - 1);
        &n.coeffs[mi]
    }

    fn push_node(&mut self, genome: Genome, parent: u32, depth: u16) -> u32 {
        let id = self.nodes.len() as u32;
        let coeffs = genome
            .modes
            .iter()
            .map(|m| ModeCoeffs::compile(m, genome.metab_eff))
            .collect();
        let next_sib = self
            .nodes
            .get(parent as usize)
            .map(|p| p.first_child)
            .unwrap_or(NONE);
        self.by_genome.insert(genome.clone(), id);
        self.nodes.push(GenotypeNode {
            genome,
            coeffs,
            parent,
            first_child: NONE,
            next_sibling: next_sib,
            live_cells: 0,
            born: 0,
            folded_nodes: 0,
            depth,
            tombstone: false,
            species: NO_SPECIES,
        });
        if let Some(p) = self.nodes.get_mut(parent as usize) {
            p.first_child = id;
        }
        self.live_nodes += 1;
        let sp = self.assign_species(id, parent);
        self.nodes[id as usize].species = sp;
        id
    }

    /// Intern the seed algae as root genotype 0.
    pub fn seed_algae(&mut self) -> u32 {
        if self.nodes.is_empty() {
            self.push_node(Genome::algae(), NONE, 0);
        }
        0
    }

    /// Intern a root-level genome (test/archive seeds) under the algae root.
    pub fn intern_root(&mut self, genome: Genome) -> u32 {
        let root = self.seed_algae();
        self.intern_child(root, genome)
    }

    /// Intern a genome seeded from the archive as a true lineage root
    /// (no parent genotype) whose species continues the archived one:
    /// same generated name and depth, flagged archived (§c.4 "seeding
    /// from the archive continues the tree across tanks"). A genome
    /// already in the table keeps its node.
    pub fn intern_archive_root(&mut self, genome: Genome, species_name: &str, species_depth: u16) -> u32 {
        if let Some(&id) = self.by_genome.get(&genome) {
            return id;
        }
        // Make sure the algae root exists first so ids stay stable.
        self.seed_algae();
        let id = self.push_node(genome.clone(), NONE, 0);
        // `push_node` founded a fresh depth-1 species; rename it to the
        // archived identity (its own founder genome and signature stand).
        let sp = self.nodes[id as usize].species;
        if let Some(s) = self.species.get_mut(sp as usize) {
            if s.founder == id {
                let base = species_name.to_string();
                let same = self
                    .species
                    .iter()
                    .filter(|o| o.founder != id && o.name == base)
                    .count() as u32;
                let s = &mut self.species[sp as usize];
                s.name = if same == 0 { base } else { species::disambiguate(&base, same + 1) };
                s.depth = species_depth.max(1);
                s.archived = true;
            }
        }
        id
    }

    pub fn intern_child(&mut self, parent: u32, genome: Genome) -> u32 {
        if let Some(&id) = self.by_genome.get(&genome) {
            return id;
        }
        let depth = self
            .nodes
            .get(parent as usize)
            .map(|p| p.depth.saturating_add(1))
            .unwrap_or(0);
        self.push_node(genome, parent, depth)
    }

    pub fn on_birth(&mut self, id: u32) {
        if let Some(n) = self.nodes.get_mut(id as usize) {
            n.live_cells = n.live_cells.saturating_add(1);
            n.born = n.born.saturating_add(1);
        }
    }

    pub fn on_death(&mut self, id: u32) {
        let Some(n) = self.nodes.get_mut(id as usize) else {
            return;
        };
        n.live_cells = n.live_cells.saturating_sub(1);
        if n.live_cells > 0 || n.tombstone {
            return;
        }
        self.fold_cascade(id);
    }

    /// Fold an extinct leaf into its parent; repeat while the parent is
    /// itself extinct and now childless.
    fn fold_cascade(&mut self, mut id: u32) {
        loop {
            let Some(n) = self.nodes.get(id as usize) else {
                return;
            };
            if n.tombstone || n.live_cells > 0 || n.first_child != NONE || n.parent == NONE {
                return;
            }
            let parent = n.parent;
            self.fold_leaf(id, parent);
            id = parent;
        }
    }

    fn fold_leaf(&mut self, id: u32, parent: u32) {
        let (next_sibling, born, folded, genome) = {
            let Some(n) = self.nodes.get(id as usize) else {
                return;
            };
            (n.next_sibling, n.born, n.folded_nodes, n.genome.clone())
        };
        if let Some(p) = self.nodes.get_mut(parent as usize) {
            p.folded_nodes = p.folded_nodes.saturating_add(1 + folded);
            p.born = p.born.saturating_add(born);
            if p.first_child == id {
                p.first_child = next_sibling;
            } else {
                let mut c = p.first_child;
                while c != NONE {
                    let next = self.nodes.get(c as usize).map(|n| n.next_sibling);
                    if next == Some(id) {
                        if let Some(node) = self.nodes.get_mut(c as usize) {
                            node.next_sibling = next_sibling;
                        }
                        break;
                    }
                    c = next.unwrap_or(NONE);
                }
            }
        }
        self.by_genome.remove(&genome);
        if let Some(n) = self.nodes.get_mut(id as usize) {
            n.tombstone = true;
            n.live_cells = 0;
            self.live_nodes = self.live_nodes.saturating_sub(1);
        }
    }

    pub fn live_total(&self) -> u32 {
        self.nodes.iter().map(|n| n.live_cells).sum()
    }

    /// Serialisable view: every non-tombstone node in id order.
    pub fn export(&self) -> Vec<(u32, u32, u16, Genome)> {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| !n.tombstone)
            .map(|(i, n)| (i as u32, n.parent, n.depth, n.genome.clone()))
            .collect()
    }

    /// Rebuild from an export; ids are preserved (gaps become tombstones).
    pub fn import(rows: &[(u32, u32, u16, Genome)]) -> Self {
        let mut s = Self::new();
        let max_id = rows.iter().map(|r| r.0).max().map(|m| m as usize + 1).unwrap_or(0);
        for i in 0..max_id {
            let tomb = GenotypeNode {
                genome: Genome::algae(),
                coeffs: Vec::new(),
                parent: NONE,
                first_child: NONE,
                next_sibling: NONE,
                live_cells: 0,
                born: 0,
                folded_nodes: 0,
                depth: 0,
                tombstone: true,
                species: NO_SPECIES,
            };
            let _ = i;
            s.nodes.push(tomb);
        }
        for (id, parent, depth, genome) in rows {
            let i = *id as usize;
            let coeffs = genome
                .modes
                .iter()
                .map(|m| ModeCoeffs::compile(m, genome.metab_eff))
                .collect();
            s.by_genome.insert(genome.clone(), *id);
            s.nodes[i] = GenotypeNode {
                genome: genome.clone(),
                coeffs,
                parent: *parent,
                first_child: NONE,
                next_sibling: NONE,
                live_cells: 0,
                born: 0,
                folded_nodes: 0,
                depth: *depth,
                tombstone: false,
                species: NO_SPECIES,
            };
            s.live_nodes += 1;
        }
        // Rebuild child lists.
        for i in (0..s.nodes.len()).rev() {
            if s.nodes[i].tombstone {
                continue;
            }
            let p = s.nodes[i].parent;
            if p != NONE && (p as usize) < s.nodes.len() {
                let first = s.nodes[p as usize].first_child;
                s.nodes[i].next_sibling = first;
                s.nodes[p as usize].first_child = i as u32;
            }
        }
        if s.nodes.is_empty() {
            s.seed_algae();
        }
        s
    }

    /// Restore the species layer from a save, or rebuild it (ids are
    /// creation-ordered, so parents come before children).
    pub fn import_species(&mut self, table: Vec<SpeciesRec>, node_species: &[(u32, u16)]) {
        self.species = table;
        for (id, sp) in node_species {
            if let Some(n) = self.nodes.get_mut(*id as usize) {
                n.species = *sp;
            }
        }
        for id in 0..self.nodes.len() as u32 {
            let (tomb, sp, parent) = {
                let n = &self.nodes[id as usize];
                (n.tombstone, n.species, n.parent)
            };
            if tomb || (sp != NO_SPECIES && (sp as usize) < self.species.len()) {
                continue;
            }
            let sp = self.assign_species(id, parent);
            self.nodes[id as usize].species = sp;
        }
        self.new_species.clear();
    }

    /// `(genotype, species)` for every non-tombstone node.
    pub fn export_species(&self) -> Vec<(u32, u16)> {
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| !n.tombstone)
            .map(|(i, n)| (i as u32, n.species))
            .collect()
    }
}

impl Default for GenomeStore {
    fn default() -> Self {
        Self::new()
    }
}
