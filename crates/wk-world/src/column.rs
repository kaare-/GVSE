use serde::{Deserialize, Serialize};
use wk_material::{MaterialId, MaterialRegistry, MAX_LAYERS, SAMPLE_WIDTH_M};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Activity {
    Dormant,
    HydrologyActive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MarkerId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SedimentLoad {
    pub total: i64,
    pub dominant: MaterialId,
}

impl Default for SedimentLoad {
    fn default() -> Self {
        Self {
            total: 0,
            dominant: MaterialId::Sand,
        }
    }
}

impl SedimentLoad {
    pub fn add(&mut self, material: MaterialId, mass: i64) {
        if mass <= 0 {
            return;
        }
        if self.total == 0 {
            self.dominant = material;
        }
        self.total += mass;
    }

    pub fn take(&mut self, mass: i64) -> SedimentLoad {
        let taken = mass.min(self.total);
        self.total -= taken;
        SedimentLoad {
            total: taken,
            dominant: self.dominant,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ResidualBucket {
    pub erosion: i64,
    pub infiltration: i64,
    pub evaporation: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Layer {
    pub material: MaterialId,
    pub thickness: i64,
    pub age_start: u64,
    pub age_end: u64,
}

impl Default for Layer {
    fn default() -> Self {
        Self {
            material: MaterialId::Sand,
            thickness: 0,
            age_start: 0,
            age_end: 0,
        }
    }
}

/// One vertical column sitting on top of the chunk's bedrock line.
///
/// Every physical substance sits in `layers`: sand, clay, stone, organic
/// but also water, ice, and snow — they're all just materials with
/// different property rows. There are no more "special" per-column state
/// buckets like `surface_water` or `ice`: standing water is simply a
/// `Water` layer at the top of the stack, an ice cap is an `Ice` layer,
/// snowfall is a `Snow` layer.
///
/// `moisture` (water occupying the pore space of the topmost porous solid
/// layer) is the one remaining scalar side-channel. Conceptually it
/// belongs to that layer, but tracking it per-layer would balloon the
/// data model and complicate flow between layers — for phase 1 of the
/// unification it stays as a column-level scalar, always associated
/// with `top_porous_layer_index()`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Column {
    pub surface_y: f32,
    pub layers: [Layer; MAX_LAYERS],
    pub layer_count: u8,
    /// Water occupying the pore space of the topmost porous solid layer
    /// (see `top_porous_layer_index`). Kept as a column-level scalar for
    /// now; per-layer moisture would require restructuring the flow and
    /// discharge logic more than the phase-1 refactor allows.
    pub moisture: i64,
    pub sediment: SedimentLoad,
    pub residual: ResidualBucket,
    pub activity: Activity,
    pub marker: Option<MarkerId>,
}

impl Default for Column {
    fn default() -> Self {
        Self {
            surface_y: 0.0,
            layers: [Layer::default(); MAX_LAYERS],
            layer_count: 0,
            moisture: 0,
            sediment: SedimentLoad::default(),
            residual: ResidualBucket::default(),
            activity: Activity::HydrologyActive,
            marker: None,
        }
    }
}

impl Column {
    pub fn top_layer(&self) -> Option<&Layer> {
        if self.layer_count == 0 {
            None
        } else {
            Some(&self.layers[0])
        }
    }

    pub fn top_material(&self) -> MaterialId {
        self.top_layer()
            .map(|l| l.material)
            .unwrap_or(MaterialId::Stone)
    }

    pub fn solid_mass(&self) -> i64 {
        (0..self.layer_count as usize)
            .map(|i| self.layers[i].thickness)
            .sum()
    }

    /// Index of the topmost layer with nonzero porosity (i.e. can hold
    /// pore-water). Skips past Water/Ice/Snow caps to find the actual
    /// substrate layer. `None` if no such layer exists (bare bedrock).
    pub fn top_porous_layer_index(&self) -> Option<usize> {
        for i in 0..self.layer_count as usize {
            if MaterialRegistry::props(self.layers[i].material).porosity > 0 {
                return Some(i);
            }
        }
        None
    }

    pub fn top_porous_layer(&self) -> Option<&Layer> {
        self.top_porous_layer_index().map(|i| &self.layers[i])
    }

    /// Kg of Water sitting on the very top of this column (standing water,
    /// puddle depth or lake / ocean surface). Zero if the top layer is
    /// anything else — a puddle under a snow cap doesn't count, and
    /// ice-covered water is `top_ice_mass`, not `top_water_mass`.
    pub fn top_water_mass(&self) -> i64 {
        match self.top_layer() {
            Some(l) if l.material == MaterialId::Water => l.thickness,
            _ => 0,
        }
    }

    /// Kg of Snow currently piled on top of the column (0 if the top
    /// layer is not Snow).
    pub fn top_snow_mass(&self) -> i64 {
        match self.top_layer() {
            Some(l) if l.material == MaterialId::Snow => l.thickness,
            _ => 0,
        }
    }

    /// Kg of Ice currently capping the column (0 if the top layer is not
    /// Ice). "Ice under a light snow dusting" is not counted here — only
    /// the true topmost layer.
    pub fn top_ice_mass(&self) -> i64 {
        match self.top_layer() {
            Some(l) if l.material == MaterialId::Ice => l.thickness,
            _ => 0,
        }
    }

    pub fn moisture_cap(&self) -> i64 {
        let Some(layer) = self.top_porous_layer() else {
            return 0;
        };
        let props = MaterialRegistry::props(layer.material);
        (layer.thickness * props.porosity as i64) / 255
    }

    pub fn mass_to_height_delta(&self, material: MaterialId, mass: i64) -> f32 {
        let density = MaterialRegistry::props(material).density.max(1) as f32;
        let volume = mass as f32 / density;
        let area = SAMPLE_WIDTH_M;
        volume / area
    }

    /// Elevation to use for temperature/climate purposes. Under the
    /// unified material model, snow and water are stratigraphic layers
    /// with the correct density, so `surface_y` already reflects the
    /// natural terrain height — no more "subtract snow layer height"
    /// workaround was needed.
    pub fn climate_elevation(&self) -> f32 {
        // Skip past any weather deposits (water/ice/snow) to expose the
        // permanent geographic elevation. A puddle on top of a peak
        // shouldn't make the peak read as warmer, and a thick snowpack
        // shouldn't feed itself by reading colder as it grows.
        let mut y = self.surface_y;
        for i in 0..self.layer_count as usize {
            let mat = self.layers[i].material;
            if matches!(
                mat,
                MaterialId::Water | MaterialId::Ice | MaterialId::Snow
            ) {
                y -= self.mass_to_height_delta(mat, self.layers[i].thickness);
            } else {
                break;
            }
        }
        y
    }

    /// Elevation of the groundwater table: the topmost porous solid
    /// layer's pore space fills from the bottom of that layer upward as
    /// `moisture` approaches its cap, reaching the ground surface exactly
    /// when fully saturated (any further inflow discharges as surface
    /// water — see the discharge handling in barrier commit).
    pub fn water_table_y(&self) -> f32 {
        let Some(idx) = self.top_porous_layer_index() else {
            return self.surface_y;
        };
        // Elevation of the top of the porous layer = surface minus
        // everything sitting above it in the stack (water/ice/snow caps).
        let mut cap_height = 0.0f32;
        for i in 0..idx {
            cap_height +=
                self.mass_to_height_delta(self.layers[i].material, self.layers[i].thickness);
        }
        let layer_top_y = self.surface_y - cap_height;
        let layer = &self.layers[idx];
        let layer_height_m = self.mass_to_height_delta(layer.material, layer.thickness);
        if layer_height_m <= 0.0 {
            return layer_top_y;
        }
        let cap = self.moisture_cap().max(1);
        let saturation = (self.moisture as f32 / cap as f32).clamp(0.0, 1.0);
        let layer_bottom_y = layer_top_y - layer_height_m;
        layer_bottom_y + saturation * layer_height_m
    }

    /// Insert `mass` kg of `material` as a solid layer *underneath* any
    /// Water / Ice / Snow cap sitting on top of this column. If there
    /// is no such cap, this is equivalent to `deposit_to_top`.
    ///
    /// Meant for sediment settling out of moving water: physically, sand
    /// carried by a river drops onto the *riverbed* when the current
    /// slows, not onto the water surface. Depositing on top was the
    /// mechanism that caused water to end up buried inside the solid
    /// stack as sediment layers sandwiched fresh puddles.
    pub fn deposit_below_fluid_cap(
        &mut self,
        material: MaterialId,
        mass: i64,
        tick: u64,
    ) {
        if mass <= 0 {
            return;
        }
        let insert_at = self.first_non_fluid_index();
        if insert_at == 0 {
            self.deposit_to_top(material, mass, tick);
            return;
        }
        self.activity = Activity::HydrologyActive;

        // Fast path: merge with the layer we'd be inserting above if
        // it's already the same material.
        if insert_at < self.layer_count as usize
            && self.layers[insert_at].material == material
        {
            self.layers[insert_at].thickness += mass;
            self.layers[insert_at].age_end = tick;
            self.surface_y += self.mass_to_height_delta(material, mass);
            return;
        }

        if (self.layer_count as usize) >= MAX_LAYERS {
            self.merge_layers(false, tick);
        }

        if (self.layer_count as usize) >= MAX_LAYERS {
            if insert_at < self.layer_count as usize {
                self.layers[insert_at].thickness += mass;
                self.layers[insert_at].age_end = tick;
            }
        } else {
            // Shift layers from insert_at..layer_count down by one slot
            // to make room, then place the new layer at insert_at (the
            // *bottom* of the fluid cap, i.e. right on the bed).
            for i in (insert_at + 1..=self.layer_count as usize).rev() {
                self.layers[i] = self.layers[i - 1];
            }
            self.layers[insert_at] = Layer {
                material,
                thickness: mass,
                age_start: tick,
                age_end: tick,
            };
            self.layer_count += 1;
        }
        self.surface_y += self.mass_to_height_delta(material, mass);
    }

    /// Index of the first layer that is *not* a fluid/weather cap
    /// (Water / Ice / Snow). Returns `layer_count` if every layer is
    /// fluid (would be an unusual state — a column with nothing but a
    /// puddle sitting on bare bedrock).
    fn first_non_fluid_index(&self) -> usize {
        for i in 0..self.layer_count as usize {
            if !matches!(
                self.layers[i].material,
                MaterialId::Water | MaterialId::Ice | MaterialId::Snow
            ) {
                return i;
            }
        }
        self.layer_count as usize
    }

    pub fn deposit_to_top(&mut self, material: MaterialId, mass: i64, tick: u64) {
        if mass <= 0 {
            return;
        }
        self.activity = Activity::HydrologyActive;

        if self.layer_count > 0 && self.layers[0].material == material {
            let layer = &mut self.layers[0];
            layer.thickness += mass;
            layer.age_end = tick;
            self.surface_y += self.mass_to_height_delta(material, mass);
            return;
        }

        if (self.layer_count as usize) >= MAX_LAYERS {
            self.merge_layers(false, tick);
        }

        if (self.layer_count as usize) >= MAX_LAYERS {
            if self.layer_count > 0 {
                self.layers[0].thickness += mass;
                self.layers[0].age_end = tick;
            }
        } else {
            for i in (1..=self.layer_count as usize).rev() {
                self.layers[i] = self.layers[i - 1];
            }
            self.layers[0] = Layer {
                material,
                thickness: mass,
                age_start: tick,
                age_end: tick,
            };
            self.layer_count += 1;
        }
        self.surface_y += self.mass_to_height_delta(material, mass);
    }

    /// Remove `mass` kg from whatever layer is currently on top,
    /// regardless of erosion rules. Meant for fluid/deposit management
    /// (draining water, evaporating a puddle, melting a snow cap) —
    /// contrast with `erode_from_top`, which respects erosion resistance
    /// and won't touch stone/bedrock. Returns actual mass removed and
    /// which material it came from.
    pub fn take_from_top_layer(&mut self, mass: i64) -> (i64, MaterialId) {
        if mass <= 0 || self.layer_count == 0 {
            return (0, MaterialId::Air);
        }
        self.activity = Activity::HydrologyActive;
        let mat = self.layers[0].material;
        let take = mass.min(self.layers[0].thickness);
        let dh = self.mass_to_height_delta(mat, take);
        self.layers[0].thickness -= take;
        self.surface_y -= dh;
        if self.layers[0].thickness <= 0 {
            self.pop_top_layer();
        }
        (take, mat)
    }

    /// Adjust top-of-column water by `delta` kg. Positive: adds water
    /// (creating or growing the top Water layer). Negative: removes from
    /// the top Water layer (no-op if the top isn't water — mass has
    /// nowhere to come from). Convenience for flow subsystems that
    /// think in delta-terms.
    pub fn adjust_top_water(&mut self, delta: i64, tick: u64) -> i64 {
        if delta > 0 {
            self.deposit_to_top(MaterialId::Water, delta, tick);
            delta
        } else if delta < 0 {
            if self.top_material() == MaterialId::Water {
                let (removed, _) = self.take_from_top_layer(-delta);
                -removed
            } else {
                0
            }
        } else {
            0
        }
    }

    /// Erode the topmost erodible-and-soft layer. Skips past cover
    /// materials that shouldn't be picked up as sediment (water, ice,
    /// snow) so a river can actually cut into the sand under a puddle.
    /// Stops without eroding if the first erodible layer is hard enough
    /// to be effectively bedrock-like (resistance ≥ 150).
    pub fn erode_from_top(&mut self, mass: i64) -> (i64, MaterialId) {
        if mass <= 0 {
            return (0, MaterialId::Sand);
        }
        let mut idx = 0usize;
        while idx < self.layer_count as usize {
            let mat = self.layers[idx].material;
            if mat.is_erodible() && MaterialRegistry::props(mat).erosion_resistance < 150 {
                break;
            }
            // Only allow eroding under a Water cap. Ice/Snow/Stone above
            // the target protect the layer below.
            if mat != MaterialId::Water {
                return (0, MaterialId::Sand);
            }
            idx += 1;
        }
        if idx >= self.layer_count as usize {
            return (0, MaterialId::Sand);
        }
        self.activity = Activity::HydrologyActive;

        let mat = self.layers[idx].material;
        let take = mass.min(self.layers[idx].thickness);
        let dh = self.mass_to_height_delta(mat, take);
        self.layers[idx].thickness -= take;
        self.surface_y -= dh;
        if self.layers[idx].thickness <= 0 {
            self.remove_layer_at(idx);
        }
        (take, mat)
    }

    fn pop_top_layer(&mut self) {
        self.remove_layer_at(0);
    }

    fn remove_layer_at(&mut self, idx: usize) {
        if idx >= self.layer_count as usize {
            return;
        }
        for i in idx..(self.layer_count as usize) - 1 {
            self.layers[i] = self.layers[i + 1];
        }
        self.layer_count -= 1;
    }

    pub fn merge_layers(&mut self, force_epoch: bool, tick: u64) {
        let _ = tick;
        let mut i = 0usize;
        while i + 1 < self.layer_count as usize {
            let a = &self.layers[i];
            let b = &self.layers[i + 1];
            let pinned = self.marker.map(|m| m.0 as usize) == Some(i);

            let can_merge = !pinned
                && a.material == b.material
                && a.age_end + wk_material::MERGE_GAP >= b.age_start
                && a.thickness + b.thickness <= wk_material::MERGE_MAX_THICKNESS;

            if can_merge || (force_epoch && a.material == b.material && !pinned) {
                let merged = Layer {
                    material: a.material,
                    thickness: a.thickness + b.thickness,
                    age_start: a.age_start.min(b.age_start),
                    age_end: a.age_end.max(b.age_end),
                };
                self.layers[i] = merged;
                for j in i + 1..(self.layer_count as usize) - 1 {
                    self.layers[j] = self.layers[j + 1];
                }
                self.layer_count -= 1;
            } else {
                i += 1;
            }
        }
    }

    pub fn clamp_state(&mut self) {
        let cap = self.moisture_cap();
        if self.moisture > cap {
            // Discharge: pore space is full; overflow surfaces as
            // standing water on top (a spring/seep). Kept here as well
            // as in barrier commit so no code path ends up with
            // moisture > cap silently held on the column.
            let overflow = self.moisture - cap;
            self.moisture = cap;
            self.deposit_to_top(MaterialId::Water, overflow, 0);
        }
        self.moisture = self.moisture.max(0);
        self.sediment.total = self.sediment.total.max(0);
        while self.layer_count > 0 && self.layers[0].thickness <= 0 {
            self.pop_top_layer();
        }
        if self.surface_y.is_nan() {
            self.surface_y = 0.0;
        }
    }

    /// Keep `surface_y` consistent with the summed layer column height.
    pub fn recompute_surface_y(&mut self, bedrock_y: f32) {
        let height: f32 = (0..self.layer_count as usize)
            .map(|i| {
                self.mass_to_height_delta(self.layers[i].material, self.layers[i].thickness)
            })
            .sum();
        self.surface_y = bedrock_y + height;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erode_half_mass_lowers_surface() {
        let mut col = Column::default();
        col.deposit_to_top(MaterialId::Sand, 1000, 0);
        let y0 = col.surface_y;
        let (removed, _) = col.erode_from_top(500);
        assert_eq!(removed, 500);
        assert!(col.surface_y < y0);
        assert!((col.surface_y - y0).abs() > 0.01);
    }

    #[test]
    fn merge_three_sand_layers() {
        let mut col = Column::default();
        col.layer_count = 3;
        col.layers[0] = Layer {
            material: MaterialId::Sand,
            thickness: 100,
            age_start: 20,
            age_end: 20,
        };
        col.layers[1] = Layer {
            material: MaterialId::Sand,
            thickness: 100,
            age_start: 10,
            age_end: 10,
        };
        col.layers[2] = Layer {
            material: MaterialId::Sand,
            thickness: 100,
            age_start: 0,
            age_end: 0,
        };
        col.merge_layers(true, 100);
        assert_eq!(col.layer_count, 1);
        assert_eq!(col.layers[0].thickness, 300);
    }

    #[test]
    fn sand_clay_do_not_merge() {
        let mut col = Column::default();
        col.deposit_to_top(MaterialId::Sand, 100, 0);
        col.deposit_to_top(MaterialId::Clay, 100, 10);
        col.merge_layers(true, 100);
        assert_eq!(col.layer_count, 2);
    }

    #[test]
    fn water_becomes_a_top_layer() {
        let mut col = Column::default();
        col.deposit_to_top(MaterialId::Sand, 1000, 0);
        col.deposit_to_top(MaterialId::Water, 250, 5);
        assert_eq!(col.top_material(), MaterialId::Water);
        assert_eq!(col.top_water_mass(), 250);
        assert_eq!(col.layer_count, 2);
    }

    #[test]
    fn erode_from_top_cuts_under_water_cap() {
        let mut col = Column::default();
        col.deposit_to_top(MaterialId::Sand, 1000, 0);
        col.deposit_to_top(MaterialId::Water, 500, 5);
        let (removed, mat) = col.erode_from_top(100);
        assert_eq!(removed, 100);
        assert_eq!(mat, MaterialId::Sand);
    }

    #[test]
    fn erode_from_top_stops_at_ice_cap() {
        let mut col = Column::default();
        col.deposit_to_top(MaterialId::Sand, 1000, 0);
        col.deposit_to_top(MaterialId::Ice, 500, 5);
        let (removed, _) = col.erode_from_top(100);
        assert_eq!(removed, 0);
    }

    #[test]
    fn deposit_below_fluid_cap_inserts_under_water() {
        let mut col = Column::default();
        col.deposit_to_top(MaterialId::Sand, 1000, 0);
        col.deposit_to_top(MaterialId::Water, 250, 5);
        // Now: [Water 250, Sand 1000]
        col.deposit_below_fluid_cap(MaterialId::Clay, 200, 10);
        // Expected: [Water 250, Clay 200, Sand 1000] — water stays on top
        assert_eq!(col.layer_count, 3);
        assert_eq!(col.layers[0].material, MaterialId::Water);
        assert_eq!(col.layers[0].thickness, 250);
        assert_eq!(col.layers[1].material, MaterialId::Clay);
        assert_eq!(col.layers[1].thickness, 200);
        assert_eq!(col.layers[2].material, MaterialId::Sand);
    }

    #[test]
    fn deposit_below_fluid_cap_merges_with_bed() {
        let mut col = Column::default();
        col.deposit_to_top(MaterialId::Sand, 1000, 0);
        col.deposit_to_top(MaterialId::Water, 250, 5);
        col.deposit_below_fluid_cap(MaterialId::Sand, 500, 10);
        // Sand under water grows in-place, no new layer inserted.
        assert_eq!(col.layer_count, 2);
        assert_eq!(col.layers[0].material, MaterialId::Water);
        assert_eq!(col.layers[1].material, MaterialId::Sand);
        assert_eq!(col.layers[1].thickness, 1500);
    }

    #[test]
    fn deposit_below_fluid_cap_falls_through_without_cap() {
        let mut col = Column::default();
        col.deposit_to_top(MaterialId::Sand, 1000, 0);
        // No water on top. Should behave like deposit_to_top.
        col.deposit_below_fluid_cap(MaterialId::Clay, 200, 10);
        assert_eq!(col.layer_count, 2);
        assert_eq!(col.layers[0].material, MaterialId::Clay);
        assert_eq!(col.layers[1].material, MaterialId::Sand);
    }

    #[test]
    fn top_porous_skips_water_cap() {
        let mut col = Column::default();
        col.deposit_to_top(MaterialId::Sand, 1000, 0);
        col.deposit_to_top(MaterialId::Water, 250, 5);
        assert_eq!(col.top_porous_layer().unwrap().material, MaterialId::Sand);
    }
}
