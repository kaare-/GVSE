use serde::{Deserialize, Serialize};
use wk_material::{MaterialId, MaterialRegistry, MAX_LAYERS, SAMPLE_WIDTH_M};

/// Soft cap on voids per column. Pathological caves can exceed this but
/// most columns stay well below; keeps growth from runaway dissolution.
pub const MAX_VOIDS: usize = 4;

/// kg of free water per metre of void / column depth — must match
/// `wk_sim::subsystems::shared::WATER_MASS_PER_METRE_DEPTH`.
pub const VOID_WATER_KG_PER_M: f32 = 250.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Activity {
    Dormant,
    HydrologyActive,
}

/// How a void was created. Karst/burrow/collapse share the same geometry
/// but ecology and dig rules may treat origins differently later.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum VoidOrigin {
    #[default]
    Karst,
    Burrow,
    Collapse,
}

/// Sparse cavity annotation on a column. Layers still hold all mass;
/// voids describe where that mass isn't. Never represent caves as Air
/// layers — density settling would float them to the top.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Void {
    /// Absolute elevation of the ceiling (metres).
    pub top_y: f32,
    /// Ceiling − floor (metres).
    pub height_m: f32,
    /// Free water pooled inside the void (kg).
    pub water_mass: i64,
    /// Material of the layer immediately above (roof).
    pub roof_material: MaterialId,
    pub origin: VoidOrigin,
    /// 0..255 connectivity to surface (light / ventilation proxy).
    pub light: u8,
}

impl Void {
    pub fn floor_y(self) -> f32 {
        self.top_y - self.height_m
    }

    pub fn mid_y(self) -> f32 {
        self.top_y - 0.5 * self.height_m
    }

    /// True when the void ceiling reaches (or breaches) the column surface.
    pub fn open_to_surface(self, surface_y: f32) -> bool {
        self.top_y >= surface_y - 0.05
    }

    /// Geometric free-water capacity (kg) for this cavity.
    pub fn capacity_kg(self) -> i64 {
        (self.height_m.max(0.0) * VOID_WATER_KG_PER_M).round() as i64
    }

    pub fn free_capacity_kg(self) -> i64 {
        (self.capacity_kg() - self.water_mass.max(0)).max(0)
    }

    /// Fill fraction of geometric capacity, 0..1.
    pub fn fill_frac(self) -> f32 {
        let cap = self.capacity_kg().max(1) as f32;
        (self.water_mass.max(0) as f32 / cap).clamp(0.0, 1.0)
    }

    /// True when elevation `y` lies inside this cavity.
    pub fn contains_y(self, y: f32) -> bool {
        y <= self.top_y + 1e-3 && y >= self.floor_y() - 1e-3 && self.height_m > 1e-4
    }
}

/// Ambient atmospheric gas levels (relative units, ~1.0 = well-mixed air).
pub const AMBIENT_AIR_CO2: f32 = 1.0;
pub const AMBIENT_AIR_O2: f32 = 1.0;
/// Henry-ish equilibrium dissolved levels under ambient air.
pub const EQUIL_WATER_CO2: f32 = 0.85;
pub const EQUIL_WATER_O2: f32 = 0.90;

/// Per-column plant / soil-biology state (stage 8). Not a stratigraphic
/// layer — biomass must not participate in density settling.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Ecology {
    /// Root binding of the topsoil, 0 = bare, 1 = dense mat.
    pub root_density: f32,
    /// Leaf-area index proxy, 0 = bare, 1 = full canopy.
    pub leaf_area: f32,
    /// Standing dead organic mass (kg).
    pub dead_biomass: i64,
    /// Living plant mass (kg).
    pub alive_biomass: i64,
    /// Plant-available nutrient fraction, 0..1.
    pub nutrient: f32,
    /// Atmospheric CO₂ above the column (relative units).
    #[serde(default = "default_air_co2")]
    pub air_co2: f32,
    /// Atmospheric O₂ above the column.
    #[serde(default = "default_air_o2")]
    pub air_o2: f32,
    /// Dissolved CO₂ in standing / pore water (relative units).
    #[serde(default = "default_water_co2")]
    pub water_co2: f32,
    /// Dissolved O₂ in standing / pore water.
    #[serde(default = "default_water_o2")]
    pub water_o2: f32,
}

fn default_air_co2() -> f32 {
    AMBIENT_AIR_CO2
}
fn default_air_o2() -> f32 {
    AMBIENT_AIR_O2
}
fn default_water_co2() -> f32 {
    EQUIL_WATER_CO2
}
fn default_water_o2() -> f32 {
    EQUIL_WATER_O2
}

impl Default for Ecology {
    fn default() -> Self {
        Self {
            root_density: 0.0,
            leaf_area: 0.0,
            dead_biomass: 0,
            alive_biomass: 0,
            nutrient: 0.0,
            air_co2: AMBIENT_AIR_CO2,
            air_o2: AMBIENT_AIR_O2,
            water_co2: EQUIL_WATER_CO2,
            water_o2: EQUIL_WATER_O2,
        }
    }
}

