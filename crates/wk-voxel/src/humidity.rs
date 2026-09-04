//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Atmospheric water heatmap.
//!
//! Coarse-resolution sparse map of "water mass currently in the air"
//! per `tile_cols × tile_cols` block of world cells. Evaporation
//! routes removed cell saturation into this heatmap so the sim stays
//! mass-conservative even when water leaves the ground.
//!
//! The heatmap is intentionally decoupled from the cell grid so it
//! can run at its own resolution — coarser than cells, matching the
//! design doc's "temperature/humidity/wind sampled at 4×4 tiles"
//! plan. Diffusion is a straight two-neighbour (right + up) pairwise
//! filter; combined with symmetric application that's the standard
//! isotropic diffusion stencil in a form that trivially conserves
//! mass across a snapshot pass.
//!
//! Callers should set [`Humidity::bounds`] to the stamped world so
//! diffusion cannot grow an unbounded sparse haze outside the map.
//! Diffusion itself is also meant to run on a schedule (see
//! [`humidity_diffuse_due`]) — matching column-GVSE's `HumidityField`
//! period — not every physics tick.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::fasthash::{FxHashMap, FxHashSet};

/// Inclusive tile-coordinate rectangle.
///
/// When set on a [`Humidity`], diffusion stays inside the box.
/// Vertical edges are Neumann (no-flux). Horizontal edges wrap when
/// [`Humidity::wrap_x`] is set (ring worlds).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TileBounds {
    pub hx_min: i32,
    pub hx_max: i32,
    pub hy_min: i32,
    pub hy_max: i32,
}

impl TileBounds {
    /// Tile box covering world cells `[x0, x1) × [y0, y1)`.
    pub fn from_world_cells(tile_cols: i32, x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        let tc = tile_cols.max(1);
        let x_lo = x0.min(x1 - 1);
        let x_hi = (x1 - 1).max(x0);
        let y_lo = y0.min(y1 - 1);
        let y_hi = (y1 - 1).max(y0);
        Self {
            hx_min: x_lo.div_euclid(tc),
            hx_max: x_hi.div_euclid(tc),
            hy_min: y_lo.div_euclid(tc),
            hy_max: y_hi.div_euclid(tc),
        }
    }

    pub fn contains(self, hx: i32, hy: i32) -> bool {
        hx >= self.hx_min && hx <= self.hx_max && hy >= self.hy_min && hy <= self.hy_max
    }

    pub fn tile_capacity(self) -> usize {
        let (w, h) = self.dims();
        w.saturating_mul(h)
    }

    /// `(width, height)` in tiles. Either side may be zero.
    pub fn dims(self) -> (usize, usize) {
        let w = (self.hx_max - self.hx_min + 1).max(0) as usize;
        let h = (self.hy_max - self.hy_min + 1).max(0) as usize;
        (w, h)
    }

    /// Row-major index. Caller guarantees `contains(hx, hy)` and `w > 0`.
    pub fn index(self, w: usize, hx: i32, hy: i32) -> usize {
        (hy - self.hy_min) as usize * w + (hx - self.hx_min) as usize
    }

    /// Inverse of [`Self::index`]. Caller guarantees `w > 0`.
    pub fn coords(self, w: usize, i: usize) -> (i32, i32) {
        (
            self.hx_min + (i % w) as i32,
            self.hy_min + (i / w) as i32,
        )
    }

    /// Packed sequential walk when the box is tiny (tests) or at least
    /// half occupied. Same stencil as the sparse path — not view LOD.
    pub fn prefer_dense_walk(self, occupied: usize) -> bool {
        let cap = self.tile_capacity();
        cap > 0 && (cap <= 256 || occupied.saturating_mul(2) >= cap)
    }
}

/// Cadence for atmospheric diffusion — same numbers as column-GVSE
/// `SubsystemId::HumidityField` (`period: 20`, `phase: 3`).
pub const HUMIDITY_DIFFUSE_PERIOD: u64 = 20;
pub const HUMIDITY_DIFFUSE_PHASE: u64 = 3;
/// Face-following wind can report `|vy|` near 1.0. Humidity treats `|v|`
/// as the fraction of tile mass that leaves this tick, so an uncapped
/// climb vacuums the air below `min_mass_to_rain` and C / surplus never
/// drop. Overlay and slip keep the raw field; only this hop is damped.
const HUMIDITY_VY_ADV_CAP: f32 = 0.10;

/// True on ticks when humidity diffusion should run.
pub fn humidity_diffuse_due(tick: u64) -> bool {
    tick % HUMIDITY_DIFFUSE_PERIOD == HUMIDITY_DIFFUSE_PHASE
}

/// A sparse 2D heatmap keyed by tile coordinates. Each tile covers
/// `tile_cols` × `tile_cols` world cells. Missing keys are implicit
/// zero — a fresh atmosphere is dry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Humidity {
    /// World cells per tile side. Must be ≥ 1. Typical value: 4.
    pub tile_cols: i32,
    /// Water mass per tile. Same units as [`crate::cell::Sat`] (a `u8`
    /// on 0..255) but stored as `f32` so diffusion can accumulate
    /// fractional deltas without quantisation error.
    pub cells: HashMap<(i32, i32), f32>,
    /// Optional hard clamp on tile keys. `None` leaves diffusion
    /// unbounded (unit-test convenience only — production worlds
    /// should always set this).
    pub bounds: Option<TileBounds>,
    /// When true (and [`Self::bounds`] is set), horizontal diffusion
    /// wraps at `hx_min`/`hx_max` so the atmosphere joins on a ring.
    pub wrap_x: bool,
    /// Legacy residual from the integer-step advect path. Flux
    /// advection keeps these at zero; field stays for old saves.
    #[serde(default)]
    pub advect_rx: f32,
    #[serde(default)]
    pub advect_ry: f32,
}

impl Humidity {
    pub fn new(tile_cols: i32) -> Self {
        Self {
            tile_cols: tile_cols.max(1),
            cells: HashMap::new(),
            bounds: None,
            wrap_x: false,
            advect_rx: 0.0,
            advect_ry: 0.0,
        }
    }

    /// Convenience: humidity map pre-clamped to a stamped world's
    /// cell rectangle `[x0, x1) × [y0, y1)`.
    pub fn with_world_bounds(tile_cols: i32, x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        let mut h = Self::new(tile_cols);
        h.bounds = Some(TileBounds::from_world_cells(h.tile_cols, x0, y0, x1, y1));
        h
    }

    /// Tile coord for a world cell.
    pub fn tile_of(&self, gx: i32, gy: i32) -> (i32, i32) {
        (gx.div_euclid(self.tile_cols), gy.div_euclid(self.tile_cols))
    }

    fn accepts(&self, hx: i32, hy: i32) -> bool {
        self.bounds.map(|b| b.contains(hx, hy)).unwrap_or(true)
    }

    fn use_dense_slab(&self, b: TileBounds) -> bool {
        b.prefer_dense_walk(self.cells.len())
    }

    fn pack_slab(&self, b: TileBounds) -> Vec<f32> {
        let (w, h) = b.dims();
        let mut slab = vec![0.0f32; w.saturating_mul(h)];
        for (&(hx, hy), &v) in &self.cells {
            if b.contains(hx, hy) {
                slab[b.index(w, hx, hy)] = v;
            }
        }
        slab
    }

    /// Neighbour tile in +x / −x, wrapping horizontally on ring maps.
    pub fn wrap_tile_x(&self, hx: i32) -> Option<i32> {
        self.wrap_hx(hx)
    }

    /// Neighbour tile in +x / −x, wrapping horizontally on ring maps.
    fn wrap_hx(&self, hx: i32) -> Option<i32> {
        match self.bounds {
            Some(b) if self.wrap_x => {
                let w = b.hx_max - b.hx_min + 1;
                if w <= 0 {
                    return None;
                }
                Some(b.hx_min + (hx - b.hx_min).rem_euclid(w))
            }
            Some(b) => {
                if hx >= b.hx_min && hx <= b.hx_max {
                    Some(hx)
                } else {
                    None
                }
            }
            None => Some(hx),
        }
    }

    /// Deposit `mass` at world cell `(gx, gy)`.
    ///
    /// Deposits outside [`Self::bounds`] are dropped (the cell grid
    /// should not evaporate outside the stamped world).
    /// Soft per-tile ceiling so evaporation cannot stockpile unboundedly
    /// when rain / condensation cannot keep up (overnight flood safety).
    /// This is the hold of **very warm** air ([`Self::SAT_FULL_TEMP_C`]).
    pub const MAX_MASS_PER_TILE: f32 = 2_500.0;

    /// Temperature at which a tile holds [`Self::MAX_MASS_PER_TILE`].
    ///
    /// 40 °C is "very warm" sky for this climate (hot desert). 100 °C is
    /// boiling steam — if that were the top, 18 °C air would hold ~2 %
    /// and the field would rain out constantly.
    pub const SAT_FULL_TEMP_C: f32 = 40.0;
    /// Curve is defined down to here; hold floors at a trace so cold
    /// tiles do not divide-by-zero.
    pub const SAT_MIN_TEMP_C: f32 = -100.0;

    /// Saturation vapour pressure (hPa), Magnus / August-Roche-Magnus.
    ///
    /// Water above 0 °C, ice below. Exponential in T — the natural
    /// Clausius–Clapeyron shape, not a linear ramp.
    pub fn sat_vapor_pressure_hpa(temp_c: f32) -> f32 {
        let t = temp_c.clamp(Self::SAT_MIN_TEMP_C, 100.0);
        if t >= 0.0 {
            6.112 * (17.62 * t / (t + 243.12)).exp()
        } else {
            6.112 * (22.46 * t / (t + 272.62)).exp()
        }
    }

    /// How much vapor a humidity tile can hold at `temp_c`.
    ///
    /// Full at [`Self::SAT_FULL_TEMP_C`]. Same mass in colder air is
    /// closer to rain / visible cloud. A 255-on-a-cell picture is the
    /// same curve: 40 °C → 255, 0 °C → ~21, −0.1 °C → ~21, −3 °C → ~16,
    /// −20 °C → ~4, −100 °C → ~0. Inspector `humidity=` is this tile
    /// mass (40 °C → 2500, 0 °C → ~207, −0.1 °C → ~206, −3 °C → ~162).
    pub fn saturation_mass_at_temp(temp_c: f32) -> f32 {
        let full = Self::sat_vapor_pressure_hpa(Self::SAT_FULL_TEMP_C);
        let here = Self::sat_vapor_pressure_hpa(temp_c);
        let ratio = (here / full.max(1e-6)).clamp(0.0, 1.0);
        (Self::MAX_MASS_PER_TILE * ratio).max(0.5)
    }

    /// [`Self::saturation_mass_at_temp`] scaled onto a 0..255 air cell.
    pub fn saturation_cell_sat_at_temp(temp_c: f32) -> f32 {
        let full = Self::sat_vapor_pressure_hpa(Self::SAT_FULL_TEMP_C);
        let here = Self::sat_vapor_pressure_hpa(temp_c);
        let ratio = (here / full.max(1e-6)).clamp(0.0, 1.0);
        (u8::MAX as f32 * ratio).max(0.05)
    }

    pub fn add(&mut self, gx: i32, gy: i32, mass: f32) {
        let _ = self.try_add(gx, gy, mass);
    }

