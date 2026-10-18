//! Phase 3 — the archive's core half (`roadmap.md` Phase 3, `cell-language.md`
//! §c.4): a `Specimen` record the shell stores and shares, a deterministic
//! pixel portrait grown from the genome alone, and the species tree the
//! book's tree-of-life view draws. Storage, pages and the share sheet are
//! the shell's.

use serde::{Deserialize, Serialize};

use crate::bio::profile::ModeCoeffs;
use crate::bio::store::{BONE_OWNER, OWNER_BIAS, SHELL_OWNER};
use crate::bio::{Genome, NO_SPECIES};
use crate::cell::Cell;
use crate::hash::hash_bytes;
use crate::tank::{Tank, TankInput};

pub const SPECIMEN_VERSION: u32 = 1;

/// Ticks a portrait sandbox runs at most; growth usually settles earlier.
pub const PORTRAIT_TICKS: u32 = 900;
/// Sandbox side in physics cells (water everywhere, always day).
const PORTRAIT_CELLS: u32 = 20;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpecimenStats {
    pub cells: u32,
    pub slots: u32,
    pub age_ticks: u32,
    pub max_age_ticks: u32,
    pub children: u32,
    pub kin_alive: u32,
    pub descendants_alive: u32,
}

/// One saved creature. Pixel-exact portrait at 1× (the shell scales it
/// 8×); `display_name` is the player's, `species_name` the generated one
/// that stays as the subtitle (§c.4).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Specimen {
    pub version: u32,
    /// `hash(genome, tank seed, tick)` — stable across devices.
    pub id: u64,
    pub genome: Genome,
    pub species_name: String,
    pub display_name: String,
    /// Species names from the root to this one.
    pub lineage: Vec<String>,
    pub species_depth: u16,
    pub tank_seed: u64,
    pub tick: u64,
    pub generation: u32,
    pub mutations_since_seed: u16,
    pub stats: SpecimenStats,
    /// Seed-mode colour.
    pub colour: [u8; 3],
    pub notes: String,
    pub portrait_w: u16,
    pub portrait_h: u16,
    /// RGBA8, row-major, alpha 0 outside the body.
    pub portrait: Vec<u8>,
    /// Specimen this creature's tank was seeded from, when its lineage
    /// root came out of the archive (cross-tank ancestry).
    pub archive_parent: Option<u64>,
}

impl Specimen {
    pub fn to_bytes(&self) -> Vec<u8> {
        postcard::to_allocvec(self).unwrap_or_default()
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let s: Specimen = postcard::from_bytes(bytes).ok()?;
        (s.version == SPECIMEN_VERSION && s.genome.is_valid()).then_some(s)
    }

    /// The archive page as text (export, §2.2 #7).
    pub fn page_text(&self) -> String {
        let mut s = String::new();
        s.push_str(&self.display_name);
        if self.display_name != self.species_name {
            s.push_str(" (");
            s.push_str(&self.species_name);
            s.push(')');
        }
        s.push('\n');
        s.push_str(&format!(
            "generation {} · {} mutations since seed · species depth {}\n",
            self.generation, self.mutations_since_seed, self.species_depth
        ));
        if !self.lineage.is_empty() {
            s.push_str("lineage: ");
            s.push_str(&self.lineage.join(" → "));
            s.push('\n');
        }
        s.push_str(&format!(
            "{} cells in {} slots · age {}/{} s · {} children · kin alive {} · descendants alive {}\n",
            self.stats.cells,
            self.stats.slots,
            self.stats.age_ticks / crate::consts::TICK_HZ as u32,
            self.stats.max_age_ticks / crate::consts::TICK_HZ as u32,
            self.stats.children,
            self.stats.kin_alive,
            self.stats.descendants_alive
        ));
        s.push_str(&format!("tank {:016x} · tick {}\n", self.tank_seed, self.tick));
        if !self.notes.is_empty() {
            s.push('\n');
            s.push_str(&self.notes);
            s.push('\n');
        }
        s
    }
}

/// One node of the species tree for the tree-of-life view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeciesNode {
    pub id: u16,
    /// `NO_SPECIES` for a root.
    pub parent: u16,
    pub depth: u16,
    pub name: String,
    pub colour: [u8; 3],
    pub live_cells: u32,
    pub live_genotypes: u32,
    /// Genotypes the species has ever had (live + folded) — the "N forms"
    /// label on a collapsed edge.
    pub forms: u32,
    /// Organisms ever born with a genotype of this species (edge thickness).
    pub born: u32,
    pub first_tick: u64,
    pub archived: bool,
    pub extinct: bool,
}

