//! Tiny feed-forward net driven by muscle feedback + sensors (S4).

use serde::{Deserialize, Serialize};

use crate::body::{LightSample, MuscleFeedback, PressureSample, VestibularSample};

/// Controller architecture tag — more kinds can land later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum NetKind {
    /// Classic MLP: proprioception (+ sensors) → hidden → muscle actuations.
    #[default]
    FeedForwardV1,
}

/// How many of each sensor ending feed the net.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SensorCounts {
    pub pressure: usize,
    pub light: usize,
    /// Cochlea-like balance organs (3 channels each).
    pub vestibular: usize,
}

impl SensorCounts {
    pub fn extra_inputs(self) -> usize {
        self.pressure + self.light + self.vestibular * 3
    }
}

/// One encode frame of creature sensors.
#[derive(Debug, Clone, Default)]
pub struct SensorFrame {
    pub pressure: Vec<PressureSample>,
    pub light: Vec<LightSample>,
    pub vestibular: Vec<VestibularSample>,
}

impl SensorFrame {
    pub fn counts(&self) -> SensorCounts {
        SensorCounts {
            pressure: self.pressure.len(),
            light: self.light.len(),
            vestibular: self.vestibular.len(),
        }
    }
}

/// Fixed-topology controller: feedback → hidden → muscle actuations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StudioNet {
    #[serde(default)]
    pub kind: NetKind,
    pub n_in: usize,
    pub n_hidden: usize,
    pub n_out: usize,
    /// Muscle effector count (outputs).
    #[serde(default)]
    pub n_effectors: usize,
    #[serde(default)]
    pub n_pressure: usize,
    #[serde(default)]
    pub n_light: usize,
    #[serde(default)]
    pub n_vestibular: usize,
    /// Row-major `n_hidden × n_in`.
    pub w1: Vec<f32>,
    pub b1: Vec<f32>,
    /// Row-major `n_out × n_hidden`.
    pub w2: Vec<f32>,
    pub b2: Vec<f32>,
}

impl StudioNet {
    pub fn new(n_in: usize, _n_hidden: usize, n_out: usize, seed: u64) -> Self {
        Self::for_body(
            n_out,
            SensorCounts {
                pressure: n_in.saturating_sub(n_out * 3),
                ..SensorCounts::default()
            },
            seed,
        )
    }

    pub fn for_muscles(n_muscles: usize, seed: u64) -> Self {
        Self::for_body(n_muscles, SensorCounts::default(), seed)
    }

    /// Muscle proprio (×3) + sensor channels.
    pub fn for_body(n_muscles: usize, sensors: SensorCounts, seed: u64) -> Self {
        let n_in = n_muscles * 3 + sensors.extra_inputs();
        let n_hidden = (n_muscles * 4 + sensors.extra_inputs() * 2).max(4);
        let mut w1 = vec![0.0; n_hidden * n_in];
        let mut w2 = vec![0.0; n_muscles * n_hidden];
        let mut b1 = vec![0.0; n_hidden];
        let mut b2 = vec![0.0; n_muscles];
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
            kind: NetKind::FeedForwardV1,
            n_in,
            n_hidden,
            n_out: n_muscles,
            n_effectors: n_muscles,
            n_pressure: sensors.pressure,
            n_light: sensors.light,
            n_vestibular: sensors.vestibular,
            w1,
            b1,
            w2,
            b2,
        }
    }

    pub fn sensor_counts(&self) -> SensorCounts {
        SensorCounts {
            pressure: self.n_pressure,
            light: self.n_light,
            vestibular: self.n_vestibular,
        }
    }

    pub fn matches_body(&self, n_muscles: usize, sensors: SensorCounts) -> bool {
        self.n_out == n_muscles && self.sensor_counts() == sensors
    }

    pub fn kind_label(&self) -> &'static str {
        match self.kind {
            NetKind::FeedForwardV1 => "FF-v1",
        }
    }

    pub fn encode_feedback(fb: &[MuscleFeedback]) -> Vec<f32> {
        Self::encode_inputs(fb, &SensorFrame::default())
    }

    pub fn encode_inputs(fb: &[MuscleFeedback], sensors: &SensorFrame) -> Vec<f32> {
        let mut v = Vec::with_capacity(fb.len() * 3 + sensors.counts().extra_inputs());
        for m in fb {
            v.push(m.actuation);
            v.push((m.length / m.rest_length.max(0.01)).clamp(0.0, 2.0) * 0.5);
            v.push(m.tension.clamp(0.0, 4.0) * 0.25);
        }
        for p in &sensors.pressure {
            v.push(p.pressure.clamp(0.0, 1.0));
        }
        for l in &sensors.light {
            v.push(l.light.clamp(0.0, 1.0));
        }
        for g in &sensors.vestibular {
            v.push(g.upright.clamp(0.0, 1.0));
            v.push(g.ang_rate.clamp(0.0, 1.0));
            v.push(g.fall_speed.clamp(0.0, 1.0));
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
                tension: 1.0,
            },
        ];
        let out = net.forward(&StudioNet::encode_feedback(&fb));
        assert_eq!(out.len(), 2);
    }

    #[test]
    fn multi_sensor_channels() {
        let sensors = SensorCounts {
            pressure: 1,
            light: 1,
            vestibular: 1,
        };
        let net = StudioNet::for_body(1, sensors, 3);
        assert_eq!(net.n_in, 3 + 1 + 1 + 3);
        assert_eq!(net.n_light, 1);
        assert_eq!(net.n_vestibular, 1);
        let fb = [MuscleFeedback {
            muscle_id: 0,
            actuation: 0.0,
            length: 1.0,
            rest_length: 1.0,
            tension: 0.0,
        }];
        let frame = SensorFrame {
            pressure: vec![PressureSample {
                id: 0,
                pressure: 0.5,
            }],
            light: vec![LightSample { id: 0, light: 0.8 }],
            vestibular: vec![VestibularSample {
                id: 0,
                upright: 0.9,
                ang_rate: 0.1,
                fall_speed: 0.0,
            }],
        };
        let out = net.forward(&StudioNet::encode_inputs(&fb, &frame));
        assert_eq!(out.len(), 1);
    }
}
