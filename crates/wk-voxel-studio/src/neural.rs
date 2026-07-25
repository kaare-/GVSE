//! Tiny feed-forward net driven by muscle feedback (S4).

use serde::{Deserialize, Serialize};

use crate::body::MuscleFeedback;

/// Fixed-topology controller: feedback → hidden → muscle actuations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StudioNet {
    pub n_in: usize,
    pub n_hidden: usize,
    pub n_out: usize,
    /// Row-major `n_hidden × n_in`.
    pub w1: Vec<f32>,
    pub b1: Vec<f32>,
    /// Row-major `n_out × n_hidden`.
    pub w2: Vec<f32>,
    pub b2: Vec<f32>,
}

impl StudioNet {
    pub fn new(n_in: usize, n_hidden: usize, n_out: usize, seed: u64) -> Self {
        let mut w1 = vec![0.0; n_hidden * n_in];
        let mut w2 = vec![0.0; n_out * n_hidden];
        let mut b1 = vec![0.0; n_hidden];
        let mut b2 = vec![0.0; n_out];
        let mut s = seed;
        for v in w1.iter_mut().chain(w2.iter_mut()) {
            s = hash(s);
            *v = ((s % 1000) as f32 / 1000.0) * 1.2 - 0.6;
        }
        for v in b1.iter_mut().chain(b2.iter_mut()) {
            s = hash(s);
            *v = ((s % 1000) as f32 / 1000.0) * 0.2 - 0.1;
        }
        Self {
            n_in,
            n_hidden,
            n_out,
            w1,
            b1,
            w2,
            b2,
        }
    }

    pub fn for_muscles(n_muscles: usize, seed: u64) -> Self {
        // Per muscle: actuation, length/rest, tension → 3 inputs.
        Self::new(n_muscles * 3, (n_muscles * 4).max(4), n_muscles, seed)
    }

    pub fn encode_feedback(fb: &[MuscleFeedback]) -> Vec<f32> {
        let mut v = Vec::with_capacity(fb.len() * 3);
        for m in fb {
            v.push(m.actuation);
            v.push((m.length / m.rest_length.max(0.01)).clamp(0.0, 2.0) * 0.5);
            v.push(m.tension.clamp(0.0, 4.0) * 0.25);
        }
        v
    }

    pub fn forward(&self, input: &[f32]) -> Vec<f32> {
        assert_eq!(input.len(), self.n_in);
        let mut h = vec![0.0; self.n_hidden];
        for i in 0..self.n_hidden {
            let mut s = self.b1[i];
            for j in 0..self.n_in {
                s += self.w1[i * self.n_in + j] * input[j];
            }
            h[i] = s.tanh();
        }
        let mut out = vec![0.0; self.n_out];
        for i in 0..self.n_out {
            let mut s = self.b2[i];
            for j in 0..self.n_hidden {
                s += self.w2[i * self.n_hidden + j] * h[j];
            }
            out[i] = (s.tanh() * 0.5 + 0.5).clamp(0.0, 1.0);
        }
        out
    }

    pub fn mutate(&self, seed: u64, scale: f32) -> Self {
        let mut n = self.clone();
        let mut s = seed;
        for v in n
            .w1
            .iter_mut()
            .chain(n.w2.iter_mut())
            .chain(n.b1.iter_mut())
            .chain(n.b2.iter_mut())
        {
            s = hash(s);
            let noise = ((s % 1000) as f32 / 1000.0) * 2.0 - 1.0;
            *v = (*v + noise * scale).clamp(-3.0, 3.0);
        }
        n
    }
}

fn hash(x: u64) -> u64 {
    x.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forward_shape() {
        let net = StudioNet::for_muscles(2, 1);
        let fb = [
            MuscleFeedback {
                muscle_id: 0,
                actuation: 0.2,
                length: 3.0,
                rest_length: 3.0,
                tension: 0.1,
            },
            MuscleFeedback {
                muscle_id: 1,
                actuation: 0.8,
                length: 2.0,
                rest_length: 3.0,
                tension: 0.5,
            },
        ];
        let out = net.forward(&StudioNet::encode_feedback(&fb));
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|v| (0.0..=1.0).contains(v)));
    }
}