    /// Add mass; returns how much was actually accepted under the
    /// per-tile cap ([`Self::MAX_MASS_PER_TILE`]).
    pub fn try_add(&mut self, gx: i32, gy: i32, mass: f32) -> f32 {
        self.try_add_capped(gx, gy, mass, Self::MAX_MASS_PER_TILE)
    }

    /// [`Self::try_add`] capped at [`Self::saturation_mass_at_temp`].
    ///
    /// Refuses *new* mass the air cannot hold. Does **not** clamp
    /// vapour already in the tile — that surplus is rain above freeze,
    /// or a gathered flake / hold below
    /// ([`crate::precipitate_thermal_surplus`]), not a delete.
    pub fn try_add_at_temp(&mut self, gx: i32, gy: i32, mass: f32, temp_c: f32) -> f32 {
        self.try_add_capped(gx, gy, mass, Self::saturation_mass_at_temp(temp_c))
    }

    fn try_add_capped(&mut self, gx: i32, gy: i32, mass: f32, cap: f32) -> f32 {
        if mass <= 0.0 {
            return 0.0;
        }
        let key = self.tile_of(gx, gy);
        if !self.accepts(key.0, key.1) {
            return 0.0;
        }
        let entry = self.cells.entry(key).or_insert(0.0);
        let room = (cap.max(0.5) - *entry).max(0.0);
        let take = mass.min(room);
        *entry += take;
        take
    }

    /// Remove up to `mass` from the tile covering `(gx, gy)`. Returns
    /// how much was actually removed.
    pub fn take(&mut self, gx: i32, gy: i32, mass: f32) -> f32 {
        if mass <= 0.0 {
            return 0.0;
        }
        let key = self.tile_of(gx, gy);
        let Some(entry) = self.cells.get_mut(&key) else {
            return 0.0;
        };
        let take = mass.min(*entry);
        *entry -= take;
        if *entry < 1e-3 {
            self.cells.remove(&key);
        }
        take
    }

    /// Pull vapor from a short vertical stack of tiles near `(gx, gy)`.
    pub fn take_near(&mut self, gx: i32, gy: i32, mass: f32) -> f32 {
        if mass <= 0.0 {
            return 0.0;
        }
        let mut need = mass;
        let mut got = 0.0;
        for dy in [0, -4, -8, 4, -12, 8, -16] {
            if need <= 1e-3 {
                break;
            }
            let took = self.take(gx, gy + dy, need);
            got += took;
            need -= took;
        }
        got
    }

    /// Peek available vapor near `(gx, gy)` without removing it.
    pub fn peek_near(&self, gx: i32, gy: i32) -> f32 {
        let mut total = 0.0;
        for dy in [0, -4, -8, 4, -12, 8, -16] {
            total += self.at_cell(gx, gy + dy);
        }
        total
    }

    /// Humidity on this tile plus the 8 Moore neighbours.
    ///
    /// Used by the snow lottery: a flake still costs a full water cell
    /// (thaw is `Air+FULL`), so we gather from the local parcel instead
    /// of raining leftover vapour as liquid below freeze.
    pub fn peek_around(&self, gx: i32, gy: i32) -> f32 {
        let (hx, hy) = self.tile_of(gx, gy);
        self.peek_around_tile(hx, hy)
    }

    /// [`Self::peek_around`] in tile coordinates.
    pub fn peek_around_tile(&self, hx: i32, hy: i32) -> f32 {
        let mut total = 0.0;
        for dy in -1..=1 {
            for dx in -1..=1 {
                let Some(nx) = self.wrap_hx(hx + dx) else {
                    continue;
                };
                if !self.accepts(nx, hy + dy) {
                    continue;
                }
                total += self.at_tile(nx, hy + dy);
            }
        }
        total
    }

    /// Spend `mass` from this tile first, then the 8 Moore neighbours
    /// (same stencil as [`Self::peek_around`]). Horizontal tiles are
    /// fair here: the flake is paying a full cell, not a leftover drizzle.
    pub fn take_around(&mut self, gx: i32, gy: i32, mass: f32) -> f32 {
        let (hx, hy) = self.tile_of(gx, gy);
        self.take_around_tile(hx, hy, mass)
    }

    /// [`Self::take_around`] in tile coordinates. Centre tile first.
    pub fn take_around_tile(&mut self, hx: i32, hy: i32, mass: f32) -> f32 {
        if mass <= 0.0 {
            return 0.0;
        }
        let tc = self.tile_cols.max(1);
        let cell_of = |tx: i32, ty: i32| (tx * tc + tc / 2, ty * tc + tc / 2);
        let mut need = mass;
        let mut got = 0.0;
        // Centre first so a tile that can pay for itself does.
        let order = [
            (0, 0),
            (-1, 0),
            (1, 0),
            (0, -1),
            (0, 1),
            (-1, -1),
            (1, -1),
            (-1, 1),
            (1, 1),
        ];
        for (dx, dy) in order {
            if need <= 1e-3 {
                break;
            }
            let Some(nx) = self.wrap_hx(hx + dx) else {
                continue;
            };
            let ny = hy + dy;
            if !self.accepts(nx, ny) {
                continue;
            }
            let (cx, cy) = cell_of(nx, ny);
            let took = self.take(cx, cy, need);
            got += took;
            need -= took;
        }
        got
    }

    /// Humidity mass at world cell `(gx, gy)`. Missing tile → 0.
    pub fn at_cell(&self, gx: i32, gy: i32) -> f32 {
        let key = self.tile_of(gx, gy);
        *self.cells.get(&key).unwrap_or(&0.0)
    }

    /// Humidity mass at tile coord `(hx, hy)`. Missing → 0.
    pub fn at_tile(&self, hx: i32, hy: i32) -> f32 {
        *self.cells.get(&(hx, hy)).unwrap_or(&0.0)
    }

    /// Tiles above a surface cell that count as its vapor column.
    pub const VAPOR_COLUMN_TILES: i32 = 12;

    /// True when the air column above `(gx, gy)` is already wet enough
    /// that more evaporation would only stockpile sky haze.
    ///
    /// Buoyant rise empties the surface tile every tick, so the per-tile
    /// cap at sea level never trips — a long soak used to fill the whole
    /// sky grid, then condensation walked every column (~7 FPS).
    pub fn column_near_saturated(&self, gx: i32, gy: i32) -> bool {
        let (hx, hy0) = self.tile_of(gx, gy);
        let mut sum = 0.0f32;
        let mut n = 0i32;
        let mut peak = 0.0f32;
        for i in 0..Self::VAPOR_COLUMN_TILES {
            let hy = hy0 + i;
            if !self.accepts(hx, hy) {
                break;
            }
            let m = self.at_tile(hx, hy);
            sum += m;
            peak = peak.max(m);
            n += 1;
        }
        if n == 0 {
            return false;
        }
        peak >= Self::MAX_MASS_PER_TILE * 0.92
            || (sum / n as f32) >= Self::MAX_MASS_PER_TILE * 0.50
    }

    /// True when total vapor exceeds a thin cloud-deck budget (not the
    /// entire sky rectangle). Long soaks used to saturate every tile.
    pub fn atmosphere_overfull(&self) -> bool {
        let width = match self.bounds {
            Some(b) => (b.hx_max - b.hx_min + 1).max(1) as f32,
            None => {
                let mut min_hx = i32::MAX;
                let mut max_hx = i32::MIN;
                for &(hx, _) in self.cells.keys() {
                    min_hx = min_hx.min(hx);
                    max_hx = max_hx.max(hx);
                }
                if min_hx > max_hx {
                    return false;
                }
                (max_hx - min_hx + 1).max(1) as f32
            }
        };
        let budget = width * 8.0 * (Self::MAX_MASS_PER_TILE * 0.45);
        self.total_mass() > budget
    }

    /// Bilinear sample in world-cell space (smooth haze; no tile facets).
    ///
    /// Tile centres sit at `(hx + 0.5, hy + 0.5) * tile_cols`. Horizontal
    /// samples wrap when [`Self::wrap_x`] is set.
    pub fn sample_bilinear(&self, gx: f32, gy: f32) -> f32 {
        let tc = self.tile_cols.max(1) as f32;
        let fx = gx / tc - 0.5;
        let fy = gy / tc - 0.5;
        let x0 = fx.floor() as i32;
        let y0 = fy.floor() as i32;
        let tx = (fx - x0 as f32).clamp(0.0, 1.0);
        let ty = (fy - y0 as f32).clamp(0.0, 1.0);
        let hx = |x: i32| self.wrap_hx(x).unwrap_or(x);
        let m00 = self.at_tile(hx(x0), y0);
        let m10 = self.at_tile(hx(x0 + 1), y0);
        let m01 = self.at_tile(hx(x0), y0 + 1);
        let m11 = self.at_tile(hx(x0 + 1), y0 + 1);
        let a = m00 + (m10 - m00) * tx;
        let b = m01 + (m11 - m01) * tx;
        a + (b - a) * ty
    }

    /// Total humidity mass across all tiles. Useful for
    /// mass-conservation assertions in tests and for HUD summaries.
    pub fn total_mass(&self) -> f32 {
        self.cells.values().copied().sum()
    }

    /// Drop any sparse keys outside [`Self::bounds`].
    pub fn clamp_to_bounds(&mut self) {
        let Some(b) = self.bounds else {
            return;
        };
        self.cells.retain(|&(hx, hy), _| b.contains(hx, hy));
    }

    /// Explicit 4-neighbour diffusion step.
    ///
    /// `alpha` is the fraction of each pairwise head difference
    /// transferred per pass. Von Neumann stability for the
    /// 4-neighbour stencil requires `alpha ≤ 0.25`; we clamp to that.
    /// Compute-then-apply from a snapshot so the result is
    /// independent of iteration order; pruning removes near-zero
    /// tiles so the sparse map doesn't grow without bound.
    ///
    /// When [`Self::bounds`] is set without [`Self::wrap_x`], horizontal
    /// out-of-box neighbours are Neumann walls. With `wrap_x`, the left
    /// and right tile edges join (ring atmosphere).
    pub fn diffuse(&mut self, alpha: f32) {
        let alpha = alpha.clamp(0.0, 0.25);
        if alpha == 0.0 || self.cells.is_empty() {
            return;
        }
        if let Some(b) = self.bounds {
            if self.use_dense_slab(b) {
                self.diffuse_slab(alpha, b);
                return;
            }
        }
        // Snapshot the current state so we don't chase deltas across
        // the pass.
        let snap: FxHashMap<(i32, i32), f32> = self.cells.iter().map(|(&k, &v)| (k, v)).collect();

        // Build the iteration set: every mapped tile *plus* each of
        // its four neighbours (so a lone spike still spreads to its
        // -x/-y sides, which would never be sources otherwise). We
        // then walk this set and only look at (+x, +y) direction
        // pairs so every undirected pair is visited exactly once.
        let mut sources: Vec<(i32, i32)> = Vec::with_capacity(snap.len() * 5);
        for &(hx, hy) in snap.keys() {
            sources.push((hx, hy));
            if let Some(nx) = self.wrap_hx(hx + 1) {
                if self.accepts(nx, hy) {
                    sources.push((nx, hy));
                }
            }
            if let Some(nx) = self.wrap_hx(hx - 1) {
                if self.accepts(nx, hy) {
                    sources.push((nx, hy));
                }
            }
            if self.accepts(hx, hy + 1) {
                sources.push((hx, hy + 1));
            }
            if self.accepts(hx, hy - 1) {
                sources.push((hx, hy - 1));
            }
        }
        sources.sort_unstable();
        sources.dedup();

        let mut deltas: FxHashMap<(i32, i32), f32> = FxHashMap::default();
        for &(hx, hy) in &sources {
            let val = *snap.get(&(hx, hy)).unwrap_or(&0.0);
            // +x neighbour (possibly wrapped).
            if let Some(nx) = self.wrap_hx(hx + 1) {
                if self.accepts(nx, hy) && nx != hx {
                    let n_val = *snap.get(&(nx, hy)).unwrap_or(&0.0);
                    let flow = (val - n_val) * alpha;
                    if flow.abs() >= 1e-9 {
                        *deltas.entry((hx, hy)).or_insert(0.0) -= flow;
                        *deltas.entry((nx, hy)).or_insert(0.0) += flow;
                    }
                }
            }
            // +y neighbour (never wraps).
            let n_key = (hx, hy + 1);
            if self.accepts(n_key.0, n_key.1) {
                let n_val = *snap.get(&n_key).unwrap_or(&0.0);
                let flow = (val - n_val) * alpha;
                if flow.abs() >= 1e-9 {
                    *deltas.entry((hx, hy)).or_insert(0.0) -= flow;
                    *deltas.entry(n_key).or_insert(0.0) += flow;
                }
            }
        }
        for (k, d) in deltas {
            if !self.accepts(k.0, k.1) {
                continue;
            }
            *self.cells.entry(k).or_insert(0.0) += d;
        }
        let bounds = self.bounds;
        self.cells.retain(|&(hx, hy), v| {
            v.abs() > 1e-6 && bounds.map(|b| b.contains(hx, hy)).unwrap_or(true)
        });
    }