/// Grow `genome` alone in a still, lit sandbox and draw what it builds:
/// a pixel-exact RGBA portrait of the body (alpha 0 around it). Pure
/// function of the genome — the same bytes give the same picture on any
/// device (`cell-language.md` §f).
pub fn portrait(genome: &Genome) -> (u16, u16, Vec<u8>) {
    let mut tank = match Tank::try_new(0x5EED_F00D, PORTRAIT_CELLS, PORTRAIT_CELLS) {
        Ok(t) => t,
        Err(_) => return (1, 1, vec![0, 0, 0, 0]),
    };
    for vx in 0..PORTRAIT_CELLS as i32 {
        for vy in 0..PORTRAIT_CELLS as i32 {
            tank.place(vx, vy, Cell::water());
        }
    }
    tank.set_lambda_scale_q8(0);
    tank.set_minerals_all(255);
    let centre = (PORTRAIT_CELLS as u16 * crate::consts::PX_PER_CELL as u16) / 2;
    let Some(seed) = tank.seed_genome_at(centre, centre, genome.clone()) else {
        return (1, 1, vec![0, 0, 0, 0]);
    };
    let org = tank.cell(seed).map(|c| c.org).unwrap_or(0);
    let input = TankInput {
        always_day: true,
        ..TankInput::default()
    };
    let mut last_shape = 0usize;
    let mut still = 0u32;
    for _ in 0..PORTRAIT_TICKS {
        // Keep every cell of the body fed so the portrait is the full
        // build, not a starving one; the pose is what the program makes.
        tank.feed_org_full(org);
        tank.tick(&input);
        let Some(o) = tank.cells().org(org) else {
            break;
        };
        let shape = o.n_alive as usize + o.secretions.len();
        if shape == last_shape {
            still += 1;
            if still > 90 {
                break;
            }
        } else {
            still = 0;
            last_shape = shape;
        }
    }
    let Some(bb) = tank.org_bbox(org) else {
        return (1, 1, vec![0, 0, 0, 0]);
    };
    let w = (bb.2 - bb.0 + 1) as usize;
    let h = (bb.3 - bb.1 + 1) as usize;
    let mut out = vec![0u8; w * h * 4];
    let cells = tank.cells();
    let genomes = tank.genomes();
    let own_secretion = |px: u16, py: u16| {
        cells
            .org(org)
            .is_some_and(|o| o.secretions.iter().any(|s| s.px == px && s.py == py))
    };
    for y in 0..h {
        for x in 0..w {
            let px = bb.0 as usize + x;
            let py = bb.1 as usize + y;
            let owner = tank.owner_at(px as u16, py as u16);
            let rgb = match owner {
                BONE_OWNER if own_secretion(px as u16, py as u16) => Some([0xEF, 0xE7, 0xDA]),
                SHELL_OWNER if own_secretion(px as u16, py as u16) => Some([0x9F, 0xB3, 0xC8]),
                o if (OWNER_BIAS..SHELL_OWNER).contains(&o) => cells
                    .get(o - OWNER_BIAS)
                    .filter(|c| c.org == org)
                    .map(|c| genomes.coeffs(c.genotype, c.mode).colour),
                _ => None,
            };
            if let Some(rgb) = rgb {
                let i = (y * w + x) * 4;
                out[i..i + 3].copy_from_slice(&rgb);
                out[i + 3] = 255;
            }
        }
    }
    let pink = crate::bio::Block::Cilia.colour(0);
    for (_, c) in cells.iter_alive() {
        if c.org != org || genomes.coeffs(c.genotype, c.mode).cilia == 0 {
            continue;
        }
        // The trail is this creature's, even when a daughter in the
        // sandbox happens to be sitting on it. Skip only pixels we
        // already painted as this body's own tissue.
        let mut paint = |x: i32, y: i32| {
            if x < bb.0 as i32 || y < bb.1 as i32 || x > bb.2 as i32 || y > bb.3 as i32 {
                return;
            }
            let lx = (x - bb.0 as i32) as usize;
            let ly = (y - bb.1 as i32) as usize;
            let i = (ly * w + lx) * 4;
            if out[i + 3] != 0 {
                return;
            }
            out[i..i + 3].copy_from_slice(&pink);
            out[i + 3] = 255;
        };
        let (px, py) = (c.px as i32, c.py as i32);
        crate::bio::step::for_cilia_fringe(px, py, c.w, c.h, c.heading, false, &mut paint);
        crate::bio::step::for_cilia_fringe(px, py, c.w, c.h, c.heading, true, &mut paint);
    }
    (w as u16, h as u16, out)
}

