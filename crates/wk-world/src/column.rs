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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Column {
    pub surface_y: f32,
    pub layers: [Layer; MAX_LAYERS],
    pub layer_count: u8,
    pub surface_water: i64,
    pub moisture: i64,
    /// Frozen surface water (kg) — tracked separately from `surface_water`
    /// so it's automatically excluded from evaporation/infiltration/lateral
    /// flow (they only ever look at `surface_water`); thaws back into it
    /// when warm. See run_freeze_thaw.
    pub ice: i64,
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
            surface_water: 0,
            moisture: 0,
            ice: 0,
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

    pub fn moisture_cap(&self) -> i64 {
        let top = self.top_material();
        let props = MaterialRegistry::props(top);
        let layer_mass = self
            .top_layer()
            .map(|l| l.thickness)
            .unwrap_or(0)
            .max(1);
        (layer_mass * props.porosity as i64) / 255
    }

    pub fn mass_to_height_delta(&self, material: MaterialId, mass: i64) -> f32 {
        let density = MaterialRegistry::props(material).density.max(1) as f32;
        let volume = mass as f32 / density;
        let area = SAMPLE_WIDTH_M;
        volume / area
    }

    /// Elevation to use for temperature/climate purposes — deliberately
    /// *excludes* any snow currently piled on top. Real climate depends on
    /// a place's permanent geographic elevation, not today's snow depth;
    /// using the literal (snow-inflated) surface_y would create a runaway
    /// feedback loop: snow raises surface_y -> higher elevation reads
    /// colder -> more snow falls -> raises surface_y further, forever.
    pub fn climate_elevation(&self) -> f32 {
        match self.top_layer() {
            Some(top) if top.material == MaterialId::Snow => {
                self.surface_y - self.mass_to_height_delta(MaterialId::Snow, top.thickness)
            }
            _ => self.surface_y,
        }
    }

    /// Elevation of the groundwater table: the top layer's pore space fills
    /// from the bottom of that layer upward as `moisture` approaches its
    /// cap, reaching the ground surface exactly when fully saturated (at
    /// which point any further water has nowhere to go but the surface —
    /// see the discharge/spring handling in the barrier commit).
    pub fn water_table_y(&self) -> f32 {
        let Some(top) = self.top_layer() else {
            return self.surface_y;
        };
        let layer_height_m = self.mass_to_height_delta(top.material, top.thickness);
        if layer_height_m <= 0.0 {
            return self.surface_y;
        }
        let cap = self.moisture_cap().max(1);
        let saturation = (self.moisture as f32 / cap as f32).clamp(0.0, 1.0);
        let layer_bottom = self.surface_y - layer_height_m;
        layer_bottom + saturation * layer_height_m
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
            // merge failed to free space; add to existing top
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

    pub fn erode_from_top(&mut self, mut mass: i64) -> (i64, MaterialId) {
        if mass <= 0 || self.layer_count == 0 {
            return (0, MaterialId::Sand);
        }
        self.activity = Activity::HydrologyActive;

        let mut removed = 0i64;
        let mut material_out = MaterialId::Sand;

        while mass > 0 && self.layer_count > 0 {
            let material = self.layers[0].material;
            if !material.is_erodible() {
                break;
            }
            let resistance = MaterialRegistry::props(material).erosion_resistance;
            if resistance >= 150 {
                break;
            }
            let take = mass.min(self.layers[0].thickness);
            let height_delta = self.mass_to_height_delta(material, take);
            self.layers[0].thickness -= take;
            self.surface_y -= height_delta;
            removed += take;
            mass -= take;
            material_out = material;

            if self.layers[0].thickness == 0 {
                self.pop_top_layer();
            }
        }

        (removed, material_out)
    }

    fn pop_top_layer(&mut self) {
        if self.layer_count == 0 {
            return;
        }
        for i in 0..(self.layer_count as usize) - 1 {
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
        self.surface_water = self.surface_water.max(0);
        self.ice = self.ice.max(0);
        let cap = self.moisture_cap();
        if self.moisture > cap {
            self.surface_water += self.moisture - cap;
            self.moisture = cap;
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
}
