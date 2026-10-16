//! Phase 5 M2 — `StudioNet` controller over sensor → muscle/cilia drives.
//!
//! Ported from `wk-voxel-studio` (`neural.rs`): fixed-topology MLP with
//! hash-seeded weights. Topology exists only when the body has NERVE
//! tissue (`cell-language.md` §a.4 / roadmap M2); intra-cell links from
//! Phase 2 keep working without it. Inter-cell wiring is implied by the
//! shared net (NERVE both ends is the evolvability gate for *having* a
//! net at all).

use serde::{Deserialize, Serialize};

use crate::bio::genome::GenomeStore;
use crate::bio::store::{CellStore, NONE};
use crate::hash::hash_u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum NetKind {
    #[default]
    FeedForwardV1,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SensorCounts {
    pub light: usize,
    pub touch: usize,
    pub chem: usize,
}

impl SensorCounts {
    pub fn extra_inputs(self) -> usize {
        self.light + self.touch + self.chem
    }
}

/// Fixed-topology controller: proprio + sensors → hidden → effector drives.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StudioNet {
    pub kind: NetKind,
    pub n_in: usize,
    pub n_hidden: usize,
    pub n_out: usize,
    pub n_effectors: usize,
    pub n_light: usize,
    pub n_touch: usize,
    pub n_chem: usize,
    pub w1: Vec<f32>,
    pub b1: Vec<f32>,
    pub w2: Vec<f32>,
    pub b2: Vec<f32>,
}

#[derive(Debug, Clone, Copy)]
pub struct EffectorFeedback {
    pub actuation: f32,
    pub length_ratio: f32,
    pub tension: f32,
}

#[derive(Debug, Clone, Default)]
pub struct SensorFrame {
    pub light: Vec<f32>,
    pub touch: Vec<f32>,
    pub chem: Vec<f32>,
}

impl StudioNet {
    pub fn for_body(n_effectors: usize, sensors: SensorCounts, seed: u64) -> Self {
        let n_in = n_effectors * 3 + sensors.extra_inputs();
        let n_hidden = (n_effectors * 4 + sensors.extra_inputs() * 2).max(4);
        let mut w1 = vec![0.0; n_hidden * n_in.max(1)];
        let mut w2 = vec![0.0; n_effectors.max(1) * n_hidden];
        let mut b1 = vec![0.0; n_hidden];
        let mut b2 = vec![0.0; n_effectors.max(1)];
        for (i, v) in w1.iter_mut().chain(w2.iter_mut()).enumerate() {
            let s = hash_u64(seed, i as u64, 0x5E_01, 0);
            *v = ((s % 1000) as f32 / 1000.0) * 1.2 - 0.6;
        }
        for (i, v) in b1.iter_mut().chain(b2.iter_mut()).enumerate() {
            let s = hash_u64(seed, i as u64, 0x5E_02, 0);
            *v = ((s % 1000) as f32 / 1000.0) * 0.2 - 0.1;
        }
        Self {
            kind: NetKind::FeedForwardV1,
            n_in,
            n_hidden,
            n_out: n_effectors,
            n_effectors,
            n_light: sensors.light,
            n_touch: sensors.touch,
            n_chem: sensors.chem,
            w1,
            b1,
            w2,
            b2,
        }
    }

    pub fn sensor_counts(&self) -> SensorCounts {
        SensorCounts {
            light: self.n_light,
            touch: self.n_touch,
            chem: self.n_chem,
        }
    }

    pub fn matches_body(&self, n_effectors: usize, sensors: SensorCounts) -> bool {
        self.n_out == n_effectors && self.sensor_counts() == sensors
    }

    pub fn encode_inputs(fb: &[EffectorFeedback], sensors: &SensorFrame) -> Vec<f32> {
        let mut v = Vec::with_capacity(fb.len() * 3 + sensors.light.len() + sensors.touch.len() + sensors.chem.len());
        for m in fb {
            v.push(m.actuation);
            v.push(m.length_ratio.clamp(0.0, 2.0) * 0.5);
            v.push(m.tension.clamp(0.0, 4.0) * 0.25);
        }
        for &x in &sensors.light {
            v.push(x.clamp(0.0, 1.0));
        }
        for &x in &sensors.touch {
            v.push(x.clamp(0.0, 1.0));
        }
        for &x in &sensors.chem {
            v.push(x.clamp(0.0, 1.0));
        }
        v
    }

    pub fn forward(&self, input: &[f32]) -> Vec<f32> {
        if input.len() != self.n_in || self.n_out == 0 {
            return vec![0.0; self.n_out];
        }
        let mut h = vec![0.0; self.n_hidden];
        for (i, hi) in h.iter_mut().enumerate() {
            let mut s = self.b1[i];
            for (j, &xj) in input.iter().enumerate() {
                s += self.w1[i * self.n_in + j] * xj;
            }
            *hi = s.tanh();
        }
        let mut out = vec![0.0; self.n_out];
        for (i, oi) in out.iter_mut().enumerate() {
            let mut s = self.b2[i];
            for (j, &hj) in h.iter().enumerate() {
                s += self.w2[i * self.n_hidden + j] * hj;
            }
            *oi = (s.tanh() * 0.5 + 0.5).clamp(0.0, 1.0);
        }
        out
    }

