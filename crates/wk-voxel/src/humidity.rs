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
        let w = (self.hx_max - self.hx_min + 1).max(0) as usize;
        let h = (self.hy_max - self.hy_min + 1).max(0) as usize;
        w.saturating_mul(h)
    }
}

/// Cadence for atmospheric diffusion — same numbers as column-GVSE
/// `SubsystemId::HumidityField` (`period: 20`, `phase: 3`).
pub const HUMIDITY_DIFFUSE_PERIOD: u64 = 20;
pub const HUMIDITY_DIFFUSE_PHASE: u64 = 3;

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
    /// Sub-tile advection residual (shared climate wind). Used so
    /// clouds crawl smoothly instead of jumping whole tiles.
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
    pub const MAX_MASS_PER_TILE: f32 = 2_500.0;

    /// Saturation mass at air temperature (Clausius-lite, cheap).
    ///
    /// ~[`Self::MAX_MASS_PER_TILE`] near 18 °C. Cold air holds less, so
    /// the same vapor is closer to rain / visible cloud.
    pub fn saturation_mass_at_temp(temp_c: f32) -> f32 {
        let scale = ((temp_c + 8.0) / 26.0).clamp(0.16, 1.55);
        Self::MAX_MASS_PER_TILE * scale
    }

    /// Lifting condensation level (world-y): dewpoint depression → cloud base.
    ///
    /// `cloud_alt_above_sea` is a **scale**, not a hard shelf. Moist/cool air
    /// condenses low; dry/warm air climbs further. Used by buoyant rise and
    /// the visual deck so rain is not pinned to `sea + alt`.
    pub fn lifting_condensation_y(
        mass: f32,
        temp_c: f32,
        sea_level_y: i32,
        cloud_alt_above_sea: i32,
    ) -> f32 {
        let sat = Self::saturation_mass_at_temp(temp_c).max(1.0);
        let deficit = (1.0 - mass / sat).clamp(0.0, 1.0);
        // Same span the visual deck used; kept here so physics and paint agree.
        const MIN_FRAC: f32 = 0.55;
        const SPAN_FRAC: f32 = 0.90;
        sea_level_y as f32
            + cloud_alt_above_sea.max(1) as f32 * (MIN_FRAC + SPAN_FRAC * deficit)
    }

    /// Near-surface vapor for evaporation RH (first few tiles above the seat).
    ///
    /// Same-tile mass alone is useless once buoyant rise runs — the ground
    /// seat empties every tick. A short column is the air the film actually
    /// evaporates into.
    pub const EVAP_COLUMN_TILES: i32 = 3;

    pub fn near_surface_mass(&self, hx: i32, hy0: i32) -> f32 {
        let mut peak = 0.0f32;
        for i in 0..Self::EVAP_COLUMN_TILES {
            let hy = hy0 + i;
            if !self.accepts(hx, hy) {
                break;
            }
            peak = peak.max(self.at_tile(hx, hy));
        }
        peak
    }

    pub fn add(&mut self, gx: i32, gy: i32, mass: f32) {
        let _ = self.try_add(gx, gy, mass);
    }

    /// Add mass; returns how much was actually accepted under the
    /// per-tile cap ([`Self::MAX_MASS_PER_TILE`]).
    pub fn try_add(&mut self, gx: i32, gy: i32, mass: f32) -> f32 {
        if mass <= 0.0 {
            return 0.0;
        }
        let key = self.tile_of(gx, gy);
        if !self.accepts(key.0, key.1) {
            return 0.0;
        }
        let entry = self.cells.entry(key).or_insert(0.0);
        let room = (Self::MAX_MASS_PER_TILE - *entry).max(0.0);
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

    /// Peak humidity mass in the vapor column starting at tile `(hx, hy0)`.
    ///
    /// Buoyant rise empties the seat on the ground, so same-tile humidity
    /// underestimates how wet the air overhead is. Thermal shade and the
    /// evaporative budget both want this column peak.
    pub fn column_peak_mass(&self, hx: i32, hy0: i32) -> f32 {
        let mut peak = 0.0f32;
        for i in 0..Self::VAPOR_COLUMN_TILES {
            let hy = hy0 + i;
            if !self.accepts(hx, hy) {
                break;
            }
            peak = peak.max(self.at_tile(hx, hy));
        }
        peak
    }

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
        // Snapshot the current state so we don't chase deltas across
        // the pass.
        let snap: HashMap<(i32, i32), f32> = self.cells.clone();

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

        let mut deltas: HashMap<(i32, i32), f32> = HashMap::new();
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

    /// Buoyant lift: a fraction of each tile's mass moves one tile up,
    /// so vapor from ocean evaporation rises toward the cloud deck.
    /// Mass-conserving; stops at `max_hy`.
    pub fn buoyant_rise(&mut self, fraction: f32, max_hy: i32) {
        self.buoyant_rise_thermal(fraction, max_hy, None);
    }

    /// Weather-aware rise: stop at the per-column lifting condensation
    /// level and leave a near-saturation residual so the surface is not
    /// stripped into a hard empty band under a fixed `sea + alt` shelf.
    pub fn buoyant_rise_weather(
        &mut self,
        fraction: f32,
        sea_level_y: i32,
        cloud_alt_above_sea: i32,
        temp: Option<&crate::temperature::Temperature>,
    ) {
        let tc = self.tile_cols.max(1);
        // Dry-air LCL is the hard ceiling (same span as lifting_condensation_y).
        let max_hy = ((sea_level_y as f32
            + cloud_alt_above_sea.max(1) as f32 * 1.45)
            / tc as f32)
            .ceil() as i32;
        self.buoyant_rise_thermal_inner(fraction, max_hy, temp, Some((sea_level_y, cloud_alt_above_sea)));
    }

    /// [`Self::buoyant_rise`] scaled by the local lapse: warm air under
    /// colder air lifts harder; a stable inversion almost sits still.
    /// Same tile walk as the uniform rise — no extra world scans.
    /// How much a column's temperature anomaly changes its lift, per degree.
    const CONVECTION_GAIN_PER_C: f32 = 0.15;
    /// Anomaly is clamped before it is applied, so a freak tile cannot dominate.
    const CONVECTION_CLAMP_C: f32 = 6.0;
    /// Cool ground suppresses but never fully blocks lift; warm ground roughly
    /// doubles it. Bounded so convection reshapes the field rather than gating it.
    const CONVECTION_MIN_GAIN: f32 = 0.25;
    const CONVECTION_MAX_GAIN: f32 = 2.0;
    /// Retain this fraction of local saturation at each tile — only the
    /// moist excess rises. Without it, every tick gutting the surface into
    /// a zero-humidity shelf under the deck.
    const RISE_KEEP_RH: f32 = 0.62;

    pub fn buoyant_rise_thermal(
        &mut self,
        fraction: f32,
        max_hy: i32,
        temp: Option<&crate::temperature::Temperature>,
    ) {
        self.buoyant_rise_thermal_inner(fraction, max_hy, temp, None);
    }

    fn buoyant_rise_thermal_inner(
        &mut self,
        fraction: f32,
        max_hy: i32,
        temp: Option<&crate::temperature::Temperature>,
        lcl: Option<(i32, i32)>,
    ) {
        let fraction = fraction.clamp(0.0, 0.45);
        if fraction == 0.0 || self.cells.is_empty() {
            return;
        }
        let snap = self.cells.clone();
        let tc = self.tile_cols.max(1);
        // Mean temperature **per row**, computed once.
        //
        // Two mistakes to avoid here, both of which were made and measured. It has
        // to be hoisted: calling `Temperature::mean()` inside the loop scans the
        // whole field per tile and collapsed the frame rate. And it has to be
        // per-row: against a *global* mean every high-altitude tile reads as cool,
        // because temperature falls with altitude, so lift was suppressed aloft
        // everywhere and vapour piled into a dense unmoving layer near the ground.
        // The anomaly that means anything is horizontal — this column against other
        // columns at the same height.
        //
        // Averaged across the **world's** tile row, not across the tiles that
        // happen to hold vapour: keyed on occupancy, a lone cloud is its own mean
        // and never convects at all, and the reference drifts with wherever the
        // vapour currently is.
        let row_mean: HashMap<i32, f32> = match (temp, self.bounds) {
            (Some(t), Some(b)) => {
                let rows: std::collections::HashSet<i32> = snap.keys().map(|&(_, hy)| hy).collect();
                rows.into_iter()
                    .map(|hy| {
                        let mut sum = 0.0f32;
                        let mut n = 0u32;
                        for hx in b.hx_min..=b.hx_max {
                            sum += t.at_tile(hx, hy);
                            n += 1;
                        }
                        (hy, sum / n.max(1) as f32)
                    })
                    .collect()
            }
            _ => HashMap::new(),
        };
        let mut deltas: HashMap<(i32, i32), f32> = HashMap::new();
        for (&(hx, hy), &mass) in &snap {
            if mass <= 0.0 || hy >= max_hy {
                continue;
            }
            let dest = hy + 1;
            if !self.accepts(hx, dest) {
                continue;
            }
            // Per-column LCL: moist air stops low, dry air may climb to max_hy.
            if let Some((sea, alt)) = lcl {
                let t_c = temp
                    .map(|t| t.at_tile(hx, hy))
                    .unwrap_or(18.0);
                let lcl_y = Self::lifting_condensation_y(mass, t_c, sea, alt);
                let lcl_hy = (lcl_y / tc as f32).floor() as i32;
                if hy >= lcl_hy {
                    continue;
                }
            }
            let lift_f = if let Some(t) = temp {
                let here = t.at_tile(hx, hy);
                let above = t.at_tile(hx, dest);
                let lapse = (here - above).clamp(-5.0, 10.0);
                let base = (fraction * (0.40 + lapse * 0.11)).clamp(0.0, 0.45);
                let reference = row_mean.get(&hy).copied().unwrap_or(here);
                let anomaly =
                    (here - reference).clamp(-Self::CONVECTION_CLAMP_C, Self::CONVECTION_CLAMP_C);
                let gain = (1.0 + anomaly * Self::CONVECTION_GAIN_PER_C)
                    .clamp(Self::CONVECTION_MIN_GAIN, Self::CONVECTION_MAX_GAIN);
                (base * gain).clamp(0.0, 0.45)
            } else {
                fraction
            };
            // Weather rise only: leave a near-sat residual so the water–air
            // interface is not gutted every tick. Plain thermal rise (tests /
            // callers without LCL) still moves the full parcel.
            let liftable = if lcl.is_some() {
                let t_c = temp
                    .map(|t| t.at_tile(hx, hy))
                    .unwrap_or(18.0);
                let sat = Self::saturation_mass_at_temp(t_c);
                (mass - sat * Self::RISE_KEEP_RH).max(0.0)
            } else {
                mass
            };
            let lift = liftable * lift_f;
            if lift < 1e-6 {
                continue;
            }
            *deltas.entry((hx, hy)).or_insert(0.0) -= lift;
            *deltas.entry((hx, dest)).or_insert(0.0) += lift;
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
    }

    /// Advect atmospheric mass by a uniform climate wind `(vx, vy)`
    /// in tiles/tick. Fractional remainders accumulate in
    /// [`Self::advect_rx`] / [`Self::advect_ry`] so motion stays smooth.
    ///
    /// Mass-conserving: every gram lands on an accepted tile (vertical
    /// edges are Neumann — mass that would leave sticks at the rim).
    pub fn advect(&mut self, vx: f32, vy: f32) {
        self.advect_inner(vx, vy, None);
    }

    /// [`Self::advect`] that **climbs the live hill** instead of tunneling
    /// through it, then spends orographic lift on the windward face.
    ///
    /// Uniform `(vx, vy)` alone moves every seat by the same δ — fine over
    /// flat sea, but after near-surface vapor residuals returned, that slab
    /// slid *through* mountains and reappeared in the lee ("drifts behind").
    /// Destination seats buried under the live crest are lifted to free air
    /// so the field bumps the landscape.
    pub fn advect_with_surface(
        &mut self,
        vx: f32,
        vy: f32,
        wind: &crate::wind::Wind,
        world: &crate::grid::World,
    ) {
        self.advect_inner(vx, vy, Some((wind, world)));
        self.apply_orographic_lift(wind, Some(world));
    }

    fn advect_inner(
        &mut self,
        vx: f32,
        vy: f32,
        surface: Option<(&crate::wind::Wind, &crate::grid::World)>,
    ) {
        if self.cells.is_empty() || (vx == 0.0 && vy == 0.0) {
            return;
        }
        self.advect_rx += vx;
        self.advect_ry += vy;
        let dx = self.advect_rx.trunc() as i32;
        let dy = self.advect_ry.trunc() as i32;
        self.advect_rx -= dx as f32;
        self.advect_ry -= dy as f32;
        if dx == 0 && dy == 0 {
            // Still purge buried seats when the residual has not stepped yet.
            if let Some((wind, world)) = surface {
                self.lift_buried_to_free_air(wind, world);
            }
            return;
        }
        let snap = self.cells.clone();
        self.cells.clear();
        for ((hx, hy), mass) in snap {
            if mass.abs() < 1e-9 {
                continue;
            }
            let nhx = match self.wrap_hx(hx + dx) {
                Some(x) => x,
                None => hx,
            };
            let mut nhy = hy + dy;
            if !self.accepts(nhx, nhy) {
                nhy = hy;
                if !self.accepts(nhx, nhy) {
                    *self.cells.entry((hx, hy)).or_insert(0.0) += mass;
                    continue;
                }
            }
            if let Some((wind, world)) = surface {
                nhy = self.free_air_hy(wind, world, nhx).max(nhy);
                if !self.accepts(nhx, nhy) {
                    *self.cells.entry((hx, hy)).or_insert(0.0) += mass;
                    continue;
                }
            }
            *self.cells.entry((nhx, nhy)).or_insert(0.0) += mass;
        }
        let bounds = self.bounds;
        self.cells.retain(|&(hx, hy), v| {
            v.abs() > 1e-6 && bounds.map(|b| b.contains(hx, hy)).unwrap_or(true)
        });
    }

    /// First tile row whose centre sits in free air above the live crest.
    fn free_air_hy(
        &self,
        wind: &crate::wind::Wind,
        world: &crate::grid::World,
        hx: i32,
    ) -> i32 {
        let tc = self.tile_cols.max(1);
        let gx = hx * tc + tc / 2;
        let surf = crate::worldgen::live_surface_at(
            world,
            wind.seed,
            gx,
            wind.sea_level_y,
            wind.width_cols,
        );
        // mid = hy*tc + tc/2 > surf  ⇒  hy >= ceil((surf+1 - tc/2) / tc)
        ((surf + 1 - tc / 2).max(0) + tc - 1) / tc
    }

    /// Move any mass still parked inside the hill up to free air.
    fn lift_buried_to_free_air(
        &mut self,
        wind: &crate::wind::Wind,
        world: &crate::grid::World,
    ) {
        let keys: Vec<(i32, i32)> = self.cells.keys().copied().collect();
        let mut moves: Vec<((i32, i32), (i32, i32), f32)> = Vec::new();
        for (hx, hy) in keys {
            let air = self.free_air_hy(wind, world, hx);
            if hy >= air {
                continue;
            }
            let Some(&mass) = self.cells.get(&(hx, hy)) else {
                continue;
            };
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
        let snap = self.cells.clone();
        let mut deltas: HashMap<(i32, i32), f32> = HashMap::new();
        for (&(hx, hy), &mass) in &snap {
            if mass <= 0.0 {
                continue;
            }
            let lift = wind.orographic_lift(world, hx);
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
    fn advect_climbs_over_a_live_hill_instead_of_tunneling() {
        use crate::cell::Cell;
        use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
        use crate::grid::World;
        use crate::wind::Wind;
        use wk_material::MaterialId;

        let sea = 20;
        let mut world = World::new(3);
        // Flat approach, then a steep crest at x=24..31.
        for x in 0i32..40 {
            let top = if (24..32).contains(&x) { sea + 28 } else { sea };
            for y in 0i32..=(top + 2) {
                world.ensure_chunk(ChunkCoord::new(
                    x.div_euclid(CHUNK_CELLS_W as i32),
                    y.div_euclid(CHUNK_CELLS_H as i32),
                ));
            }
            for y in 0i32..=top {
                world.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
            world.set_cell(x, top + 1, Cell::air());
        }
        let wind = Wind::climate(4, 1.0, 3, 64, sea, 0, 320, false);
        let mut h = Humidity::with_world_bounds(4, 0, 0, 64, 320);
        // Seat over the flat approach (hy ~ sea/4), about to step into the hill.
        let hx0 = 20 / 4;
        let hy0 = sea / 4;
        h.cells.insert((hx0, hy0), 100.0);
        h.advect_rx = 0.0;
        h.advect_with_surface(1.0, 0.0, &wind, &world);
        let hx1 = hx0 + 1;
        let buried = h.at_tile(hx1, hy0);
        let crest_air = h.free_air_hy(&wind, &world, hx1);
        assert!(
            buried < 1.0,
            "mass must not sit inside the hill (got {buried:.1} at hy={hy0})"
        );
        let above: f32 = h
            .cells
            .iter()
            .filter(|(&(x, y), _)| x == hx1 && y >= crest_air)
            .map(|(_, &m)| m)
            .sum();
        assert!(
            above > 90.0,
            "mass should land in free air above the crest (hy>={crest_air}, got {above:.1})"
        );
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
        let warm = Humidity::saturation_mass_at_temp(20.0);
        let cold = Humidity::saturation_mass_at_temp(-8.0);
        assert!(warm > cold * 1.8, "cold air must hold much less vapor");
        assert!(warm <= Humidity::MAX_MASS_PER_TILE * 1.55);
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
        unstable.buoyant_rise_thermal(0.10, 4, Some(&t_up));
        stable.buoyant_rise_thermal(0.10, 4, Some(&t_st));
        assert!(
            unstable.at_tile(0, 1) > stable.at_tile(0, 1) + 2.0,
            "warm-under-cold should lift more ({} vs {})",
            unstable.at_tile(0, 1),
            stable.at_tile(0, 1)
        );
    }

    #[test]
    fn weather_rise_leaves_a_near_surface_residual() {
        // Fixed-deck rise used to strip every tile below sea+alt, leaving a
        // zero-humidity shelf over the water. Weather rise must keep a
        // residual under local saturation.
        let mut h = Humidity::with_world_bounds(4, 0, 0, 64, 320);
        let sea = 80;
        let hx = 2;
        let surf_hy = sea / 4;
        let sat = Humidity::saturation_mass_at_temp(18.0);
        h.cells.insert((hx, surf_hy), sat * 0.90);
        let mut t = crate::temperature::Temperature::with_world_bounds(
            4, 0, 0, 64, 320, 1, 64, sea, false,
        );
        for v in t.cells.values_mut() {
            *v = 18.0;
        }
        for _ in 0..30 {
            h.buoyant_rise_weather(0.12, sea, 48, Some(&t));
        }
        let left = h.at_tile(hx, surf_hy);
        assert!(
            left > sat * Humidity::RISE_KEEP_RH * 0.85,
            "surface seat should retain near-sat residual (got {left:.0}, sat={sat:.0})"
        );
    }

    #[test]
    fn moist_air_stops_rising_lower_than_dry_air() {
        let sea = 80;
        let alt = 100;
        let sat = Humidity::saturation_mass_at_temp(18.0);
        let moist_lcl = Humidity::lifting_condensation_y(sat * 0.95, 18.0, sea, alt);
        let dry_lcl = Humidity::lifting_condensation_y(sat * 0.15, 18.0, sea, alt);
        assert!(
            moist_lcl + 8.0 < dry_lcl,
            "LCL must rise as air dries ({moist_lcl} vs {dry_lcl})"
        );

        // Both start above the keep residual so excess can climb; moist
        // should arrest lower.
        let mut moist = Humidity::with_world_bounds(4, 0, 0, 64, 320);
        let mut dry = moist.clone();
        let hx = 3;
        let hy0 = sea / 4;
        moist.cells.insert((hx, hy0), sat * 0.95);
        dry.cells.insert((hx, hy0), sat * 0.70);
        let mut t = crate::temperature::Temperature::with_world_bounds(
            4, 0, 0, 64, 320, 1, 64, sea, false,
        );
        for v in t.cells.values_mut() {
            *v = 18.0;
        }
        for _ in 0..80 {
            moist.buoyant_rise_weather(0.20, sea, alt, Some(&t));
            dry.buoyant_rise_weather(0.20, sea, alt, Some(&t));
        }
        let top = |h: &Humidity| {
            h.cells
                .iter()
                .filter(|(&(x, _), _)| x == hx)
                .map(|(&(_, hy), _)| hy)
                .max()
                .unwrap_or(hy0)
        };
        let moist_top = top(&moist);
        let dry_top = top(&dry);
        assert!(
            moist_top <= dry_top,
            "moist column should not climb above dry ({moist_top} vs {dry_top})"
        );
        let moist_lcl_hy = (moist_lcl / 4.0).floor() as i32;
        assert!(
            moist_top <= moist_lcl_hy + 1,
            "moist rise must arrest near its LCL ({moist_top} vs lcl_hy {moist_lcl_hy})"
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
        let temp = split_temp(width);
        let n_hx = width / 4;

        // Sample free air above fixture peaks (continental surf tops out
        // ~159 with this seed/sea). Lower rows mix buried geothermal into
        // the row mean, so a "warm" pick against the *global* mean can still
        // be cold vs the row — both seats clamp to min gain and vertical
        // air-lapse decides the race. Convection's reference is the row.
        let hy = 40;
        let row_mean: f32 = (0..n_hx).map(|hx| temp.at_tile(hx, hy)).sum::<f32>() / n_hx as f32;

        let mut warm_hx = None;
        let mut cool_hx = None;
        for hx in 0..n_hx {
            let here = temp.at_tile(hx, hy);
            if here > row_mean + 0.5 && warm_hx.is_none() {
                warm_hx = Some(hx);
            }
            if here < row_mean - 0.5 && cool_hx.is_none() {
                cool_hx = Some(hx);
            }
        }
        let (Some(warm_hx), Some(cool_hx)) = (warm_hx, cool_hx) else {
            // No horizontal spread in this fixture: nothing to assert about
            // convection, and saying so beats a false pass.
            eprintln!("fixture had no horizontal temperature spread; skipped");
            return;
        };

        let lifted = |hx: i32| -> f32 {
            let mut h = Humidity::with_world_bounds(4, 0, 0, width, 320);
            h.cells.insert((hx, hy), 1000.0);
            h.buoyant_rise_thermal(0.30, 60, Some(&temp));
            h.cells.get(&(hx, hy + 1)).copied().unwrap_or(0.0)
        };
        let w = lifted(warm_hx);
        let c = lifted(cool_hx);
        assert!(
            w > c,
            "the warmer column should lift more vapour ({w:.2} vs {c:.2})"
        );
    }

    #[test]
    fn convection_conserves_vapour() {
        // Lift moves mass between tiles; it must never create or destroy any.
        let width = 128;
        let temp = split_temp(width);
        let mut h = Humidity::with_world_bounds(4, 0, 0, width, 320);
        for hx in 0..(width / 4) {
            h.cells.insert((hx, 10), 500.0);
        }
        let before = h.total_mass();
        for _ in 0..20 {
            h.buoyant_rise_thermal(0.30, 60, Some(&temp));
        }
        let after = h.total_mass();
        assert!(
            (before - after).abs() < before * 1e-4,
            "convection must conserve vapour ({before} -> {after})"
        );
        let _ = TempConfig::default();
    }
}