impl Ecology {
    pub fn biomass_total(self) -> i64 {
        self.alive_biomass.max(0) + self.dead_biomass.max(0)
    }

    /// Seed a sparse starter community from biome + relative elevation.
    pub fn seed_from_biome(biome: crate::climate::Biome, rel_sea_m: f32) -> Self {
        use crate::climate::Biome;
        let (alive, nutrient, roots, leaves) = match biome {
            Biome::Ocean | Biome::Shelf => (0, 0.0, 0.0, 0.0),
            Biome::Coast => (40, 0.35, 0.15, 0.12),
            Biome::Plains => (80, 0.45, 0.25, 0.22),
            Biome::Mountain => {
                if rel_sea_m > 55.0 {
                    (10, 0.15, 0.05, 0.04)
                } else {
                    (30, 0.25, 0.10, 0.08)
                }
            }
        };
        Self {
            root_density: roots,
            leaf_area: leaves,
            dead_biomass: 0,
            alive_biomass: alive,
            nutrient,
            air_co2: AMBIENT_AIR_CO2,
            air_o2: AMBIENT_AIR_O2,
            water_co2: EQUIL_WATER_CO2,
            water_o2: EQUIL_WATER_O2,
        }
    }
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
    /// Sparse cavities (karst / burrow / collapse). Empty for most columns.
    /// `serde(default)` keeps older saves loadable.
    #[serde(default)]
    pub voids: Vec<Void>,
    /// Plant / soil-biology bucket (stage 8). Default = barren.
    #[serde(default)]
    pub ecology: Ecology,
    /// Horizontal free-surface velocity (m/s) for wind/tide gravity waves.
    /// Zero when dry. Not a gene — pure hydro state. `serde(default)` for
    /// older saves.
    #[serde(default)]
    pub surface_u: f32,
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
            voids: Vec::new(),
            ecology: Ecology::default(),
            surface_u: 0.0,
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
            if self.layers[i].thickness <= 0 {
                continue;
            }
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

    /// Water available for lateral flow: total kg of Water in the fluid
    /// cap (any Water layer sitting above the first solid substrate),
    /// plus the elevation of the top of that water body (excludes any
    /// snow / ice floating above it). `None` if the column has no
    /// water in its cap.
    ///
    /// This is what `run_surface_water` actually cares about: snow
    /// floating on a pool doesn't stop the pool below from draining
    /// sideways when a neighbouring column has a lower water surface.
    /// The snow just settles onto whatever's left as the water leaves.
    pub fn flowable_water(&self) -> Option<(f32, i64)> {
        let mut total_water = 0i64;
        for j in 0..self.layer_count as usize {
            let m = self.layers[j].material;
            match m {
                MaterialId::Water => total_water += self.layers[j].thickness,
                MaterialId::Snow | MaterialId::Ice => {
                    // Cap material. Doesn't seal the water below —
                    // water can still flow sideways out from under it.
                }
                _ => break,
            }
        }
        if total_water <= 0 {
            return None;
        }
        // Free-surface elevation rests on the solid bed — not on
        // `surface_y`, which still adds cavity height. Counting voids as
        // "ground" made shoreline karst mouths sit metres above their
        // neighbours, so lake-level drained those columns and algae rode
        // a notched / wavy free surface into the air.
        let h = self.mass_to_height_delta(MaterialId::Water, total_water);
        Some((self.solid_bed_y() + h, total_water))
    }

