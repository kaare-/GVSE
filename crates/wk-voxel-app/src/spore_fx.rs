//! Ephemeral wind-borne spore puffs (cosmetic). Not saved / not sim mass.
//!
//! Spawned when [`wk_voxel::OrganismStore::step_with_climate_wind`] reports
//! a [`wk_voxel::SporeRelease`]. Particles loft, home toward the landing
//! column, settle on the sporeling seat, then fade.

use macroquad::prelude::*;
use wk_voxel::{ModuleId, SporeRelease};

/// One floating spore speck in world cell space.
#[derive(Debug, Clone)]
struct SporePuff {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    /// Landing seat (sporeling crown).
    tx: f32,
    ty: f32,
    life: f32,
    max_life: f32,
    /// 0..1 size scale.
    size: f32,
    /// True after the puff reaches the seat — fade in place.
    landed: bool,
}

#[derive(Debug, Default)]
pub struct SporeFx {
    puffs: Vec<SporePuff>,
    /// Salt so successive bursts don't stack identical paths.
    burst_i: u64,
}

impl SporeFx {
    pub fn new() -> Self {
        Self::default()
    }

    /// Emit a short lilac plume from parent toward the sporeling seat.
    pub fn burst(&mut self, release: &SporeRelease, wind_vx: f32) {
        self.burst_i = self.burst_i.wrapping_add(1);
        let dx = (release.to_gx - release.from_gx) as f32;
        let dy = (release.to_gy - release.from_gy) as f32;
        let dist = (dx * dx + dy * dy).sqrt().max(1.0);
        // Flight time scales with distance so long hops stay readable.
        let flight = (0.7 + dist * 0.09).clamp(0.8, 2.8);
        let n = 8usize;
        for i in 0..n {
            let h = hash01(self.burst_i, i as u64, 0x5F0E);
            let h2 = hash01(self.burst_i, i as u64, 0x5F0F);
            let h3 = hash01(self.burst_i, i as u64, 0x5F10);
            let scatter_y = (h2 - 0.5) * 1.2;
            let scatter_x = (h3 - 0.5) * 0.7;
            // Aim near the landing crown with a little ground scatter.
            let tx = release.to_gx as f32 + 0.5 + scatter_x * 0.35;
            let ty = release.to_gy as f32 + 0.35 + scatter_y.abs() * 0.15;
            // Initial loft + wind; homing takes over in `update`.
            let dir = if dx.abs() > 0.5 {
                dx.signum()
            } else if wind_vx.abs() > 0.01 {
                wind_vx.signum()
            } else if self.burst_i & 1 == 0 {
                1.0
            } else {
                -1.0
            };
            let speed = (dist / flight) * (0.85 + h * 0.35);
            let life = flight + 0.35 + h * 0.45; // linger briefly after landing
            self.puffs.push(SporePuff {
                x: release.from_gx as f32 + 0.5 + scatter_x,
                y: release.from_gy as f32 + 0.85 + scatter_y * 0.35,
                vx: dir * speed * 0.55 + wind_vx * 8.0,
                vy: 1.1 + h2 * 0.8,
                tx,
                ty,
                life,
                max_life: life,
                size: 0.35 + h3 * 0.55,
                landed: false,
            });
        }
        // Soft cap so long demos don't accumulate thousands of puffs.
        if self.puffs.len() > 400 {
            let drop = self.puffs.len() - 400;
            self.puffs.drain(0..drop);
        }
    }

    pub fn burst_all(&mut self, releases: &[SporeRelease], wind_vx: f32) {
        for r in releases {
            self.burst(r, wind_vx);
        }
    }