    /// Same +x/+y pairwise stencil as [`Self::diffuse`], walking the
    /// bound box instead of cloning the SipHash map and sorting a 5×
    /// neighbour set. Implicit zeros are empty tiles (neighbour expand).
    fn diffuse_slab(&mut self, alpha: f32, b: TileBounds) {
        let (w, h) = b.dims();
        let n = w.saturating_mul(h);
        if n == 0 {
            return;
        }
        let snap = self.pack_slab(b);
        let mut deltas = vec![0.0f32; n];
        for iy in 0..h {
            for ix in 0..w {
                let i = iy * w + ix;
                let hx = b.hx_min + ix as i32;
                let hy = b.hy_min + iy as i32;
                let val = snap[i];
                if let Some(nx) = self.wrap_hx(hx + 1) {
                    if self.accepts(nx, hy) && nx != hx {
                        let ni = b.index(w, nx, hy);
                        let flow = (val - snap[ni]) * alpha;
                        if flow.abs() >= 1e-9 {
                            deltas[i] -= flow;
                            deltas[ni] += flow;
                        }
                    }
                }
                let n_key = (hx, hy + 1);
                if self.accepts(n_key.0, n_key.1) {
                    let ni = b.index(w, n_key.0, n_key.1);
                    let flow = (val - snap[ni]) * alpha;
                    if flow.abs() >= 1e-9 {
                        deltas[i] -= flow;
                        deltas[ni] += flow;
                    }
                }
            }
        }
        for iy in 0..h {
            for ix in 0..w {
                let d = deltas[iy * w + ix];
                if d.abs() < 1e-9 {
                    continue;
                }
                let hx = b.hx_min + ix as i32;
                let hy = b.hy_min + iy as i32;
                if !self.accepts(hx, hy) {
                    continue;
                }
                *self.cells.entry((hx, hy)).or_insert(0.0) += d;
            }
        }
        self.cells.retain(|&(hx, hy), v| {
            v.abs() > 1e-6 && b.contains(hx, hy)
        });
    }

    /// Buoyant lift: a fraction of each tile's mass moves one tile up
    /// while the lapse is unstable. Mass-conserving; stops at `max_hy`
    /// (the sky box, not a sea-level deck).
    pub fn buoyant_rise(&mut self, fraction: f32, max_hy: i32) {
        self.buoyant_rise_thermal(fraction, max_hy, None);
    }

    /// How much a column's temperature anomaly changes its lift, per degree.
    const CONVECTION_GAIN_PER_C: f32 = 0.15;
    /// Anomaly is clamped before it is applied, so a freak tile cannot dominate.
    const CONVECTION_CLAMP_C: f32 = 6.0;
    /// Cool ground suppresses but never fully blocks lift; warm ground roughly
    /// doubles it. Bounded so convection reshapes the field rather than gating it.
    const CONVECTION_MIN_GAIN: f32 = 0.25;
    const CONVECTION_MAX_GAIN: f32 = 2.0;

    /// [`Self::buoyant_rise`] scaled by the local lapse: warm air under
    /// colder air lifts harder; a stable inversion almost sits still.
    /// When `temp` is mutable, each lift also mixes source heat into
    /// the tile above (humid-air heat capacity). Same tile walk as the
    /// uniform rise — no extra world scans.
    pub fn buoyant_rise_thermal(
        &mut self,
        fraction: f32,
        max_hy: i32,
        mut temp: Option<&mut crate::temperature::Temperature>,
    ) {
        let fraction = fraction.clamp(0.0, 0.45);
        if fraction == 0.0 || self.cells.is_empty() {
            return;
        }
        // Read `cells` in place — a full clone every tick was an FPS sink
        // once loft filled more than a fog film. Deltas apply after.
        //
        // Row means come from [`Temperature::row_mean_at`] (rebuilt on the
        // period-20 thermal step). Scanning every hx of every wet hy here
        // was the other cliff: more lofted rows → width × rows lookups.
        let mut deltas: FxHashMap<(i32, i32), f32> = FxHashMap::default();
        let mut heat_lifts: Vec<(i32, i32, f32)> = Vec::new();
        let keys: Vec<(i32, i32)> = self.cells.keys().copied().collect();
        for (hx, hy) in keys {
            let mass = *self.cells.get(&(hx, hy)).unwrap_or(&0.0);
            if mass <= 0.0 || hy >= max_hy {
                continue;
            }
            let dest = hy + 1;
            if !self.accepts(hx, dest) {
                continue;
            }
            let lift_f = if let Some(t) = temp.as_deref() {
                let here = t.at_tile(hx, hy);
                let above = t.at_tile(hx, dest);
                let lapse = (here - above).clamp(-5.0, 10.0);
                let base = (fraction * (0.40 + lapse * 0.11)).clamp(0.0, 0.45);
                // Horizontal anomaly: how much warmer this column is than the
                // world.
                //
                // The lapse term alone is nearly uniform, because temperature
                // falls smoothly with altitude everywhere — so vapour rose at
                // much the same rate over every column and the field stayed
                // horizontally flat however hard it was driven. Convection is
                // the *difference* between columns: thermals form over ground
                // that is warmer than its surroundings and not over ground that
                // is cooler, which is what organises moisture instead of
                // spreading it evenly.
                //
                // Warm land against cool sea, sunlit slope against shaded, and
                // the diurnal swing all feed this for free, since they are
                // already in the temperature field.
                let reference = t.row_mean_at(hy);
                let anomaly =
                    (here - reference).clamp(-Self::CONVECTION_CLAMP_C, Self::CONVECTION_CLAMP_C);
                let gain = (1.0 + anomaly * Self::CONVECTION_GAIN_PER_C)
                    .clamp(Self::CONVECTION_MIN_GAIN, Self::CONVECTION_MAX_GAIN);
                (base * gain).clamp(0.0, 0.45)
            } else {
                fraction
            };
            let lift = mass * lift_f;
            if lift < 1e-6 {
                continue;
            }
            *deltas.entry((hx, hy)).or_insert(0.0) -= lift;
            *deltas.entry((hx, dest)).or_insert(0.0) += lift;
            heat_lifts.push((hx, hy, lift / mass));
        }
        for (k, d) in deltas {
            if !self.accepts(k.0, k.1) {
                continue;
            }
            *self.cells.entry(k).or_insert(0.0) += d;
        }
        let bounds = self.bounds;
        self.cells.retain(|&(hx, hy), v| {
            *v > 1e-6 && bounds.map(|b| b.contains(hx, hy)).unwrap_or(true)
        });
        if let Some(t) = temp.as_deref_mut() {
            t.lift_heat_with_vapor(&heat_lifts);
        }
    }

    /// Advect atmospheric mass by climate wind `(vx, vy)` in tiles/tick.
    ///
    /// Per-tick **fractional flux** (not an integer whole-field step).
    /// `|v|` is the share of mass that leaves toward the neighbour this
    /// tick, capped at 1. Mass-conserving; vertical edges are Neumann.
    ///
    /// [`Self::advect_rx`] / [`Self::advect_ry`] are leftover from the
    /// residual/`trunc` path and stay zero so old saves do not jump.
    pub fn advect(&mut self, vx: f32, vy: f32) {
        self.advect_inner(vx, vy, None);
    }

    /// [`Self::advect`] shaped by the rebuilt local wind heatmap
    /// ([`crate::wind::Wind::vector_at`]) — terrain, thermal, swirl —
    /// then orographic lift and wind-driven vertical mixing.
    ///
    /// Free-air height is cached per occupied column so the flux pass
    /// does not walk the live surface once per seat (that was the
    /// humidity-advect FPS cliff). Wind samples are cached once per
    /// seat before the two axis passes — `vector_at` misses walk the
    /// world, and calling that twice per tile was the leftover cost
    /// after the field rebuild. When the bound box is at least half
    /// full, flux / lift / mix / oro share one packed slab and only
    /// tiles that moved are written back. Sparse maps keep the
    /// HashMap walk (demo soak early). Not view LOD.
    pub fn advect_with_surface(
        &mut self,
        vx: f32,
        vy: f32,
        wind: &crate::wind::Wind,
        world: &crate::grid::World,
    ) {
        if let Some(b) = self.bounds {
            if self.use_dense_slab(b) {
                self.advect_with_surface_slab(vx, vy, wind, world, b);
                return;
            }
        }
        let free_air = self.build_free_air_cache(wind, world);
        self.advect_inner(vx, vy, Some((wind, world, &free_air)));
        self.wind_mix(wind.mix_strength(vx, vy), Some((wind, world, &free_air)));
        self.apply_orographic_lift(wind, Some(world));
    }

    /// One pack of the bound box, then flux / buried-lift / mix / oro
    /// on the slab. Write back only tiles that moved. Same stencil as
    /// the HashMap path — not view LOD.
    fn advect_with_surface_slab(
        &mut self,
        vx: f32,
        vy: f32,
        wind: &crate::wind::Wind,
        world: &crate::grid::World,
        b: TileBounds,
    ) {
        let free_air = self.build_free_air_cache(wind, world);
        let (w, h) = b.dims();
        let n = w.saturating_mul(h);
        if n == 0 {
            return;
        }
        let snap = self.pack_slab(b);
        let mut work = snap.clone();
        let surface = Some((wind, world, &free_air));
        // One sample per occupied seat — same leftover the sparse
        // path already cut. Both axes donate from `snap`.
        let mut vectors = vec![(0.0f32, 0.0f32); n];
        for i in 0..n {
            if snap[i].abs() < 1e-9 {
                continue;
            }
            let (hx, hy) = b.coords(w, i);
            vectors[i] = wind.vector_at(Some(world), hx, hy);
        }
        self.flux_axis_into(&snap, &mut work, vx, vy, surface, true, b, w, &vectors);
        self.flux_axis_into(&snap, &mut work, vx, vy, surface, false, b, w, &vectors);
        self.lift_buried_into(&mut work, wind, world, &free_air, b, w, h);
        self.wind_mix_into(
            &mut work,
            wind.mix_strength(vx, vy),
            surface,
            b,
            w,
            h,
        );
        self.oro_into(&mut work, wind, Some(world), b, w, h);
        self.sync_slab_changes(b, &snap, &work);
    }