    pub fn mutate(&self, seed: u64, scale: f32) -> Self {
        let mut n = self.clone();
        for (i, v) in n
            .w1
            .iter_mut()
            .chain(n.w2.iter_mut())
            .chain(n.b1.iter_mut())
            .chain(n.b2.iter_mut())
            .enumerate()
        {
            let s = hash_u64(seed, i as u64, 0x5E_03, 0);
            let noise = ((s % 1000) as f32 / 1000.0) * 2.0 - 1.0;
            *v = (*v + noise * scale).clamp(-3.0, 3.0);
        }
        n
    }
}

#[derive(Debug, Default)]
pub struct NetStore {
    /// Parallel to org ids.
    nets: Vec<Option<StudioNet>>,
}

impl NetStore {
    pub fn clear(&mut self) {
        self.nets.clear();
    }

    fn slot(&mut self, org: u32) -> &mut Option<StudioNet> {
        let i = org as usize;
        if i >= self.nets.len() {
            self.nets.resize_with(i + 1, || None);
        }
        &mut self.nets[i]
    }
}

/// Drive threshold (studio used 0.55 for discrete pull).
pub const NET_DRIVE: f32 = 0.55;

/// For each organism with NERVE tissue, rebuild/reuse a net and write
/// effector `running` from its outputs. Returns orgs driven.
pub fn step(
    nets: &mut NetStore,
    cells: &mut CellStore,
    genomes: &GenomeStore,
    world_seed: u64,
) -> u32 {
    let orgs: Vec<u32> = cells
        .iter_orgs()
        .filter(|(_, o)| o.alive && o.n_alive >= 1)
        .map(|(id, _)| id)
        .collect();
    let mut driven = 0u32;
    for org_id in orgs {
        if drive_org(nets, cells, genomes, world_seed, org_id) {
            driven += 1;
        }
    }
    driven
}

fn drive_org(
    nets: &mut NetStore,
    cells: &mut CellStore,
    genomes: &GenomeStore,
    world_seed: u64,
    org_id: u32,
) -> bool {
    let Some(o) = cells.org(org_id) else {
        return false;
    };
    let mut nerve = 0u32;
    let mut effectors: Vec<u32> = Vec::new();
    let mut sensors = SensorFrame::default();
    let mut sensor_counts = SensorCounts::default();
    for s in o.slots.iter().filter(|s| s.cell != NONE) {
        let Some(c) = cells.get(s.cell) else {
            continue;
        };
        let mc = genomes.coeffs(c.genotype, c.mode);
        if mc.nerve > 0 {
            nerve += 1;
        }
        if mc.muscle > 0 || mc.cilia > 0 {
            effectors.push(s.cell);
        }
        if mc.sensor > 0 {
            let v = (c.sense as f32 / 255.0).clamp(0.0, 1.0);
            match mc.sensor_kind {
                0 => {
                    sensors.light.push(v);
                    sensor_counts.light += 1;
                }
                1 => {
                    sensors.touch.push(v);
                    sensor_counts.touch += 1;
                }
                _ => {
                    sensors.chem.push(v);
                    sensor_counts.chem += 1;
                }
            }
        }
    }
    // Net exists only where nerve tissue was evolved.
    if nerve == 0 || effectors.is_empty() {
        *nets.slot(org_id) = None;
        return false;
    }

    let n_eff = effectors.len();
    let need_new = !matches!(
        nets.slot(org_id),
        Some(n) if n.matches_body(n_eff, sensor_counts)
    );
    if need_new {
        let seed = hash_u64(world_seed, org_id as u64, 0x5E_10, nerve as u64);
        *nets.slot(org_id) = Some(StudioNet::for_body(n_eff, sensor_counts, seed));
    }
    let Some(net) = nets.slot(org_id).as_ref() else {
        return false;
    };

    let fb: Vec<EffectorFeedback> = effectors
        .iter()
        .map(|&id| {
            let c = cells.get(id);
            EffectorFeedback {
                actuation: if c.map(|c| c.running).unwrap_or(false) {
                    1.0
                } else {
                    0.0
                },
                length_ratio: 1.0,
                tension: c.map(|c| c.stress as f32 / 255.0).unwrap_or(0.0),
            }
        })
        .collect();
    let input = StudioNet::encode_inputs(&fb, &sensors);
    // Pad/truncate if sensor counts drifted mid-tick.
    let mut input = input;
    input.resize(net.n_in, 0.0);
    let out = net.forward(&input);
    for (i, &id) in effectors.iter().enumerate() {
        let drive = out.get(i).copied().unwrap_or(0.0) >= NET_DRIVE;
        if let Some(c) = cells.get_mut(id) {
            // Net OR with existing intra-cell drive so Phase 2 links still work.
            c.running = c.running || drive;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_shape_is_stable() {
        let net = StudioNet::for_body(
            2,
            SensorCounts {
                light: 1,
                touch: 0,
                chem: 0,
            },
            1,
        );
        let fb = [
            EffectorFeedback {
                actuation: 0.2,
                length_ratio: 1.0,
                tension: 0.1,
            },
            EffectorFeedback {
                actuation: 0.8,
                length_ratio: 0.7,
                tension: 1.0,
            },
        ];
        let sensors = SensorFrame {
            light: vec![0.6],
            ..SensorFrame::default()
        };
        let out = net.forward(&StudioNet::encode_inputs(&fb, &sensors));
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|&x| (0.0..=1.0).contains(&x)));
    }
}