    /// Advance puffs in world space (call every frame, even when paused).
    pub fn update(&mut self, dt: f32, wind_vx: f32, wrap_width: Option<i32>) {
        if dt <= 0.0 || self.puffs.is_empty() {
            return;
        }
        let wind_cells = wind_vx * 10.0;
        for p in &mut self.puffs {
            p.life -= dt;
            if p.landed {
                // Settle: stick near the seat, faint drift, then fade out.
                p.vx *= 1.0 - 4.0 * dt;
                p.vy *= 1.0 - 4.0 * dt;
                p.x += p.vx * dt * 0.15;
                p.y += p.vy * dt * 0.15;
                continue;
            }

            // Shortest wrapped delta to the landing seat.
            let mut ddx = p.tx - p.x;
            if let Some(w) = wrap_width {
                if w > 0 {
                    let wf = w as f32;
                    if ddx > wf * 0.5 {
                        ddx -= wf;
                    } else if ddx < -wf * 0.5 {
                        ddx += wf;
                    }
                }
            }
            let ddy = p.ty - p.y;
            let dist = (ddx * ddx + ddy * ddy).sqrt();

            if dist < 0.35 {
                p.x = p.tx;
                p.y = p.ty;
                p.vx = 0.0;
                p.vy = 0.0;
                p.landed = true;
                // Keep a short linger so the landing reads.
                p.life = p.life.min(0.45);
                p.max_life = p.life.max(0.2);
                continue;
            }

            // Home toward the seat; wind adds a soft cross-breeze.
            let home = 5.5 + dist * 0.35;
            let inv = 1.0 / dist;
            p.vx = p.vx * (1.0 - 1.8 * dt) + ddx * inv * home * dt * 8.0 + wind_cells * dt * 2.0;
            p.vy = p.vy * (1.0 - 1.5 * dt) + ddy * inv * home * dt * 8.0 - 0.25 * dt;
            // Cap so far seats don't teleport.
            let speed = (p.vx * p.vx + p.vy * p.vy).sqrt();
            let max_speed = 18.0;
            if speed > max_speed {
                p.vx *= max_speed / speed;
                p.vy *= max_speed / speed;
            }
            p.x += p.vx * dt;
            p.y += p.vy * dt;
            if let Some(w) = wrap_width {
                if w > 0 {
                    let wf = w as f32;
                    p.x = p.x.rem_euclid(wf);
                    // Keep target in the same wrap frame as the puff.
                    let tdx = p.tx - p.x;
                    if tdx > wf * 0.5 {
                        p.tx -= wf;
                    } else if tdx < -wf * 0.5 {
                        p.tx += wf;
                    }
                }
            }
        }
        self.puffs.retain(|p| p.life > 0.0);
    }

    pub fn draw(
        &self,
        origin_x: f32,
        origin_y: f32,
        cell_px: f32,
        bedrock_floor_y: i32,
        width_cols: i32,
        wrap_x: bool,
        sw: f32,
        sh: f32,
    ) {
        if self.puffs.is_empty() {
            return;
        }
        let (r, g, b) = ModuleId::ReproSpore.rgb();
        let x_copies: &[i32] = if wrap_x { &[-1, 0, 1] } else { &[0] };
        for p in &self.puffs {
            let t = (p.life / p.max_life).clamp(0.0, 1.0);
            // Fade in quick; landed puffs fade out from the seat.
            let alpha = if p.landed {
                (t / 0.9).clamp(0.0, 1.0)
            } else if t > 0.85 {
                ((1.0 - t) / 0.15).clamp(0.0, 1.0)
            } else {
                t.clamp(0.25, 1.0)
            };
            let a = (alpha * 230.0) as u8;
            let px = (p.size * cell_px * 0.55).clamp(1.5, cell_px * 0.85);
            for &x_copy in x_copies {
                let sx = origin_x + (p.x + x_copy as f32 * width_cols as f32) * cell_px;
                let sy = origin_y - (p.y - bedrock_floor_y as f32) * cell_px;
                if sx + px < 0.0 || sx > sw || sy < 0.0 || sy - px > sh {
                    continue;
                }
                draw_circle(sx, sy - cell_px * 0.5, px * 0.5, Color::from_rgba(r, g, b, a));
            }
        }
    }
}

fn hash01(a: u64, b: u64, salt: u64) -> f32 {
    let mut x = a
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(b)
        .wrapping_add(salt);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    (x >> 40) as f32 / ((1u64 << 24) as f32)
}