/// Seed-mode colour of a genome (species swatch).
pub fn genome_colour(g: &Genome) -> [u8; 3] {
    let m = g.mode(g.seed_mode);
    ModeCoeffs::compile(m, g.metab_eff).colour
}

impl Tank {
    /// Build the archive record of a living cell's organism and mark its
    /// species archived (never pruned, drawn with a portrait in the tree).
    pub fn archive_specimen(&mut self, cell: u32) -> Option<Specimen> {
        let card = self.lineage_card(cell)?;
        let rec = *self.cell(cell)?;
        let genome = self.genomes().genome(rec.genotype).clone();
        let insp = self.inspect(rec.px as u32, rec.py as u32);
        let mut lineage = Vec::new();
        let mut cur = card.species;
        let mut hops = 0;
        while let Some(s) = self.genomes().species(cur) {
            lineage.push(s.name.clone());
            if s.parent == NO_SPECIES || hops > self.species_count() {
                break;
            }
            cur = s.parent;
            hops += 1;
        }
        lineage.reverse();
        let archive_parent = self.archive_parent_of(rec.genotype);
        let mut id_bytes = postcard::to_allocvec(&genome).unwrap_or_default();
        id_bytes.extend_from_slice(&self.seed().to_le_bytes());
        id_bytes.extend_from_slice(&self.tick_count().to_le_bytes());
        let (portrait_w, portrait_h, portrait) = portrait(&genome);
        if let Some(s) = self.genomes_mut().species_mut(card.species) {
            s.archived = true;
        }
        Some(Specimen {
            version: SPECIMEN_VERSION,
            id: hash_bytes(&id_bytes),
            colour: genome_colour(&genome),
            genome,
            display_name: card.species_name.clone(),
            species_name: card.species_name,
            lineage,
            species_depth: card.species_depth,
            tank_seed: self.seed(),
            tick: self.tick_count(),
            generation: card.generation,
            mutations_since_seed: card.mutations_since_seed,
            stats: SpecimenStats {
                cells: insp.org_cells,
                slots: self.org_slot_count(cell) as u32,
                age_ticks: insp.age_ticks,
                max_age_ticks: insp.max_age_ticks,
                children: card.children,
                kin_alive: card.kin_alive,
                descendants_alive: card.descendants_alive,
            },
            notes: String::new(),
            portrait_w,
            portrait_h,
            portrait,
            archive_parent,
        })
    }

    /// Place one copy of an archived creature at pixel `(px, py)`. Its
    /// genome becomes a lineage root of this tank whose species carries
    /// the specimen's name and depth, so the tree continues across tanks
    /// (§c.4). Returns the seed cell id.
    pub fn seed_specimen(&mut self, specimen: &Specimen, px: u16, py: u16) -> Option<u32> {
        if !specimen.genome.is_valid() {
            return None;
        }
        let geno = self.genomes_mut().intern_archive_root(
            specimen.genome.clone(),
            &specimen.species_name,
            specimen.species_depth,
        );
        self.note_archive_root(geno, specimen.id);
        let seed_mode = specimen.genome.seed_mode;
        self.spawn_root(geno, seed_mode, px, py)
    }