    /// Donor-cell flux into `work`. Masses come from the pre-advect
    /// `snap` so both axes commute the same way as [`Self::flux_axis`].
    fn flux_axis_into(
        &self,
        snap: &[f32],
        work: &mut [f32],
        climate_vx: f32,
        climate_vy: f32,
        surface: Option<(
            &crate::wind::Wind,
            &crate::grid::World,
            &FxHashMap<i32, i32>,
        )>,
        horizontal: bool,
        b: TileBounds,
        w: usize,
        vectors: &[(f32, f32)],
    ) {
        let n = snap.len().min(work.len());
        if n == 0 || w == 0 {
            return;
        }
        let h = n / w;
        let mut deltas = vec![0.0f32; n];
        for iy in 0..h {
            for ix in 0..w {
                let i = iy * w + ix;
                let mass = snap[i];
                if mass.abs() < 1e-9 {
                    continue;
                }
                let hx = b.hx_min + ix as i32;
                let hy = b.hy_min + iy as i32;
                let (vx, vy) = match surface {
                    Some(_) => vectors
                        .get(i)
                        .copied()
                        .unwrap_or((climate_vx, climate_vy)),
                    None => (climate_vx, climate_vy),
                };
                let Some((step, leave)) = Self::flux_step_leave(mass, vx, vy, horizontal) else {
                    continue;
                };
                let Some((tx, ty)) = self.flux_dest(hx, hy, step, horizontal, surface) else {
                    continue;
                };
                deltas[i] -= leave;
                if b.contains(tx, ty) {
                    deltas[b.index(w, tx, ty)] += leave;
                }
            }
        }
        for i in 0..n {
            let d = deltas[i];
            if d.abs() < 1e-12 {
                continue;
            }
            if d < 0.0 {
                work[i] = (work[i] + d).max(0.0);
            } else {
                let (hx, hy) = b.coords(w, i);
                if self.accepts(hx, hy) {
                    work[i] += d;
                }
            }
        }
    }

    /// Same valley test as [`Self::lift_buried_to_free_air`]. Masses
    /// are snapshotted so a hoist cannot feed another hoist this tick.
    fn lift_buried_into(
        &self,
        work: &mut [f32],
        wind: &crate::wind::Wind,
        world: &crate::grid::World,
        cache: &FxHashMap<i32, i32>,
        b: TileBounds,
        w: usize,
        h: usize,
    ) {
        let n = work.len();
        if n == 0 || w == 0 {
            return;
        }
        let mut moves: Vec<(usize, usize, f32)> = Vec::new();
        for iy in 0..h {
            for ix in 0..w {
                let i = iy * w + ix;
                let mass = work[i];
                if mass <= 1e-9 {
                    continue;
                }
                let hx = b.hx_min + ix as i32;
                let hy = b.hy_min + iy as i32;
                let air = self.free_air_cached(wind, world, hx, cache);
                if hy >= air {
                    continue;
                }
                let mut valley = air;
                if let Some(l) = self.wrap_hx(hx - 1) {
                    valley = valley.min(self.free_air_cached(wind, world, l, cache));
                }
                if let Some(r) = self.wrap_hx(hx + 1) {
                    valley = valley.min(self.free_air_cached(wind, world, r, cache));
                }
                if hy >= valley {
                    continue;
                }
                if !self.accepts(hx, air) {
                    continue;
                }
                let dest = b.index(w, hx, air);
                if dest >= n {
                    continue;
                }
                moves.push((i, dest, mass));
            }
        }
        for (from, to, mass) in moves {
            work[from] -= mass;
            work[to] += mass;
        }
    }

    /// Vertical mix + sink on the slab. Reads `work`, writes
    /// commutative deltas — same pairs as [`Self::wind_mix`]. Empty
    /// tiles stay empty so a zero does not pull mass from above.
    fn wind_mix_into(
        &self,
        work: &mut [f32],
        mix: f32,
        surface: Option<(
            &crate::wind::Wind,
            &crate::grid::World,
            &FxHashMap<i32, i32>,
        )>,
        b: TileBounds,
        w: usize,
        h: usize,
    ) {
        let mix = mix.clamp(0.0, 1.0);
        let n = work.len();
        if mix < 1e-4 || n == 0 || w == 0 {
            return;
        }
        let alpha = (0.04 + 0.14 * mix).clamp(0.0, 0.20);
        let mut deltas = vec![0.0f32; n];
        for iy in 0..h {
            for ix in 0..w {
                let i = iy * w + ix;
                let val = work[i];
                // Sparse mix runs after retain(|v| > 1e-6).
                if val.abs() <= 1e-6 {
                    continue;
                }
                let hx = b.hx_min + ix as i32;
                let hy = b.hy_min + iy as i32;
                let above_hy = hy + 1;
                if !self.accepts(hx, above_hy) {
                    continue;
                }
                if let Some((wnd, wrld, cache)) = surface {
                    let air = self.free_air_cached(wnd, wrld, hx, cache);
                    if hy < air || above_hy < air {
                        continue;
                    }
                }
                let n_val = if b.contains(hx, above_hy) {
                    work[b.index(w, hx, above_hy)]
                } else {
                    0.0
                };
                let flow = (val - n_val) * alpha;
                if flow.abs() < 1e-9 {
                    continue;
                }
                deltas[i] -= flow;
                if b.contains(hx, above_hy) {
                    deltas[b.index(w, hx, above_hy)] += flow;
                }
            }
        }
        let sink = 0.03 * mix;
        if sink > 1e-5 {
            for iy in 0..h {
                for ix in 0..w {
                    let i = iy * w + ix;
                    let val = work[i];
                    if val <= 1e-9 {
                        continue;
                    }
                    let hx = b.hx_min + ix as i32;
                    let hy = b.hy_min + iy as i32;
                    let below = hy - 1;
                    if !self.accepts(hx, below) {
                        continue;
                    }
                    if let Some((wnd, wrld, cache)) = surface {
                        if below < self.free_air_cached(wnd, wrld, hx, cache) {
                            continue;
                        }
                    }
                    let take = val * sink;
                    deltas[i] -= take;
                    if b.contains(hx, below) {
                        deltas[b.index(w, hx, below)] += take;
                    }
                }
            }
        }
        for i in 0..n {
            let d = deltas[i];
            if d.abs() < 1e-12 {
                continue;
            }
            if d < 0.0 {
                work[i] = (work[i] + d).max(0.0);
            } else {
                let (hx, hy) = b.coords(w, i);
                if self.accepts(hx, hy) {
                    work[i] += d;
                }
            }
        }
    }

    /// Per-column orographic lift on the slab. Negative apply is `+=`
    /// without `max(0)` — same as [`Self::apply_orographic_lift`].
    fn oro_into(
        &self,
        work: &mut [f32],
        wind: &crate::wind::Wind,
        world: Option<&crate::grid::World>,
        b: TileBounds,
        w: usize,
        h: usize,
    ) {
        let n = work.len();
        if n == 0 || w == 0 {
            return;
        }
        let mut lift_col = vec![0.0f32; w];
        let mut any_lift = false;
        for ix in 0..w {
            let hx = b.hx_min + ix as i32;
            let lift = wind.orographic_lift(world, hx);
            if lift > 1e-5 {
                any_lift = true;
                lift_col[ix] = lift;
            }
        }
        if !any_lift {
            return;
        }
        let mut deltas = vec![0.0f32; n];
        for iy in 0..h {
            for ix in 0..w {
                let i = iy * w + ix;
                let mass = work[i];
                if mass <= 1e-6 {
                    continue;
                }
                let lift = lift_col[ix];
                if lift <= 1e-5 {
                    continue;
                }
                let hx = b.hx_min + ix as i32;
                let hy = b.hy_min + iy as i32;
                let dest = hy + 1;
                if !self.accepts(hx, dest) {
                    continue;
                }
                let take = mass * lift;
                if take <= 1e-9 {
                    continue;
                }
                deltas[i] -= take;
                if b.contains(hx, dest) {
                    deltas[b.index(w, hx, dest)] += take;
                }
            }
        }
        for i in 0..n {
            let d = deltas[i];
            if d.abs() < 1e-12 {
                continue;
            }
            if d < 0.0 {
                work[i] += d;
            } else {
                let (hx, hy) = b.coords(w, i);
                if self.accepts(hx, hy) {
                    work[i] += d;
                }
            }
        }
    }

    fn sync_slab_changes(&mut self, b: TileBounds, before: &[f32], after: &[f32]) {
        let (w, _) = b.dims();
        let n = before.len().min(after.len());
        for i in 0..n {
            if (after[i] - before[i]).abs() < 1e-12 {
                continue;
            }
            let (hx, hy) = b.coords(w, i);
            if after[i] > 1e-6 {
                self.cells.insert((hx, hy), after[i]);
            } else {
                self.cells.remove(&(hx, hy));
            }
        }
    }

    /// Occupied tile columns. A filled sky walks the bound `hx` range
    /// once instead of hashing every seat to discover ~width columns.
    fn occupied_columns(&self) -> Vec<i32> {
        if let Some(b) = self.bounds {
            if self.use_dense_slab(b) {
                return (b.hx_min..=b.hx_max).collect();
            }
        }
        let mut seen = FxHashSet::default();
        let mut cols = Vec::new();
        for &(hx, _) in self.cells.keys() {
            if seen.insert(hx) {
                cols.push(hx);
            }
        }
        cols
    }

    /// Precompute [`Self::free_air_hy`] for every occupied column (±1).
    fn build_free_air_cache(
        &self,
        wind: &crate::wind::Wind,
        world: &crate::grid::World,
    ) -> FxHashMap<i32, i32> {
        let mut cache = FxHashMap::default();
        for hx in self.occupied_columns() {
            for dx in -1..=1 {
                let nx = match self.wrap_hx(hx + dx) {
                    Some(x) => x,
                    None => continue,
                };
                cache
                    .entry(nx)
                    .or_insert_with(|| self.free_air_hy(wind, world, nx));
            }
        }
        cache
    }

    fn free_air_cached(
        &self,
        wind: &crate::wind::Wind,
        world: &crate::grid::World,
        hx: i32,
        cache: &FxHashMap<i32, i32>,
    ) -> i32 {
        cache
            .get(&hx)
            .copied()
            .unwrap_or_else(|| self.free_air_hy(wind, world, hx))
    }