    /// Remove up to `mass` kg from the *flowable* Water layer inside
    /// the fluid cap, even if there's Snow/Ice sitting above it.
    /// Returns actual mass removed.
    pub fn take_water_from_cap(&mut self, mass: i64) -> i64 {
        if mass <= 0 {
            return 0;
        }
        // Find the topmost Water layer in the cap.
        let mut idx = None;
        for j in 0..self.layer_count as usize {
            let m = self.layers[j].material;
            match m {
                MaterialId::Water => {
                    idx = Some(j);
                    break;
                }
                MaterialId::Snow | MaterialId::Ice => continue,
                _ => break,
            }
        }
        let Some(j) = idx else {
            return 0;
        };
        let take = mass.min(self.layers[j].thickness);
        if take <= 0 {
            return 0;
        }
        self.activity = Activity::HydrologyActive;
        let dh = self.mass_to_height_delta(MaterialId::Water, take);
        self.layers[j].thickness -= take;
        self.surface_y -= dh;
        if self.layers[j].thickness <= 0 {
            for i in j..(self.layer_count as usize).saturating_sub(1) {
                self.layers[i] = self.layers[i + 1];
            }
            if self.layer_count > 0 {
                self.layer_count -= 1;
                self.layers[self.layer_count as usize] = Layer::default();
            }
        }
        take
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

    /// Pore-water capacity of the near-surface rooting zone (metres of
    /// porous solid below any weather cap).
    ///
    /// Previously this was only the single top porous layer. A thin
    /// `Organic` beach skin then shrank the whole column's moisture
    /// budget to a few hundred kg, so plant ET flash-dried hills the
    /// moment rain stopped. Roots drink from a deeper soil profile.
    pub const MOISTURE_ROOTING_ZONE_M: f32 = 3.0;

    pub fn moisture_cap(&self) -> i64 {
        let mut depth_m = 0.0f32;
        let mut cap = 0i64;
        for i in 0..self.layer_count as usize {
            let layer = &self.layers[i];
            if layer.thickness <= 0 {
                continue;
            }
            let mat = layer.material;
            // Skip weather / fluid cap — moisture lives in solid pores.
            if matches!(
                mat,
                MaterialId::Water | MaterialId::Ice | MaterialId::Snow | MaterialId::Air
            ) {
                continue;
            }
            let porosity = MaterialRegistry::props(mat).porosity;
            if porosity == 0 {
                // Competent impermeable rock — rooting zone ends.
                break;
            }
            let h = self.mass_to_height_delta(mat, layer.thickness);
            if h <= 1e-9 {
                continue;
            }
            let remain = (Self::MOISTURE_ROOTING_ZONE_M - depth_m).max(0.0);
            if remain <= 0.0 {
                break;
            }
            let use_h = h.min(remain);
            let mass = ((layer.thickness as f32) * (use_h / h)).round() as i64;
            cap = cap.saturating_add((mass.saturating_mul(porosity as i64)) / 255);
            depth_m += use_h;
            if depth_m >= Self::MOISTURE_ROOTING_ZONE_M - 1e-4 {
                break;
            }
        }
        cap
    }

    pub fn mass_to_height_delta(&self, material: MaterialId, mass: i64) -> f32 {
        let density = MaterialRegistry::props(material).density.max(1) as f32;
        let volume = mass as f32 / density;
        let area = SAMPLE_WIDTH_M;
        volume / area
    }

    /// Solid-bed / geographic elevation (skips water, ice, snow caps).
    ///
    /// Used for biome classification, bathymetry, and landform logic — not
    /// for ambient air/water-skin temperature. Deep-ocean beds sit far below
    /// the capped thermal field; sampling temperature there clamps to the
    /// geothermal Dirichlet (~55 °C) and reads as "boiling ocean".
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

    /// Hydraulic head for a dry neighbour: the solid rock/soil bed under
    /// any snow/ice/water cap *and* cavity height. Using void-inflated
    /// `climate_elevation` here made dry karst mouths look like tall dams
    /// (and disagreed with [`Self::flowable_water`]'s solid-bed free
    /// surface). Using raw `surface_y` made snow banks into dams.
    pub fn hydraulic_bed_y(&self) -> f32 {
        self.solid_bed_y()
    }

    /// Elevation of the solid rock/soil surface, excluding weather fluids
    /// *and* cavity height. `climate_elevation` still includes voids
    /// (they inflate `surface_y`), which made submerged limestone with
    /// sea-cliff mouths look "emergent" for karst / void-capture gates.
    pub fn solid_bed_y(&self) -> f32 {
        self.climate_elevation() - self.void_height_total()
    }

    /// 0..1 sky transmittance through snow/ice sitting in the fluid cap.
    /// Deep snowpacks go effectively dark — plants and photosystems under
    /// metres of snow should not keep photosynthesising at full rate.
    pub fn cover_light_factor(&self) -> f32 {
        let mut snow = 0i64;
        let mut ice = 0i64;
        for j in 0..self.layer_count as usize {
            match self.layers[j].material {
                MaterialId::Snow => snow += self.layers[j].thickness,
                MaterialId::Ice => ice += self.layers[j].thickness,
                MaterialId::Water => {}
                _ => break,
            }
        }
        // ~250 kg ≈ 1 m of water-equivalent depth on a column.
        const KG_PER_M: f32 = 250.0;
        let snow_m = snow as f32 / KG_PER_M;
        let ice_m = ice as f32 / KG_PER_M;
        let t = (-1.5 * snow_m).exp() * (-1.0 * ice_m).exp();
        t.clamp(0.0, 1.0)
    }

    /// Elevation for near-surface ambient temperature (HUD, freeze/thaw,
    /// ecology comfort).
    ///
    /// - Submerged bed (ocean / shelf): always sample at sea level so
    ///   free-surface wobbles and abyssal beds don't move the thermometer
    ///   (deep beds clamp to the geothermal Dirichlet ~55 °C).
    /// - Emergent land: a thin weather skin above the solid bed so snow/ice
    ///   piles can't self-cool via lapse rate as they grow.
    pub fn ambient_elevation(&self, sea_level: f32) -> f32 {
        let bed = self.climate_elevation();
        const SKIN_M: f32 = 8.0;
        if bed < sea_level - 0.5 {
            sea_level
        } else {
            self.surface_y.min(bed + SKIN_M)
        }
    }

    /// Snow + ice in the weather cap (top contiguous frozen layers).
    pub fn frozen_surface_mass(&self) -> i64 {
        let mut total = 0i64;
        for i in 0..self.layer_count as usize {
            match self.layers[i].material {
                MaterialId::Snow | MaterialId::Ice => total += self.layers[i].thickness.max(0),
                MaterialId::Water => break,
                _ => break,
            }
        }
        total
    }

    /// Target pore-water mass for a regional water table at `table_y`
    /// (usually sea level). Ocean / submerged beds → full saturation;
    /// coastal land fills the aquifer up to the table; high ground keeps
    /// a modest base so the table doesn't start bone-dry.
    pub fn target_moisture_for_table(&self, table_y: f32) -> i64 {
        let cap = self.moisture_cap();
        if cap <= 0 {
            return 0;
        }
        let bed = self.climate_elevation();
        if bed < table_y - 0.05 || (self.top_water_mass() > 0 && bed <= table_y + 0.05) {
            return cap;
        }
        let Some(idx) = self.top_porous_layer_index() else {
            return 0;
        };
        let mut cap_height = 0.0f32;
        for i in 0..idx {
            cap_height +=
                self.mass_to_height_delta(self.layers[i].material, self.layers[i].thickness);
        }
        let layer_top_y = self.surface_y - cap_height;
        let layer = &self.layers[idx];
        let layer_height_m = self.mass_to_height_delta(layer.material, layer.thickness);
        if layer_height_m <= 1e-6 {
            return ((cap as f32) * 0.1).round() as i64;
        }
        let layer_bottom_y = layer_top_y - layer_height_m;
        let target = table_y.clamp(layer_bottom_y, layer_top_y);
        let sat_from_table = ((target - layer_bottom_y) / layer_height_m).clamp(0.0, 1.0);
        let sat = sat_from_table.max(0.10);
        ((cap as f32) * sat).round() as i64
    }

    /// Elevation of the groundwater table: the topmost porous solid
    /// layer's pore space fills from the bottom of that layer upward as
    /// `moisture` approaches its cap, reaching the ground surface exactly
    /// when fully saturated (any further inflow discharges as surface
    /// inflow discharges — see barrier commit).
    pub fn water_table_y(&self) -> f32 {
        if let Some((water_top, mass)) = self.flowable_water() {
            if mass > 0 {
                return water_top;
            }
        }
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

    /// Universal density settling: sort every layer above bedrock by
    /// material density, lightest on top and heaviest at the bottom,
    /// then merge same-material layers that end up adjacent.
    ///
    /// This is the physical rule you'd expect from a stack of stuff
    /// with mass: rocks sink through water, snow and ice float on top,
    /// air-density organic litter ends up on top of everything. It
    /// makes both the old "sediment must deposit below the fluid cap"
    /// bookkeeping and the old "canonical Snow-Ice-Water fluid cap
    /// order" fall out of a single rule instead of being separate
    /// bespoke passes.
    ///
    /// Total mass is preserved by construction. `age_start` of a
    /// merged layer is the minimum contributor's; `age_end` is `tick`.
    /// n ≤ MAX_LAYERS = 8 so the O(n²) insertion sort is trivial.
    pub fn settle_by_density(&mut self, tick: u64) {
        let count = self.layer_count as usize;
        if count <= 1 {
            return;
        }

        // Insertion sort ascending by density. Ascending because
        // layers[0] is the top of the stack and we want the lightest
        // material there (heaviest sinks to the bottom = highest index).
        for i in 1..count {
            let mut j = i;
            while j > 0 {
                let d_above = MaterialRegistry::props(self.layers[j - 1].material).density;
                let d_here = MaterialRegistry::props(self.layers[j].material).density;
                if d_above > d_here {
                    self.layers.swap(j - 1, j);
                    j -= 1;
                } else {
                    break;
                }
            }
        }

        // Merge adjacent same-material layers, respecting the geologic
        // MERGE_MAX_THICKNESS cap so we don't end up with a single
        // absurdly thick layer that hides all history.
        let mut write = 0usize;
        for read in 0..count {
            if write > 0 {
                let a = self.layers[write - 1];
                let b = self.layers[read];
                if a.material == b.material
                    && a.thickness + b.thickness <= wk_material::MERGE_MAX_THICKNESS
                {
                    self.layers[write - 1].thickness = a.thickness + b.thickness;
                    self.layers[write - 1].age_start = a.age_start.min(b.age_start);
                    self.layers[write - 1].age_end = a.age_end.max(b.age_end).max(tick);
                    continue;
                }
            }
            if read != write {
                self.layers[write] = self.layers[read];
            }
            write += 1;
        }
        for i in write..count {
            self.layers[i] = Layer::default();
        }
        self.layer_count = write as u8;
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

    /// Deposit solid sediment, density-settle it, and **displace** an equal
    /// height of standing water so the free surface does not spike.
    ///
    /// Corpse ooze / organic dams used `deposit_to_top` + settle under a
    /// lake: the bed rose and the water top rose with it (same water mass
    /// on a taller stack) → vertical water spikes on the sill. Open water
    /// should keep its top; the displaced kg is returned for the caller to
    /// spill into neighbours.
    ///
    /// Returns displaced water mass in kg (0 on dry land).
    pub fn deposit_sediment_settled(
        &mut self,
        material: MaterialId,
        mass: i64,
        tick: u64,
    ) -> i64 {
        if mass <= 0 || !material.is_solid() {
            return 0;
        }
        let had_water = self.flowable_water().map(|(_, m)| m).unwrap_or(0);
        let prev_surface = self.surface_y;
        let h = self.mass_to_height_delta(material, mass);
        self.deposit_to_top(material, mass, tick);
        self.settle_by_density(tick);
        if had_water <= 0 || h <= 0.0 {
            return 0;
        }
        // Match sediment height with water mass so surface returns ~prev.
        let water_density =
            MaterialRegistry::props(MaterialId::Water).density.max(1) as f32;
        let mut displace =
            (h * SAMPLE_WIDTH_M * water_density).round() as i64;
        displace = displace.min(had_water).max(0);
        if displace <= 0 {
            return 0;
        }
        let removed = -self.adjust_top_water(-displace, tick);
        // Numerical drift: pin free surface if we still sit above the old top.
        if self.surface_y > prev_surface + 1e-3 {
            let extra_h = self.surface_y - prev_surface;
            let extra = (extra_h * SAMPLE_WIDTH_M * water_density).round() as i64;
            if extra > 0 {
                let got = -self.adjust_top_water(-extra, tick);
                return removed + got;
            }
        }
        removed
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
    /// on top (density-settle will drop it below any lighter fluid cap).
    /// Negative: drains from the flowable Water layer in the fluid cap,
    /// even if it's sitting under a snow / ice cap — that cap doesn't
    /// seal the water body against lateral drainage in the flow model.
    /// Returns the actual signed change applied.
    pub fn adjust_top_water(&mut self, delta: i64, tick: u64) -> i64 {
        if delta > 0 {
            self.deposit_to_top(MaterialId::Water, delta, tick);
            // Sink deposited water under any lighter ice/snow cap
            // immediately. Without this, a water sheen briefly sits on
            // top of ice and the next phase-change tick freezes it —
            // an ice pump that drains the lake into an ever-thicker
            // ice tower during a hard freeze.
            self.settle_by_density(tick);
            delta
        } else if delta < 0 {
            let removed = self.take_water_from_cap(-delta);
            -removed
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
        // Let heavy things fall to the bottom and light things float
        // to the top. Every layer above bedrock has a density and
        // participates in this ordering, so rocks sink through water,
        // ice/snow float, and old buried layers of the same material
        // rejoin fresh ones — no special-case "fluid cap" logic.
        self.settle_by_density(0);
    }

    /// Keep `surface_y` consistent with layer heights plus void heights.
    /// A column with 30 m of rock and a 4 m void has top at bedrock+34.
    pub fn recompute_surface_y(&mut self, bedrock_y: f32) {
        let solid: f32 = (0..self.layer_count as usize)
            .map(|i| {
                self.mass_to_height_delta(self.layers[i].material, self.layers[i].thickness)
            })
            .sum();
        let voids: f32 = self.voids.iter().map(|v| v.height_m.max(0.0)).sum();
        self.surface_y = bedrock_y + solid + voids;
    }

    pub fn void_height_total(&self) -> f32 {
        self.voids.iter().map(|v| v.height_m.max(0.0)).sum()
    }

    pub fn void_water_total(&self) -> i64 {
        self.voids.iter().map(|v| v.water_mass.max(0)).sum()
    }

    /// Absolute top/bottom elevations of layer `idx`, inserting existing
    /// voids as gaps. Walks bottom-up from bedrock so void punch-outs
    /// land at their absolute `top_y` / `floor_y`.
    pub fn layer_y_range(&self, idx: usize, bedrock_y: f32) -> (f32, f32) {
        if idx >= self.layer_count as usize {
            return (bedrock_y, bedrock_y);
        }
        // Build solid segments bottom→top (highest layer index first).
        let mut segments: Vec<(f32, f32, usize)> = Vec::new();
        let mut y = bedrock_y;
        let mut void_floors: Vec<(f32, f32)> = self
            .voids
            .iter()
            .filter(|v| v.height_m > 0.0)
            .map(|v| (v.floor_y(), v.top_y))
            .collect();
        void_floors.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        let mut vi = 0usize;
        for li in (0..self.layer_count as usize).rev() {
            let h = self.mass_to_height_delta(self.layers[li].material, self.layers[li].thickness);
            // Advance past any voids whose floor is at/below current y.
            while vi < void_floors.len() && void_floors[vi].0 <= y + 1e-4 {
                let (vf, vt) = void_floors[vi];
                if vt > y {
                    y = vt.max(y);
                }
                let _ = vf;
                vi += 1;
            }
            let bot = y;
            y += h;
            segments.push((y, bot, li)); // top, bot, idx
        }
        for (top, bot, li) in segments {
            if li == idx {
                return (top, bot);
            }
        }
        (bedrock_y, bedrock_y)
    }

    /// Grow an existing void near `mid_y` by `dh`, or spawn a new one.
    /// Returns the height actually added.
    pub fn grow_void_at(
        &mut self,
        mid_y: f32,
        dh: f32,
        roof_material: MaterialId,
        origin: VoidOrigin,
    ) -> f32 {
        if dh <= 1e-6 {
            return 0.0;
        }
        // Merge into nearest void whose mid is within 1.5 m.
        let mut best: Option<usize> = None;
        let mut best_d = f32::MAX;
        for (i, v) in self.voids.iter().enumerate() {
            let d = (v.mid_y() - mid_y).abs();
            if d < 1.5 && d < best_d {
                best = Some(i);
                best_d = d;
            }
        }
        if let Some(i) = best {
            let v = &mut self.voids[i];
            let half = dh * 0.5;
            v.top_y += half;
            v.height_m += dh;
            v.roof_material = roof_material;
            v.origin = origin;
            return dh;
        }
        if self.voids.len() >= MAX_VOIDS {
            // Expand the closest void even if far — don't drop dissolution.
            let mut best = 0usize;
            let mut best_d = f32::MAX;
            for (i, v) in self.voids.iter().enumerate() {
                let d = (v.mid_y() - mid_y).abs();
                if d < best_d {
                    best = i;
                    best_d = d;
                }
            }
            let v = &mut self.voids[best];
            v.top_y += dh * 0.5;
            v.height_m += dh;
            v.roof_material = roof_material;
            v.origin = origin;
            return dh;
        }
        let half = dh * 0.5;
        self.voids.push(Void {
            top_y: mid_y + half,
            height_m: dh,
            water_mass: 0,
            roof_material,
            origin,
            light: 0,
        });
        dh
    }

    /// Drain up to `mass` kg of top/flowable water into open voids.
    /// Returns kg moved into voids.
    ///
    /// Only geometrically open mouths (`open_to_surface` vs the solid
    /// ground under any pond) capture. The old `light > 200` latch let
    /// worldgen sea-cliff / karst alcoves keep swallowing water forever
    /// — including after they flooded — which pumped the shoreline.
    /// Callers that must protect the coast should also gate with
    /// [`Self::solid_bed_y`] vs sea level before calling.
    pub fn drain_surface_water_into_voids(&mut self, mass: i64) -> i64 {
        if mass <= 0 {
            return 0;
        }
        // Judge openness against the solid/cavity stack top (climate
        // elevation strips standing water). Using free-water `surface_y`
        // would make every pond seal its own sinkhole mouth.
        let ground = self.climate_elevation();
        let open: Vec<usize> = self
            .voids
            .iter()
            .enumerate()
            .filter(|(_, v)| v.open_to_surface(ground))
            .map(|(i, _)| i)
            .collect();
        if open.is_empty() {
            return 0;
        }
        let mut remaining = mass;
        let mut moved = 0i64;
        let per = (remaining / open.len() as i64).max(1);
        for &i in &open {
            if remaining <= 0 {
                break;
            }
            let free = self.voids[i].free_capacity_kg();
            if free <= 0 {
                continue;
            }
            let take = self.take_water_from_cap(per.min(remaining).min(free));
            if take <= 0 {
                break;
            }
            self.voids[i].water_mass += take;
            remaining -= take;
            moved += take;
        }
        moved
    }

    /// First void index containing elevation `y`, if any.
    pub fn void_index_at(&self, y: f32) -> Option<usize> {
        self.voids.iter().position(|v| v.contains_y(y))
    }

    /// Move up to `mass` kg of already-accounted water into voids with
    /// free capacity (no moisture/surface bookkeeping). Used for pore
    /// overflow that would otherwise spring to the surface.
    pub fn fill_voids_from_mass(&mut self, mass: i64) -> i64 {
        if mass <= 0 || self.voids.is_empty() {
            return 0;
        }
        let mut remaining = mass;
        let mut moved = 0i64;
        for v in &mut self.voids {
            if remaining <= 0 {
                break;
            }
            let free = v.free_capacity_kg();
            if free <= 0 {
                continue;
            }
            let take = remaining.min(free);
            v.water_mass += take;
            remaining -= take;
            moved += take;
        }
        moved
    }

    /// Seep pore moisture into buried cavities when the water table
    /// intersects them (or when the column is already quite wet).
    /// Mass-conserving: `moisture` decreases, `void.water_mass` rises.
    pub fn seep_moisture_into_voids(&mut self, max_kg: i64) -> i64 {
        if max_kg <= 0 || self.voids.is_empty() {
            return 0;
        }
        let cap = self.moisture_cap();
        if cap <= 0 {
            return 0;
        }
        // Leave a pore reserve so we don't empty the aquifer into one cave.
        let reserve = ((cap as f32) * 0.20).round() as i64;
        let available = self.moisture.saturating_sub(reserve.max(0));
        if available <= 0 {
            return 0;
        }
        let table = self.water_table_y();
        let sat = (self.moisture as f32 / cap as f32).clamp(0.0, 1.0);
        let mut remaining = max_kg.min(available);
        let mut moved = 0i64;
        for v in &mut self.voids {
            if remaining <= 0 {
                break;
            }
            if v.height_m <= 1e-4 {
                continue;
            }
            // Table reaches into the cavity, or soils are wet enough for
            // capillary drip into the roof crack.
            let intersects = table > v.floor_y() + 0.05;
            if !intersects && sat < 0.30 {
                continue;
            }
            let free = v.free_capacity_kg();
            if free <= 0 {
                continue;
            }
            let take = remaining.min(free);
            v.water_mass += take;
            remaining -= take;
            moved += take;
        }
        if moved > 0 {
            self.moisture = (self.moisture - moved).max(0);
            self.activity = Activity::HydrologyActive;
        }
        moved
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
    fn adjust_top_water_settles_under_ice() {
        let mut col = Column::default();
        col.deposit_to_top(MaterialId::Sand, 1000, 0);
        col.deposit_to_top(MaterialId::Water, 500, 1);
        col.deposit_to_top(MaterialId::Ice, 200, 2);
        col.settle_by_density(2);
        assert_eq!(col.top_material(), MaterialId::Ice);
        let _ = col.adjust_top_water(100, 3);
        assert_eq!(
            col.top_material(),
            MaterialId::Ice,
            "fresh water must sink under the ice skin"
        );
        assert_eq!(col.flowable_water().map(|(_, m)| m), Some(600));
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
    fn sediment_under_water_does_not_spike_surface() {
        let mut col = Column::default();
        col.deposit_to_top(MaterialId::Sand, 2_000, 0);
        col.deposit_to_top(MaterialId::Water, 2_500, 1); // 10 m of water
        let surface_before = col.surface_y;
        let water_before = col.flowable_water().map(|(_, m)| m).unwrap_or(0);
        let displaced =
            col.deposit_sediment_settled(MaterialId::Organic, 655, 2);
        assert!(displaced > 0, "should displace standing water");
        assert!(
            (col.surface_y - surface_before).abs() < 0.05,
            "free surface must not spike (before={surface_before:.3} after={:.3})",
            col.surface_y
        );
        let water_after = col.flowable_water().map(|(_, m)| m).unwrap_or(0);
        assert_eq!(
            water_before - water_after,
            displaced,
            "displaced kg must leave this column"
        );
        assert_eq!(col.layers[0].material, MaterialId::Water);
        assert!(
            (0..col.layer_count as usize).any(|i| col.layers[i].material == MaterialId::Organic),
            "organic settled under water"
        );
    }

    #[test]
    fn settle_by_density_sinks_rock_through_water() {
        // Someone drops rock onto standing water. Rock (2600 kg/m3)
        // is heavier than water (1000), so it sinks through and ends
        // up at the bottom of the stack.
        let mut col = Column::default();
        col.deposit_to_top(MaterialId::Sand, 1000, 0);
        col.deposit_to_top(MaterialId::Water, 250, 5);
        col.deposit_to_top(MaterialId::Stone, 200, 10);
        // Before settle: Stone on top by insertion order.
        assert_eq!(col.layers[0].material, MaterialId::Stone);

        col.settle_by_density(20);

        // After: heaviest at bottom, lightest on top.
        assert_eq!(col.layers[0].material, MaterialId::Water);
        assert_eq!(col.layers[1].material, MaterialId::Sand);
        assert_eq!(col.layers[2].material, MaterialId::Stone);
    }

    #[test]
    fn settle_by_density_snow_and_ice_float_on_water() {
        let mut col = Column::default();
        col.deposit_to_top(MaterialId::Sand, 1000, 0);
        // Deliberately place in the *wrong* order — settle should fix it.
        col.deposit_to_top(MaterialId::Snow, 100, 1);
        col.deposit_to_top(MaterialId::Water, 250, 2);
        col.deposit_to_top(MaterialId::Ice, 50, 3);

        col.settle_by_density(10);

        // Densities: Snow 900 < Ice 917 < Water 1000 < Sand 1600.
        assert_eq!(col.layers[0].material, MaterialId::Snow);
        assert_eq!(col.layers[1].material, MaterialId::Ice);
        assert_eq!(col.layers[2].material, MaterialId::Water);
        assert_eq!(col.layers[3].material, MaterialId::Sand);
    }

    #[test]
    fn settle_by_density_unearths_buried_snow() {
        // Screenshot regression: rain fell on snow, froze, more snow on
        // top — the old snow was trapped beneath a rain-freeze-snow
        // sandwich until settle_by_density brought all the snow back to
        // the surface in one merged layer.
        let mut col = Column::default();
        col.deposit_to_top(MaterialId::Sand, 1000, 0);
        col.deposit_to_top(MaterialId::Snow, 1386, 100);
        col.deposit_to_top(MaterialId::Water, 27, 200);
        col.deposit_to_top(MaterialId::Ice, 1, 300);
        col.deposit_to_top(MaterialId::Snow, 35, 400);
        assert_eq!(col.layer_count, 5);

        col.settle_by_density(500);

        assert_eq!(col.layer_count, 4);
        assert_eq!(col.layers[0].material, MaterialId::Snow);
        assert_eq!(col.layers[0].thickness, 35 + 1386);
        assert_eq!(col.layers[1].material, MaterialId::Ice);
        assert_eq!(col.layers[2].material, MaterialId::Water);
        assert_eq!(col.layers[3].material, MaterialId::Sand);
    }

    #[test]
    fn settle_by_density_preserves_mass() {
        let mut col = Column::default();
        col.deposit_to_top(MaterialId::Sand, 1000, 0);
        col.deposit_to_top(MaterialId::Water, 500, 1);
        col.deposit_to_top(MaterialId::Snow, 200, 2);
        col.deposit_to_top(MaterialId::Water, 100, 3);
        let before: i64 = (0..col.layer_count as usize).map(|i| col.layers[i].thickness).sum();
        col.settle_by_density(10);
        let after: i64 = (0..col.layer_count as usize).map(|i| col.layers[i].thickness).sum();
        assert_eq!(before, after);
    }

    #[test]
    fn settle_by_density_noop_on_single_layer() {
        let mut col = Column::default();
        col.deposit_to_top(MaterialId::Sand, 1000, 0);
        let before_count = col.layer_count;
        col.settle_by_density(10);
        assert_eq!(col.layer_count, before_count);
    }

    #[test]
    fn top_porous_skips_water_cap() {
        let mut col = Column::default();
        col.deposit_to_top(MaterialId::Sand, 1000, 0);
        col.deposit_to_top(MaterialId::Water, 250, 5);
        assert_eq!(col.top_porous_layer().unwrap().material, MaterialId::Sand);
    }

    #[test]
    fn thin_organic_skin_does_not_shrink_moisture_cap() {
        let mut sand_only = Column::default();
        sand_only.deposit_to_top(MaterialId::Sand, 8_000, 0);
        let sand_cap = sand_only.moisture_cap();

        let mut with_skin = Column::default();
        with_skin.deposit_to_top(MaterialId::Sand, 8_000, 0);
        with_skin.deposit_to_top(MaterialId::Organic, 400, 1); // thin beach litter
        let skin_cap = with_skin.moisture_cap();

        // Top-layer-only cap for 400 kg organic ≈ 313 kg. Rooting zone must
        // still see the sand body underneath.
        let organic_only = (400i64 * 200) / 255;
        assert!(sand_cap > organic_only * 2, "sand rooting zone > organic skin");
        assert!(
            skin_cap > organic_only * 2,
            "organic skin must not flash-dry the column (sand={sand_cap} skin={skin_cap} organic_only={organic_only})"
        );
    }

    #[test]
    fn cover_light_factor_dims_under_deep_snow() {
        let mut col = Column::default();
        col.deposit_to_top(MaterialId::Sand, 1000, 0);
        assert!((col.cover_light_factor() - 1.0).abs() < 1e-5);
        col.deposit_to_top(MaterialId::Snow, 8_000, 1);
        assert!(
            col.cover_light_factor() < 0.02,
            "deep snow should block nearly all light, got {}",
            col.cover_light_factor()
        );
    }

    #[test]
    fn land_puddle_height_matches_render_math() {
        let mut col = Column::default();
        col.deposit_to_top(MaterialId::Sand, 50_000, 0); // tall sand
        col.deposit_to_top(MaterialId::Water, 857, 1);
        let h = col.mass_to_height_delta(MaterialId::Water, 857);
        assert!((h - 3.428).abs() < 0.01, "h={h}");
        let (top, mass) = col.flowable_water().unwrap();
        assert_eq!(mass, 857);
        // void-free: free surface must match surface_y
        assert!(
            (top - col.surface_y).abs() < 1e-3,
            "top={top} surface_y={}",
            col.surface_y
        );
        assert!((col.solid_bed_y() + h - top).abs() < 1e-3);
    }

    #[test]
    fn flowable_top_with_void_diverges_from_surface_y() {
        let mut col = Column::default();
        col.deposit_to_top(MaterialId::Sand, 50_000, 0);
        let bed_before = col.surface_y;
        col.voids.push(Void {
            top_y: bed_before,
            height_m: 10.0,
            water_mass: 0,
            roof_material: MaterialId::Sand,
            origin: VoidOrigin::Karst,
            light: 0,
        });
        col.recompute_surface_y(0.0);
        col.deposit_to_top(MaterialId::Water, 857, 1);
        let h = col.mass_to_height_delta(MaterialId::Water, 857);
        let (top, _) = col.flowable_water().unwrap();
        let gap = col.surface_y - top;
        assert!(
            (gap - 10.0).abs() < 0.05,
            "expected ~10m gap from voids, got gap={gap} surface={} top={top} solid_bed={} h={h}",
            col.surface_y,
            col.solid_bed_y(),
        );
    }
}