    /// New tank from the archive: `n` copies spread along a row of open
    /// water in the upper half. Returns how many were placed.
    pub fn seed_specimen_copies(&mut self, specimen: &Specimen, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        let (pw, ph) = self.pixel_size();
        let vis_h = self.visible_size().1;
        // Rows from a third down, looking for open water under the surface.
        let mut placed = 0;
        'rows: for row in (ph / 3)..(ph * 2 / 3) {
            let vy_top = row / crate::consts::PX_PER_CELL;
            let vy = vis_h as i32 - 1 - vy_top as i32;
            let open: Vec<u32> = (0..pw)
                .filter(|&px| {
                    let vx = (px / crate::consts::PX_PER_CELL) as i32;
                    let c = self.cell_visible(vx, vy);
                    c.is_open() && c.sat >= crate::consts::STANDING_AIR_SAT && self.owner_at(px as u16, row as u16) == 0
                })
                .collect();
            if open.len() < n as usize {
                continue;
            }
            for k in 0..n {
                let i = (open.len() as u32 - 1) * (2 * k + 1) / (2 * n);
                let px = open[i as usize];
                if self.seed_specimen(specimen, px as u16, row as u16).is_some() {
                    placed += 1;
                }
            }
            break 'rows;
        }
        placed
    }

    /// The tree-of-life picture: every species the tank has founded, saved
    /// or not. Same-plan variants are omitted (they folded into a parent).
    /// Extinct ones stay, dimmed. Id order (parents before children).
    pub fn species_tree(&self) -> Vec<SpeciesNode> {
        let genomes = self.genomes();
        let mut nodes: Vec<SpeciesNode> = genomes
            .species_table()
            .iter()
            .enumerate()
            .filter(|(_, s)| !s.collapsed)
            .map(|(i, s)| SpeciesNode {
                id: i as u16,
                parent: s.parent,
                depth: s.depth,
                name: s.name.clone(),
                colour: genome_colour(&s.founder_genome),
                live_cells: 0,
                live_genotypes: 0,
                forms: 0,
                born: 0,
                first_tick: s.first_tick,
                archived: s.archived,
                extinct: true,
            })
            .collect();
        let mut slot_of = vec![usize::MAX; genomes.species_count()];
        for (slot, node) in nodes.iter().enumerate() {
            slot_of[node.id as usize] = slot;
        }
        for i in 0..genomes.node_count() as u32 {
            let Some(n) = genomes.get(i) else {
                continue;
            };
            if n.tombstone {
                continue;
            }
            let Some(&slot) = slot_of.get(n.species as usize) else {
                continue;
            };
            let Some(node) = nodes.get_mut(slot) else {
                continue;
            };
            node.live_cells += n.live_cells;
            node.live_genotypes += u32::from(n.live_cells > 0);
            node.forms += 1 + n.folded_nodes;
            node.born += n.born;
        }
        for node in nodes.iter_mut() {
            node.extinct = node.live_cells == 0;
        }
        nodes
    }

    /// Flag a species as archived (kept by the pruner, drawn with a
    /// portrait). `false` when the id is unknown.
    pub fn mark_species_archived(&mut self, species: u16, on: bool) -> bool {
        match self.genomes_mut().species_mut(species) {
            Some(s) => {
                s.archived = on;
                true
            }
            None => false,
        }
    }
}

/// Built-in archive seeds for Phase 4 (moss / plant / crown / …).
pub fn starter_genome(kind: u8) -> Option<Genome> {
    match kind {
        0 => Some(Genome::moss()),
        1 => Some(Genome::plant()),
        2 => Some(Genome::crown()),
        3 => Some(Genome::rooting()),
        4 => Some(Genome::heater()),
        _ => None,
    }
}

pub fn starter_name(kind: u8) -> &'static str {
    match kind {
        0 => "Moss",
        1 => "Plant",
        2 => "Crown",
        3 => "Rooting",
        4 => "Heater",
        _ => "Starter",
    }
}

impl Specimen {
    /// A starter specimen; `kind` selects moss / plant / crown / …
    pub fn starter(kind: u8) -> Option<Self> {
        let genome = starter_genome(kind)?;
        let name = starter_name(kind).to_string();
        let colour = genome_colour(&genome);
        let (portrait_w, portrait_h, portrait) = portrait(&genome);
        let id_bytes = postcard::to_allocvec(&genome).unwrap_or_default();
        Some(Self {
            version: SPECIMEN_VERSION,
            id: hash_bytes(&id_bytes),
            genome,
            species_name: name.clone(),
            display_name: name,
            lineage: Vec::new(),
            species_depth: 0,
            tank_seed: 0,
            tick: 0,
            generation: 0,
            mutations_since_seed: 0,
            stats: SpecimenStats {
                cells: 1,
                slots: 1,
                ..SpecimenStats::default()
            },
            colour,
            notes: String::new(),
            portrait_w,
            portrait_h,
            portrait,
            archive_parent: None,
        })
    }
}