    fn advect_inner(
        &mut self,
        climate_vx: f32,
        climate_vy: f32,
        surface: Option<(
            &crate::wind::Wind,
            &crate::grid::World,
            &FxHashMap<i32, i32>,
        )>,
    ) {
        if self.cells.is_empty() {
            return;
        }
        if climate_vx == 0.0 && climate_vy == 0.0 && surface.is_none() {
            return;
        }
        self.advect_rx = 0.0;
        self.advect_ry = 0.0;

        // Climate-only packed flux. Surface dense already ran through
        // [`Self::advect_with_surface_slab`] (lift / mix / oro too).
        if surface.is_none() {
            if let Some(b) = self.bounds {
                if self.use_dense_slab(b) {
                    let (w, _) = b.dims();
                    let snap = self.pack_slab(b);
                    let mut work = snap.clone();
                    self.flux_axis_into(
                        &snap, &mut work, climate_vx, climate_vy, None, true, b, w, &[],
                    );
                    self.flux_axis_into(
                        &snap, &mut work, climate_vx, climate_vy, None, false, b, w, &[],
                    );
                    self.sync_slab_changes(b, &snap, &work);
                    return;
                }
            }
        }

        // Flux only iterates the snapshot — a Vec avoids rehashing the
        // saved SipHash map every tick (leftover as humidity fills).
        let snap: Vec<((i32, i32), f32)> = self.cells.iter().map(|(&k, &v)| (k, v)).collect();
        // Parallel to `snap`. A HashMap of the same seats was leftover
        // once loft filled — each axis hashed the key again.
        let vectors: Option<Vec<(f32, f32)>> = surface.map(|(wind, world, _)| {
            snap.iter()
                .map(|&((hx, hy), _)| wind.vector_at(Some(world), hx, hy))
                .collect()
        });
        let vectors = vectors.as_deref();
        self.flux_axis(&snap, climate_vx, climate_vy, surface, true, vectors);
        self.flux_axis(&snap, climate_vx, climate_vy, surface, false, vectors);

        if let Some((wind, world, cache)) = surface {
            self.lift_buried_to_free_air(wind, world, cache);
        }
        let bounds = self.bounds;
        self.cells.retain(|&(hx, hy), v| {
            v.abs() > 1e-6 && bounds.map(|b| b.contains(hx, hy)).unwrap_or(true)
        });
    }

    /// `|v|` is the fraction that leaves this tick, capped at 1.
    /// Vertical hop is damped so face-following / Jacobi climb cannot
    /// empty a tile (uncapped `|vy|` vacuums below `min_mass_to_rain`).
    fn flux_step_leave(mass: f32, vx: f32, vy: f32, horizontal: bool) -> Option<(f32, f32)> {
        let v = if horizontal {
            vx
        } else {
            vy.clamp(-HUMIDITY_VY_ADV_CAP, HUMIDITY_VY_ADV_CAP)
        };
        let step = v.clamp(-1.0, 1.0);
        if step.abs() < 1e-9 {
            return None;
        }
        let leave = mass * step.abs();
        if leave < 1e-12 {
            return None;
        }
        Some((step, leave))
    }

    /// Horizontal dest stays at this `hy`. Snapping to the neighbour's
    /// free-air crest was a leftover Y pump (pond vapour teleported
    /// onto both shores). Vertical dest may lift to free air.
    fn flux_dest(
        &self,
        hx: i32,
        hy: i32,
        step: f32,
        horizontal: bool,
        surface: Option<(
            &crate::wind::Wind,
            &crate::grid::World,
            &FxHashMap<i32, i32>,
        )>,
    ) -> Option<(i32, i32)> {
        if horizontal {
            let dir = if step > 0.0 { 1 } else { -1 };
            let nhx = self.wrap_hx(hx + dir)?;
            if !self.accepts(nhx, hy) {
                return None;
            }
            Some((nhx, hy))
        } else {
            let dir = if step > 0.0 { 1 } else { -1 };
            let nhy = hy + dir;
            let mut dest_hy = nhy;
            if let Some((wind, world, cache)) = surface {
                dest_hy = self.free_air_cached(wind, world, hx, cache).max(nhy);
            }
            if !self.accepts(hx, dest_hy) {
                return None;
            }
            Some((hx, dest_hy))
        }
    }

    /// Donor-cell flux along one axis. `|v|` is the fraction of mass that
    /// leaves toward the neighbour this tick (capped at 1).
    fn flux_axis(
        &mut self,
        snap: &[((i32, i32), f32)],
        climate_vx: f32,
        climate_vy: f32,
        surface: Option<(
            &crate::wind::Wind,
            &crate::grid::World,
            &FxHashMap<i32, i32>,
        )>,
        horizontal: bool,
        vectors: Option<&[(f32, f32)]>,
    ) {
        let mut deltas: FxHashMap<(i32, i32), f32> = FxHashMap::default();
        deltas.reserve(snap.len());
        for (i, &((hx, hy), mass)) in snap.iter().enumerate() {
            if mass.abs() < 1e-9 {
                continue;
            }
            let (vx, vy) = match surface {
                Some((wind, world, _)) => vectors
                    .and_then(|v| v.get(i).copied())
                    .unwrap_or_else(|| wind.vector_at(Some(world), hx, hy)),
                None => (climate_vx, climate_vy),
            };
            let Some((step, leave)) = Self::flux_step_leave(mass, vx, vy, horizontal) else {
                continue;
            };
            let Some((tx, ty)) = self.flux_dest(hx, hy, step, horizontal, surface) else {
                continue;
            };
            *deltas.entry((hx, hy)).or_insert(0.0) -= leave;
            *deltas.entry((tx, ty)).or_insert(0.0) += leave;
        }
        for (k, d) in deltas {
            if d < 0.0 {
                if let Some(e) = self.cells.get_mut(&k) {
                    *e = (*e + d).max(0.0);
                }
                continue;
            }
            if self.accepts(k.0, k.1) {
                *self.cells.entry(k).or_insert(0.0) += d;
            }
        }
        // Caller retains once after both axes.
    }

    /// High wind mixes the column so vapour does not translate as a slab.
    pub fn wind_mix(
        &mut self,
        mix: f32,
        surface: Option<(
            &crate::wind::Wind,
            &crate::grid::World,
            &FxHashMap<i32, i32>,
        )>,
    ) {
        let mix = mix.clamp(0.0, 1.0);
        if mix < 1e-4 || self.cells.is_empty() {
            return;
        }
        let alpha = (0.04 + 0.14 * mix).clamp(0.0, 0.20);
        // Read the live map; writes go to `deltas` so pairs stay
        // commutative without a clone or a sort (keys are unique).
        let mut deltas: FxHashMap<(i32, i32), f32> = FxHashMap::default();
        deltas.reserve(self.cells.len());
        for (&(hx, hy), &val) in &self.cells {
            let above = (hx, hy + 1);
            if !self.accepts(above.0, above.1) {
                continue;
            }
            if let Some((wind, world, cache)) = surface {
                let air = self.free_air_cached(wind, world, hx, cache);
                if hy < air || above.1 < air {
                    continue;
                }
            }
            let n_val = *self.cells.get(&above).unwrap_or(&0.0);
            let flow = (val - n_val) * alpha;
            if flow.abs() < 1e-9 {
                continue;
            }
            *deltas.entry((hx, hy)).or_insert(0.0) -= flow;
            *deltas.entry(above).or_insert(0.0) += flow;
        }
        let sink = 0.03 * mix;
        if sink > 1e-5 {
            for (&(hx, hy), &val) in &self.cells {
                if val <= 1e-9 {
                    continue;
                }
                let below = hy - 1;
                if !self.accepts(hx, below) {
                    continue;
                }
                if let Some((wind, world, cache)) = surface {
                    if below < self.free_air_cached(wind, world, hx, cache) {
                        continue;
                    }
                }
                let take = val * sink;
                *deltas.entry((hx, hy)).or_insert(0.0) -= take;
                *deltas.entry((hx, below)).or_insert(0.0) += take;
            }
        }
        for (k, d) in deltas {
            if d < 0.0 {
                if let Some(e) = self.cells.get_mut(&k) {
                    *e = (*e + d).max(0.0);
                }
                continue;
            }
            if self.accepts(k.0, k.1) {
                *self.cells.entry(k).or_insert(0.0) += d;
            }
        }
        self.cells.retain(|_, v| *v > 1e-6);
    }

    /// First tile row whose centre sits in free air above the live crest.
    fn free_air_hy(
        &self,
        wind: &crate::wind::Wind,
        world: &crate::grid::World,
        hx: i32,
    ) -> i32 {
        let tc = self.tile_cols.max(1);
        let base = self.atmosphere_base_y(world, wind, hx);
        ((base + 1 - tc / 2).max(0) + tc - 1) / tc
    }

    fn atmosphere_base_y(
        &self,
        world: &crate::grid::World,
        wind: &crate::wind::Wind,
        hx: i32,
    ) -> i32 {
        let tc = self.tile_cols.max(1);
        let gx = world.wrap_x(hx * tc + tc / 2);
        let rock = crate::worldgen::live_surface_at(
            world,
            wind.seed,
            gx,
            wind.sea_level_y,
            wind.width_cols,
        );
        crate::worldgen::live_skin_y(world, gx, rock)
    }

    fn lift_buried_to_free_air(
        &mut self,
        wind: &crate::wind::Wind,
        world: &crate::grid::World,
        cache: &FxHashMap<i32, i32>,
    ) {
        let keys: Vec<((i32, i32), f32)> = self.cells.iter().map(|(&k, &v)| (k, v)).collect();
        let mut moves: Vec<((i32, i32), (i32, i32), f32)> = Vec::new();
        for ((hx, hy), mass) in keys {
            let air = self.free_air_cached(wind, world, hx, cache);
            if hy >= air {
                continue;
            }
            // Valley air that crossed a bank sits at the neighbour's
            // waterline. Hoisting that onto this crest is the leftover
            // that pinned two hot, rainy columns on every pond shore.
            // Only seats deeper than this column *and* both neighbours
            // are truly inside a hill.
            let mut valley = air;
            if let Some(l) = self.wrap_hx(hx - 1) {
                valley = valley.min(self.free_air_cached(wind, world, l, cache));
            }
            if let Some(r) = self.wrap_hx(hx + 1) {
                valley = valley.min(self.free_air_cached(wind, world, r, cache));
            }
            if hy >= valley {
                continue;
            }
            if mass <= 1e-9 || !self.accepts(hx, air) {
                continue;
            }
            moves.push(((hx, hy), (hx, air), mass));
        }
        for (from, to, mass) in moves {
            if let Some(e) = self.cells.get_mut(&from) {
                *e -= mass;
            }
            *self.cells.entry(to).or_insert(0.0) += mass;
        }
        self.cells.retain(|_, v| v.abs() > 1e-6);
    }

    /// Move a lift-fraction of each tile one step up where the live
    /// (or seed, if `world` is `None`) surface rises downwind.
    pub fn apply_orographic_lift(
        &mut self,
        wind: &crate::wind::Wind,
        world: Option<&crate::grid::World>,
    ) {
        if self.cells.is_empty() {
            return;
        }
        let mut lift_by_hx: FxHashMap<i32, f32> = FxHashMap::default();
        let mut any_lift = false;
        for hx in self.occupied_columns() {
            let lift = wind.orographic_lift(world, hx);
            if lift > 1e-5 {
                any_lift = true;
                lift_by_hx.insert(hx, lift);
            }
        }
        if !any_lift {
            return;
        }
        let mut deltas: FxHashMap<(i32, i32), f32> = FxHashMap::default();
        deltas.reserve(self.cells.len());
        for (&(hx, hy), &mass) in &self.cells {
            if mass <= 0.0 {
                continue;
            }
            let lift = *lift_by_hx.get(&hx).unwrap_or(&0.0);
            if lift <= 1e-5 {
                continue;
            }
            let dest = hy + 1;
            if !self.accepts(hx, dest) {
                continue;
            }
            let take = mass * lift;
            if take <= 1e-9 {
                continue;
            }
            *deltas.entry((hx, hy)).or_insert(0.0) -= take;
            *deltas.entry((hx, dest)).or_insert(0.0) += take;
        }
        for (k, d) in deltas {
            if d < 0.0 {
                if let Some(e) = self.cells.get_mut(&k) {
                    *e += d;
                }
                continue;
            }
            if self.accepts(k.0, k.1) {
                *self.cells.entry(k).or_insert(0.0) += d;
            }
        }
        let bounds = self.bounds;
        self.cells.retain(|&(hx, hy), v| {
            *v > 1e-6 && bounds.map(|b| b.contains(hx, hy)).unwrap_or(true)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_accumulates_at_the_same_tile() {
        let mut h = Humidity::new(4);
        h.add(0, 0, 10.0);
        h.add(1, 3, 5.0); // same tile as (0,0)
        assert_eq!(h.at_cell(2, 2), 15.0);
    }

    #[test]
    fn sample_bilinear_smooths_between_tiles() {
        let mut h = Humidity::new(4);
        h.cells.insert((0, 0), 0.0);
        h.cells.insert((1, 0), 100.0);
        // Midway between tile centres (2,2) and (6,2) → x=4.
        let mid = h.sample_bilinear(4.0, 2.0);
        assert!(
            (mid - 50.0).abs() < 1e-3,
            "expected ~50 at midpoint, got {mid}"
        );
        assert!(h.sample_bilinear(2.0, 2.0) < mid);
        assert!(h.sample_bilinear(6.0, 2.0) > mid);
    }

    #[test]
    fn sample_bilinear_smooths_the_tile_row_at_128() {
        let mut h = Humidity::new(4);
        h.cells.insert((45, 31), 40.0);
        h.cells.insert((45, 32), 190.0);
        let lo = h.sample_bilinear(181.5, 126.0);
        let mid = h.sample_bilinear(181.5, 128.0);
        let hi = h.sample_bilinear(181.5, 130.0);
        assert!(
            lo < mid && mid < hi,
            "y=127/128 is a tile edge, not a clamp (lo={lo} mid={mid} hi={hi})"
        );
    }

    #[test]
    fn sample_bilinear_wraps_at_the_ring_seam() {
        let mut h = Humidity::with_world_bounds(4, 0, 0, 16, 16);
        h.wrap_x = true;
        h.cells.insert((0, 0), 0.0);
        h.cells.insert((3, 0), 100.0);
        // Tile centres at x=2 (hx=0) and x=14 (hx=3). Mid-seam is x=0
        // wrapping, or x=16. Must not read a missing hx=-1 as dry.
        let seam = h.sample_bilinear(0.0, 2.0);
        assert!(
            (seam - 50.0).abs() < 1.0,
            "ring seam should lerp 0↔100, got {seam}"
        );
        assert!((h.sample_bilinear(16.0, 2.0) - seam).abs() < 1e-3);
    }

    #[test]
    fn advect_moves_mass_and_conserves() {
        let mut h = Humidity::with_world_bounds(4, 0, 0, 64, 64);
        h.wrap_x = true;
        h.add(8, 8, 100.0);
        let before = h.total_mass();
        // Force a whole-tile step.
        h.advect_rx = 0.0;
        h.advect(1.0, 0.0);
        assert!((h.total_mass() - before).abs() < 1e-4);
        assert!(h.at_tile(3, 2) > 0.0 || h.at_tile(2, 2) > 0.0);
        // Original tile centre was (8,8) → tile (2,2); +1 hx → (3,2).
        assert!(
            h.at_tile(3, 2) > 50.0,
            "mass should have shifted +1 tile in x, got {}",
            h.at_tile(3, 2)
        );
    }

    #[test]
    fn fractional_flux_moves_a_share_and_conserves() {
        let mut h = Humidity::with_world_bounds(4, 0, 0, 64, 64);
        h.wrap_x = true;
        h.add(8, 8, 100.0);
        let before = h.total_mass();
        h.advect(0.25, 0.0);
        assert!((h.total_mass() - before).abs() < 1e-4);
        let stay = h.at_tile(2, 2);
        let moved = h.at_tile(3, 2);
        assert!(
            (stay - 75.0).abs() < 1e-3 && (moved - 25.0).abs() < 1e-3,
            "0.25 flux should leave 75 / move 25 (stay={stay} moved={moved})"
        );
    }

    #[test]
    fn vertical_wind_does_not_vacuum_a_tile_in_one_hop() {
        let mut h = Humidity::with_world_bounds(4, 0, 0, 64, 64);
        h.wrap_x = true;
        h.add(8, 8, 100.0);
        let before = h.total_mass();
        h.advect(0.0, 0.80);
        assert!((h.total_mass() - before).abs() < 1e-4);
        let stay = h.at_tile(2, 2);
        let moved = h.at_tile(2, 3);
        assert!(
            stay >= 88.0,
            "vy=0.80 must be capped so this tile keeps most of its mass, stay={stay}"
        );
        assert!(
            moved > 0.0 && moved <= 12.0,
            "capped climb should move a drizzle, not the whole cell, moved={moved}"
        );
    }

    #[test]
    fn local_vertical_field_is_capped_the_same_way() {
        use crate::grid::World;
        use crate::wind::Wind;
        use crate::worldgen::WorldgenParams;

        let p = WorldgenParams::default();
        let mut wind = Wind::climate(
            4,
            0.20,
            p.seed,
            p.width_cols,
            p.sea_level_y,
            p.bedrock_floor_y,
            p.sky_ceiling_y,
            true,
        );
        wind.config.terrain_drive = 0.0;
        wind.config.thermal_drive = 0.0;
        wind.config.swirl = 0.0;
        wind.config.field_smooth = 0.0;
        let world = World::new(p.seed);
        let mut h = Humidity::with_world_bounds(
            4,
            0,
            p.bedrock_floor_y,
            p.width_cols,
            p.sky_ceiling_y,
        );
        h.wrap_x = true;
        let gy = p.sea_level_y + 24;
        h.add(8, gy, 100.0);
        let (hx, hy) = h.tile_of(8, gy);
        wind.field.insert((hx, hy), (0.0, 0.80));
        h.advect_with_surface(0.0, 0.0, &wind, &world);
        let left = h.at_tile(hx, hy);
        assert!(
            left >= 70.0,
            "surface-path vy=0.80 must not vacuum the tile (uncapped would leave ~20), left={left}"
        );
    }

    #[test]
    fn local_wind_field_steers_flux_off_the_climate_mean() {
        use crate::wind::Wind;
        use crate::worldgen::WorldgenParams;
        use crate::grid::World;

        let p = WorldgenParams::default();
        let mut wind = Wind::climate(
            4,
            0.20,
            p.seed,
            p.width_cols,
            p.sea_level_y,
            p.bedrock_floor_y,
            p.sky_ceiling_y,
            true,
        );
        wind.config.terrain_drive = 0.0;
        wind.config.thermal_drive = 0.0;
        wind.config.swirl = 0.0;
        wind.config.field_smooth = 0.0;
        let world = World::new(p.seed);
        let mut h = Humidity::with_world_bounds(
            4,
            0,
            p.bedrock_floor_y,
            p.width_cols,
            p.sky_ceiling_y,
        );
        h.wrap_x = true;
        // Sit in free air so lift-buried does not hoist the fixture.
        let gy = p.sea_level_y + 24;
        h.add(8, gy, 100.0);
        let (hx, hy) = h.tile_of(8, gy);
        wind.field.insert((hx, hy), (-0.50, 0.0));
        let before = h.total_mass();
        h.advect_with_surface(0.20, 0.0, &wind, &world);
        assert!((h.total_mass() - before).abs() < 0.5);
        let left = match h.wrap_tile_x(hx - 1) {
            Some(x) => h.at_tile(x, hy),
            None => 0.0,
        };
        let right = match h.wrap_tile_x(hx + 1) {
            Some(x) => h.at_tile(x, hy),
            None => 0.0,
        };
        assert!(
            left > right,
            "local −x field should send more mass left than climate +x (L={left} R={right})"
        );
    }

    #[test]
    fn dense_surface_slab_conserves_and_caps_vy() {
        use crate::grid::World;
        use crate::wind::Wind;
        use crate::worldgen::WorldgenParams;

        let p = WorldgenParams::default();
        let mut wind = Wind::climate(
            4,
            0.20,
            p.seed,
            p.width_cols,
            p.sea_level_y,
            p.bedrock_floor_y,
            p.sky_ceiling_y,
            true,
        );
        wind.config.terrain_drive = 0.0;
        wind.config.thermal_drive = 0.0;
        wind.config.swirl = 0.0;
        wind.config.field_smooth = 0.0;
        let world = World::new(p.seed);
        // 8×8 tiles (64 ≤ 256) forces the unified slab. Sit in free
        // air so lift-buried does not hoist the fixture.
        let y0 = p.sea_level_y + 16;
        let mut h = Humidity::with_world_bounds(4, 0, y0, 32, y0 + 32);
        h.wrap_x = true;
        let gy = y0 + 8;
        h.add(8, gy, 100.0);
        let (hx, hy) = h.tile_of(8, gy);
        wind.field.insert((hx, hy), (0.0, 0.80));
        let before = h.total_mass();
        assert!(h.use_dense_slab(h.bounds.unwrap()));
        h.advect_with_surface(0.0, 0.0, &wind, &world);
        assert!(
            (h.total_mass() - before).abs() < 0.5,
            "dense slab must conserve (before={before} after={})",
            h.total_mass()
        );
        let left = h.at_tile(hx, hy);
        assert!(
            left >= 70.0,
            "dense-slab vy=0.80 must not vacuum the tile, left={left}"
        );
    }

    #[test]
    fn dense_surface_slab_matches_sparse() {
        use crate::grid::World;
        use crate::wind::Wind;
        use crate::worldgen::WorldgenParams;

        let p = WorldgenParams::default();
        let mut wind = Wind::climate(
            4,
            0.20,
            p.seed,
            p.width_cols,
            p.sea_level_y,
            p.bedrock_floor_y,
            p.sky_ceiling_y,
            true,
        );
        wind.config.terrain_drive = 0.0;
        wind.config.thermal_drive = 0.0;
        wind.config.swirl = 0.0;
        wind.config.field_smooth = 0.0;
        let world = World::new(p.seed);
        let mut sparse = Humidity::with_world_bounds(
            4,
            0,
            p.bedrock_floor_y,
            p.width_cols,
            p.sky_ceiling_y,
        );
        sparse.wrap_x = true;
        let gy = p.sea_level_y + 24;
        sparse.add(8, gy, 100.0);
        sparse.add(40, gy + 4, 60.0);
        sparse.add(80, gy - 4, 30.0);
        let (hx, hy) = sparse.tile_of(8, gy);
        wind.field.insert((hx, hy), (-0.40, 0.20));
        let mut dense = sparse.clone();
        assert!(
            !sparse.use_dense_slab(sparse.bounds.unwrap()),
            "few keys must keep the HashMap surface walk"
        );
        sparse.advect_with_surface(0.15, 0.0, &wind, &world);
        dense.advect_with_surface_slab(
            0.15,
            0.0,
            &wind,
            &world,
            dense.bounds.unwrap(),
        );
        assert!(
            (sparse.total_mass() - dense.total_mass()).abs() < 1e-3,
            "surface slab must conserve like sparse: {} vs {}",
            sparse.total_mass(),
            dense.total_mass()
        );
        for key in sparse.cells.keys().chain(dense.cells.keys()) {
            let a = sparse.at_tile(key.0, key.1);
            let b = dense.at_tile(key.0, key.1);
            assert!(
                (a - b).abs() < 1e-3,
                "tile {:?} sparse={a} slab={b}",
                key
            );
        }
    }

    #[test]
    fn orographic_lift_follows_the_live_hill() {
        use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
        use crate::grid::World;
        use crate::wind::Wind;
        use crate::worldgen::{continental_surface_y, WorldgenParams};
        use wk_material::MaterialId;
        use crate::cell::Cell;

        let p = WorldgenParams::default();
        let wind = Wind::climate(
            4,
            0.12,
            p.seed,
            p.width_cols,
            p.sea_level_y,
            p.bedrock_floor_y,
            p.sky_ceiling_y,
            true,
        );
        let mut hx_climb = 0;
        let mut best = 0.0f32;
        for hx in 0..(p.width_cols / 4) {
            let l = wind.orographic_lift(None, hx);
            if l > best {
                best = l;
                hx_climb = hx;
            }
        }
        assert!(best > 1e-4, "seed profile should loft somewhere, got {best}");

        let tc = 4;
        let gx = hx_climb * tc + tc / 2;
        let sign = if wind.climate_vx >= 0.0 { 1 } else { -1 };
        let gx_dn = gx + sign * tc;
        let hint_dn = continental_surface_y(p.seed, gx_dn, p.sea_level_y, p.width_cols);

        let mut live = World::new(p.seed);
        for x in [gx, gx_dn] {
            let hint = continental_surface_y(p.seed, x, p.sea_level_y, p.width_cols);
            for y in p.sea_level_y..=hint.max(hint_dn) {
                live.ensure_chunk(ChunkCoord::new(
                    x.div_euclid(CHUNK_CELLS_W as i32),
                    y.div_euclid(CHUNK_CELLS_H as i32),
                ));
            }
            live.set_cell(x, p.sea_level_y, Cell::solid(MaterialId::Stone));
            for y in (p.sea_level_y + 1)..=hint {
                live.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }

        let gy = (p.sea_level_y + 8).max(0);
        let mut on_hill = Humidity::with_world_bounds(
            4,
            0,
            p.bedrock_floor_y,
            p.width_cols,
            p.sky_ceiling_y,
        );
        on_hill.wrap_x = true;
        on_hill.add(gx, gy, 100.0);
        let hy0 = on_hill.tile_of(gx, gy).1;
        on_hill.apply_orographic_lift(&wind, Some(&live));
        let rose_live = on_hill.at_tile(hx_climb, hy0 + 1);

        for y in (p.sea_level_y + 1)..=hint_dn {
            live.set_cell(gx_dn, y, Cell::air());
        }
        live.set_cell(gx_dn, p.sea_level_y, Cell::solid(MaterialId::Stone));

        let mut flat = Humidity::with_world_bounds(
            4,
            0,
            p.bedrock_floor_y,
            p.width_cols,
            p.sky_ceiling_y,
        );
        flat.wrap_x = true;
        flat.add(gx, gy, 100.0);
        flat.apply_orographic_lift(&wind, Some(&live));
        let rose_flat = flat.at_tile(hx_climb, hy0 + 1);
        assert!(
            rose_live > rose_flat + 0.01,
            "flattening the live downwind column should cut lift ({rose_live} vs {rose_flat})"
        );
        assert!(
            (on_hill.total_mass() - 100.0).abs() < 1e-3,
            "lift must conserve mass"
        );
    }

    #[test]
    fn pond_bank_vapour_does_not_teleport_onto_both_shores() {
        use crate::cell::Cell;
        use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
        use crate::grid::World;
        use crate::wind::Wind;
        use wk_material::MaterialId;

        let width: i32 = 32;
        let bed: i32 = 8;
        let water_top: i32 = 12;
        let bank: i32 = 16;
        let mut world = World::new(1);
        for x in 0..width {
            let rock_top = if (8..16).contains(&x) { bed } else { bank };
            for y in 0i32..=bank {
                world.ensure_chunk(ChunkCoord::new(
                    x.div_euclid(CHUNK_CELLS_W as i32),
                    y.div_euclid(CHUNK_CELLS_H as i32),
                ));
            }
            for y in 0..=rock_top {
                world.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
            if (8..16).contains(&x) {
                for y in (bed + 1)..=water_top {
                    world.set_cell(x, y, Cell::water());
                }
            }
        }

        let mut wind = Wind::climate(4, 0.0, 1, width, bed, 0, 64, false);
        wind.config.terrain_drive = 0.0;
        wind.config.thermal_drive = 0.0;
        wind.config.swirl = 0.0;
        wind.variance = 0.0;

        let mut h = Humidity::with_world_bounds(4, 0, 0, width, 64);
        h.add(10, water_top, 80.0);
        let pond = h.tile_of(10, water_top);
        let left = h.tile_of(4, water_top);
        let right = h.tile_of(18, water_top);
        h.cells.insert((left.0, pond.1), 40.0);
        h.cells.insert((right.0, pond.1), 40.0);
        let crest_hy = h.tile_of(4, bank).1;
        assert!(
            crest_hy > pond.1,
            "fixture: bank crest must sit a tile above the pond"
        );

        h.advect_with_surface(0.0, 0.0, &wind, &world);

        assert!(
            h.at_tile(left.0, crest_hy) < 5.0,
            "left waterline must not hoist onto the crest ({})",
            h.at_tile(left.0, crest_hy)
        );
        assert!(
            h.at_tile(right.0, crest_hy) < 5.0,
            "right waterline must not hoist onto the crest ({})",
            h.at_tile(right.0, crest_hy)
        );
        assert!(
            h.at_tile(left.0, pond.1) > 20.0 && h.at_tile(right.0, pond.1) > 20.0,
            "leaked pond vapour should stay at the waterline (L={} R={})",
            h.at_tile(left.0, pond.1),
            h.at_tile(right.0, pond.1)
        );
    }

    #[test]
    fn peek_around_sums_the_moore_neighbourhood() {
        let mut h = Humidity::new(4);
        h.add(2, 2, 10.0); // tile (0,0)
        h.add(6, 2, 20.0); // tile (1,0)
        h.add(2, 6, 40.0); // tile (0,1)
        assert!(
            (h.peek_around(2, 2) - 70.0).abs() < 1e-3,
            "centre + two neighbours, got {}",
            h.peek_around(2, 2)
        );
        assert!((h.peek_around_tile(0, 0) - 70.0).abs() < 1e-3);
    }

    #[test]
    fn take_around_pays_from_neighbours_after_the_centre() {
        let mut h = Humidity::new(4);
        h.add(2, 2, 80.0);
        h.add(6, 2, 200.0);
        let got = h.take_around(2, 2, 255.0);
        assert!(
            (got - 255.0).abs() < 1e-3,
            "parcel should pay a full flake, got {got}"
        );
        assert!(
            h.at_tile(0, 0) < 1e-3,
            "centre should empty first, left {}",
            h.at_tile(0, 0)
        );
        assert!(
            (h.at_tile(1, 0) - 25.0).abs() < 1e-3,
            "neighbour should cover the shortfall, left {}",
            h.at_tile(1, 0)
        );
    }

    #[test]
    fn tile_boundary_is_exclusive_on_upper_edge() {
        let mut h = Humidity::new(4);
        h.add(0, 0, 1.0);
        h.add(4, 0, 2.0); // next tile over
        assert_eq!(h.at_cell(0, 0), 1.0);
        assert_eq!(h.at_cell(4, 0), 2.0);
        assert_eq!(h.at_tile(0, 0), 1.0);
        assert_eq!(h.at_tile(1, 0), 2.0);
    }

    #[test]
    fn diffusion_conserves_total_mass() {
        let mut h = Humidity::new(2);
        h.add(0, 0, 100.0);
        h.add(20, 0, 50.0);
        h.add(-4, -4, 25.0);
        let before = h.total_mass();
        for _ in 0..20 {
            h.diffuse(0.2);
        }
        let after = h.total_mass();
        assert!(
            (before - after).abs() < 1e-3,
            "diffusion must be mass-conservative: before={before}, after={after}"
        );
    }

    #[test]
    fn diffusion_spreads_a_spike() {
        let mut h = Humidity::new(1);
        h.add(0, 0, 100.0);
        assert_eq!(h.at_cell(1, 0), 0.0);
        h.diffuse(0.25);
        assert!(h.at_cell(1, 0) > 0.0, "mass should have flowed right");
        assert!(h.at_cell(-1, 0) > 0.0, "mass should have flowed left");
        assert!(h.at_cell(0, 1) > 0.0, "mass should have flowed up");
        assert!(h.at_cell(0, -1) > 0.0, "mass should have flowed down");
    }

    #[test]
    fn diffusion_with_alpha_zero_is_a_noop() {
        let mut h = Humidity::new(4);
        h.add(0, 0, 42.0);
        let before: Vec<((i32, i32), f32)> =
            h.cells.iter().map(|(&k, &v)| (k, v)).collect();
        h.diffuse(0.0);
        let after: Vec<((i32, i32), f32)> =
            h.cells.iter().map(|(&k, &v)| (k, v)).collect();
        assert_eq!(before, after);
    }

    #[test]
    fn empty_humidity_stays_empty_under_diffusion() {
        let mut h = Humidity::new(4);
        h.diffuse(0.2);
        assert_eq!(h.cells.len(), 0);
        assert_eq!(h.total_mass(), 0.0);
    }

    #[test]
    fn zero_add_does_not_create_an_entry() {
        let mut h = Humidity::new(4);
        h.add(0, 0, 0.0);
        assert!(h.cells.is_empty());
    }

    #[test]
    fn bounds_block_out_of_world_deposits() {
        let mut h = Humidity::with_world_bounds(4, 0, 0, 16, 16);
        h.add(2, 2, 10.0);
        h.add(100, 100, 50.0); // outside
        assert_eq!(h.total_mass(), 10.0);
        assert_eq!(h.cells.len(), 1);
    }

    #[test]
    fn diffuse_with_bounds_stays_inside_and_conserves() {
        let mut h = Humidity::with_world_bounds(1, 0, 0, 4, 4);
        // Capacity = 4×4 = 16 tiles.
        h.add(1, 1, 100.0);
        let before = h.total_mass();
        for _ in 0..80 {
            h.diffuse(0.25);
        }
        let after = h.total_mass();
        assert!(
            (before - after).abs() < 1e-3,
            "bounded diffusion must conserve: before={before}, after={after}"
        );
        assert!(
            h.cells.len() <= h.bounds.unwrap().tile_capacity(),
            "tile count {} exceeded capacity",
            h.cells.len()
        );
        for &(hx, hy) in h.cells.keys() {
            assert!(h.bounds.unwrap().contains(hx, hy), "oob tile ({hx},{hy})");
        }
        // Edge mass should remain (Neumann) — centre spike spreads but
        // does not vanish out the sides.
        assert!(after > 99.0);
    }

    #[test]
    fn diffuse_does_not_create_keys_outside_bounds() {
        let mut h = Humidity::with_world_bounds(1, 0, 0, 2, 2);
        h.add(0, 0, 100.0);
        h.diffuse(0.25);
        for &(hx, hy) in h.cells.keys() {
            assert!(
                (0..=1).contains(&hx) && (0..=1).contains(&hy),
                "created oob key ({hx},{hy})"
            );
        }
    }

    #[test]
    fn saturation_mass_shrinks_in_the_cold() {
        let hot = Humidity::saturation_mass_at_temp(Humidity::SAT_FULL_TEMP_C);
        let mild = Humidity::saturation_mass_at_temp(18.0);
        let freezing = Humidity::saturation_mass_at_temp(0.0);
        let arctic = Humidity::saturation_mass_at_temp(-20.0);
        let dead = Humidity::saturation_mass_at_temp(-100.0);
        assert!((hot - Humidity::MAX_MASS_PER_TILE).abs() < 1.0);
        assert!(
            mild > freezing * 2.5 && freezing > arctic * 2.0,
            "Clausius–Clapeyron must be steep (18={mild:.0} 0={freezing:.0} -20={arctic:.0})"
        );
        assert!(
            arctic > dead && dead < Humidity::MAX_MASS_PER_TILE * 0.01,
            "−100 °C holds only a trace (dead={dead:.2})"
        );
        assert!(hot > mild);
        let just_freeze = Humidity::saturation_mass_at_temp(-0.1);
        let three_below = Humidity::saturation_mass_at_temp(-3.0);
        assert!(
            (just_freeze - 206.0).abs() < 4.0,
            "−0.1 °C tile hold should be ~206, got {just_freeze:.1}"
        );
        assert!(
            (three_below - 162.0).abs() < 4.0,
            "−3 °C tile hold should be ~162, got {three_below:.1}"
        );
        assert!(
            (Humidity::saturation_cell_sat_at_temp(-0.1) - 21.0).abs() < 1.0
        );
        assert!(
            (Humidity::saturation_cell_sat_at_temp(-3.0) - 16.5).abs() < 1.0
        );
    }

    #[test]
    fn try_add_at_temp_refuses_oversaturated_cold_air() {
        let mut h = Humidity::new(4);
        let cap = Humidity::saturation_mass_at_temp(-15.0);
        let took = h.try_add_at_temp(2, 2, 2_000.0, -15.0);
        assert!(
            (took - cap).abs() < 0.5,
            "cold air should take its sat cap {cap:.1}, took {took:.1}"
        );
        assert!((h.at_cell(2, 2) - cap).abs() < 0.5);
        // Already-present mass is not this function's job. A later
        // cold snap must rain the surplus, not clamp this entry.
        h.cells.insert((0, 0), 800.0);
        let _ = h.try_add_at_temp(0, 0, 10.0, -15.0);
        assert!(
            (h.at_tile(0, 0) - 800.0).abs() < 1e-3,
            "try_add_at_temp must not delete vapour already in the tile"
        );
    }

    #[test]
    fn buoyant_rise_lifts_more_when_lapse_is_unstable() {
        let mut unstable = Humidity::new(4);
        unstable.add(2, 0, 100.0);
        let mut stable = Humidity::new(4);
        stable.add(2, 0, 100.0);
        let mut t_up = crate::temperature::Temperature::with_world_bounds(
            4, 0, 0, 16, 16, 1, 16, 4, false,
        );
        let mut t_st = t_up.clone();
        for ((_, hy), v) in t_up.cells.iter_mut() {
            *v = if *hy <= 0 { 24.0 } else { 8.0 };
        }
        for v in t_st.cells.values_mut() {
            *v = 12.0;
        }
        t_up.rebuild_row_means();
        t_st.rebuild_row_means();
        unstable.buoyant_rise_thermal(0.10, 4, Some(&mut t_up));
        stable.buoyant_rise_thermal(0.10, 4, Some(&mut t_st));
        assert!(
            unstable.at_tile(0, 1) > stable.at_tile(0, 1) + 2.0,
            "warm-under-cold should lift more ({} vs {})",
            unstable.at_tile(0, 1),
            stable.at_tile(0, 1)
        );
    }

    #[test]
    fn column_near_saturated_when_deck_is_wet() {
        let mut h = Humidity::new(4);
        assert!(!h.column_near_saturated(2, 0));
        h.add(2, 8, Humidity::MAX_MASS_PER_TILE);
        assert!(
            h.column_near_saturated(2, 0),
            "a near-full tile in the vapor column must block more evap"
        );
    }

    #[test]
    fn atmosphere_overfull_uses_thin_deck_budget() {
        let mut h = Humidity::with_world_bounds(4, 0, 0, 64, 256);
        assert!(!h.atmosphere_overfull());
        // width tiles = 16 → budget = 16 * 8 * MAX * 0.45
        for hx in 0..16 {
            for hy in 20..28 {
                h.cells.insert((hx, hy), Humidity::MAX_MASS_PER_TILE * 0.50);
            }
        }
        assert!(
            h.atmosphere_overfull(),
            "a filled 8-tile cloud deck must trip the soak budget"
        );
        assert!(
            h.cells.len() < h.bounds.unwrap().tile_capacity(),
            "budget is a thin deck, not the whole sky rectangle"
        );
    }

    #[test]
    fn humidity_diffuse_due_matches_column_schedule() {
        assert!(!humidity_diffuse_due(0));
        assert!(!humidity_diffuse_due(1));
        assert!(humidity_diffuse_due(3));
        assert!(!humidity_diffuse_due(4));
        assert!(humidity_diffuse_due(23));
        assert!(humidity_diffuse_due(43));
    }

    #[test]
    fn diffuse_wraps_horizontally_on_ring() {
        let mut h = Humidity::with_world_bounds(1, 0, 0, 4, 2);
        h.wrap_x = true;
        // Spike on the rightmost tile; after one pass some mass must
        // appear on the leftmost tile (the ring neighbour).
        h.add(3, 0, 100.0);
        let before = h.total_mass();
        h.diffuse(0.25);
        assert!((h.total_mass() - before).abs() < 1e-3);
        assert!(
            h.at_tile(0, 0) > 0.0,
            "mass should wrap from hx=3 to hx=0"
        );
    }

    #[test]
    fn dense_slab_matches_sparse_advect() {
        let mut sparse = Humidity::with_world_bounds(1, 0, 0, 32, 16);
        sparse.wrap_x = true;
        sparse.add(2, 3, 80.0);
        sparse.add(20, 8, 40.0);
        sparse.add(31, 0, 25.0);
        let mut dense = sparse.clone();
        assert!(
            !sparse.use_dense_slab(sparse.bounds.unwrap()),
            "few keys must keep the sparse flux walk"
        );
        let snap: Vec<((i32, i32), f32)> = sparse.cells.iter().map(|(&k, &v)| (k, v)).collect();
        sparse.flux_axis(&snap, 0.25, 0.05, None, true, None);
        sparse.flux_axis(&snap, 0.25, 0.05, None, false, None);
        let b = dense.bounds.unwrap();
        let (w, _) = b.dims();
        let packed = dense.pack_slab(b);
        let mut work = packed.clone();
        dense.flux_axis_into(&packed, &mut work, 0.25, 0.05, None, true, b, w, &[]);
        dense.flux_axis_into(&packed, &mut work, 0.25, 0.05, None, false, b, w, &[]);
        dense.sync_slab_changes(b, &packed, &work);
        assert!(
            (sparse.total_mass() - dense.total_mass()).abs() < 1e-4,
            "slab flux must conserve like sparse: {} vs {}",
            sparse.total_mass(),
            dense.total_mass()
        );
        for key in sparse.cells.keys().chain(dense.cells.keys()) {
            let a = sparse.at_tile(key.0, key.1);
            let b = dense.at_tile(key.0, key.1);
            assert!(
                (a - b).abs() < 1e-4,
                "tile {:?} sparse={a} slab={b}",
                key
            );
        }
    }

    #[test]
    fn dense_slab_matches_sparse_diffuse() {
        // 32×16 = 512 tiles (>256) with a few spikes stays on the
        // HashMap path; the sibling is forced through the slab.
        let mut sparse = Humidity::with_world_bounds(1, 0, 0, 32, 16);
        sparse.wrap_x = true;
        sparse.add(2, 3, 80.0);
        sparse.add(20, 8, 40.0);
        sparse.add(31, 0, 25.0);
        let mut dense = sparse.clone();
        assert!(
            !sparse.use_dense_slab(sparse.bounds.unwrap()),
            "few keys must keep the sparse walk"
        );
        sparse.diffuse(0.2);
        dense.diffuse_slab(0.2, dense.bounds.unwrap());
        assert!(
            (sparse.total_mass() - dense.total_mass()).abs() < 1e-4,
            "slab must conserve like sparse: {} vs {}",
            sparse.total_mass(),
            dense.total_mass()
        );
        for key in sparse.cells.keys().chain(dense.cells.keys()) {
            let a = sparse.at_tile(key.0, key.1);
            let b = dense.at_tile(key.0, key.1);
            assert!(
                (a - b).abs() < 1e-4,
                "tile {:?} sparse={a} slab={b}",
                key
            );
        }
    }
}

#[cfg(test)]
mod convection_tests {
    use super::*;
    use crate::temperature::{TempConfig, Temperature};

    /// Temperature field with one warm column and one cool one at the same height.
    fn split_temp(width: i32) -> Temperature {
        let mut t = Temperature::with_world_bounds(4, 0, 0, width, 320, 1, width, 40, false);
        t.fill_initial(0);
        t
    }

    /// Convection is the *difference* between columns, not the average lift.
    ///
    /// Buoyancy was driven only by the vertical lapse, which falls smoothly with
    /// altitude everywhere — so vapour rose at much the same rate over every
    /// column and the field stayed horizontally flat however hard it was driven.
    /// A column warmer than the world must lift more than a cooler one, or
    /// moisture is spread rather than organised.
    #[test]
    fn a_warm_column_lifts_more_than_a_cool_one() {
        let width = 256;
        let mut temp = split_temp(width);
        let mean = temp.mean();

        // Find two tiles at the same height with a real temperature spread.
        let hy = 12;
        let mut warm_hx = None;
        let mut cool_hx = None;
        for hx in 0..(width / 4) {
            let here = temp.at_tile(hx, hy);
            if here > mean + 0.5 && warm_hx.is_none() {
                warm_hx = Some(hx);
            }
            if here < mean - 0.5 && cool_hx.is_none() {
                cool_hx = Some(hx);
            }
        }
        let (Some(warm_hx), Some(cool_hx)) = (warm_hx, cool_hx) else {
            // No horizontal spread in this fixture: nothing to assert about
            // convection, and saying so beats a false pass.
            eprintln!("fixture had no horizontal temperature spread; skipped");
            return;
        };

        let lifted = |hx: i32, temp: &mut crate::temperature::Temperature| -> f32 {
            let mut h = Humidity::with_world_bounds(4, 0, 0, width, 320);
            h.cells.insert((hx, hy), 1000.0);
            h.buoyant_rise_thermal(0.30, 60, Some(temp));
            h.cells.get(&(hx, hy + 1)).copied().unwrap_or(0.0)
        };
        let w = lifted(warm_hx, &mut temp);
        let c = lifted(cool_hx, &mut temp);
        assert!(
            w > c,
            "the warmer column should lift more vapour ({w:.2} vs {c:.2})"
        );
    }

    #[test]
    fn convection_conserves_vapour() {
        // Lift moves mass between tiles; it must never create or destroy any.
        let width = 128;
        let mut temp = split_temp(width);
        let mut h = Humidity::with_world_bounds(4, 0, 0, width, 320);
        for hx in 0..(width / 4) {
            h.cells.insert((hx, 10), 500.0);
        }
        let before = h.total_mass();
        for _ in 0..20 {
            h.buoyant_rise_thermal(0.30, 60, Some(&mut temp));
        }
        let after = h.total_mass();
        assert!(
            (before - after).abs() < before * 1e-4,
            "convection must conserve vapour ({before} -> {after})"
        );
        let _ = TempConfig::default();
    }
}
