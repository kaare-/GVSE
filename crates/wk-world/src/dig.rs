//! Burrow dig API (stage 9) + root bore (Set D).
//!
//! Creatures (and tests) call [`World::dig`] to remove substrate mass,
//! open or extend a `Void { origin: Burrow }`, and dump tailings on the
//! surface. Spans wider than the roof material's `roof_span_max_m`
//! collapse into an open trench.
//!
//! Living roots call [`World::root_bore`] to convert solid substrate into
//! [`MaterialId::Organic`] in place (ghost-root prep for later fungi).

use wk_material::{MaterialId, MaterialRegistry, MAX_LAYERS, SAMPLE_WIDTH_M};

use crate::column::{Activity, Layer, VoidOrigin};
use crate::world::World;

/// Outcome of a single [`World::dig`] call.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DigResult {
    /// kg actually removed from the substrate.
    pub removed_kg: i64,
    /// Material that was dug (also deposited as surface tailings).
    pub material: MaterialId,
    /// Void height added this dig (metres). Zero if refused / trench-only.
    pub void_delta_m: f32,
    /// True when the dig exceeded the roof span and opened to surface.
    pub collapsed_to_trench: bool,
    /// True when nothing could be dug (missing column, bedrock, fluids…).
    pub refused: bool,
}

impl DigResult {
    fn refused() -> Self {
        Self {
            removed_kg: 0,
            material: MaterialId::Sand,
            void_delta_m: 0.0,
            collapsed_to_trench: false,
            refused: true,
        }
    }
}

fn is_diggable(mat: MaterialId) -> bool {
    mat.is_solid()
        && !matches!(
            mat,
            MaterialId::Bedrock | MaterialId::Water | MaterialId::Ice | MaterialId::Snow | MaterialId::Air
        )
        && MaterialRegistry::props(mat).erosion_resistance < u32::MAX
}

/// Energy multiplier to drive a root tip through `mat` (higher = harder).
/// `None` = refuse (bedrock / fluids).
pub fn root_penetrate_cost(mat: MaterialId) -> Option<f32> {
    match mat {
        MaterialId::Organic => Some(0.7),
        MaterialId::Sand | MaterialId::Clay => Some(1.8),
        MaterialId::Gravel => Some(2.6),
        MaterialId::LooseRock | MaterialId::Limestone => Some(6.5),
        MaterialId::Stone => Some(12.0),
        MaterialId::Bedrock
        | MaterialId::Water
        | MaterialId::Ice
        | MaterialId::Snow
        | MaterialId::Air => None,
    }
}

/// Outcome of a single [`World::root_bore`] call.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RootBoreResult {
    pub converted_kg: i64,
    pub from_material: MaterialId,
    pub refused: bool,
}

impl RootBoreResult {
    fn refused() -> Self {
        Self {
            converted_kg: 0,
            from_material: MaterialId::Sand,
            refused: true,
        }
    }
}

/// Horizontal span (metres) of contiguous columns that have a void whose
/// mid-elevation is within `tol_m` of `mid_y`.
fn void_span_m(world: &World, world_x: i32, mid_y: f32, tol_m: f32) -> f32 {
    let mut left = world_x;
    while let Some(col) = world.column_at(left - 1) {
        if col.voids.iter().any(|v| (v.mid_y() - mid_y).abs() <= tol_m && v.height_m > 1e-4) {
            left -= 1;
        } else {
            break;
        }
    }
    let mut right = world_x;
    while let Some(col) = world.column_at(right + 1) {
        if col.voids.iter().any(|v| (v.mid_y() - mid_y).abs() <= tol_m && v.height_m > 1e-4) {
            right += 1;
        } else {
            break;
        }
    }
    // Include the centre column.
    let cols = (right - left + 1).max(1) as f32;
    cols * SAMPLE_WIDTH_M
}

impl World {
    /// Dig `volume_kg` of substrate at absolute elevation `target_y` in
    /// column `world_x`. Removed mass is deposited on the column surface
    /// as a tailings mound; a burrow void is grown at the dig elevation.
    ///
    /// If the resulting passage would exceed the roof material's
    /// `roof_span_max_m`, the void is opened to the surface (trench).
    pub fn dig(&mut self, world_x: i32, target_y: f32, volume_kg: i64) -> DigResult {
        if volume_kg <= 0 {
            return DigResult::refused();
        }
        let coord = Self::chunk_coord_for_world_x(world_x);
        let local = Self::local_x(world_x);
        let bedrock = match self.chunks.get(&coord) {
            Some(c) => c.bedrock_y,
            None => return DigResult::refused(),
        };

        // Find a diggable layer containing target_y.
        let (layer_idx, material, take) = {
            let Some(col) = self.column_at(world_x) else {
                return DigResult::refused();
            };
            let mut found = None;
            for li in 0..col.layer_count as usize {
                let mat = col.layers[li].material;
                if !is_diggable(mat) {
                    continue;
                }
                let (top, bot) = col.layer_y_range(li, bedrock);
                if target_y <= top + 1e-3 && target_y >= bot - 1e-3 {
                    let take = volume_kg.min(col.layers[li].thickness);
                    if take > 0 {
                        found = Some((li, mat, take));
                    }
                    break;
                }
            }
            // Fallback: dig the solid layer whose mid is closest to target_y.
            if found.is_none() {
                let mut best: Option<(usize, MaterialId, i64, f32)> = None;
                for li in 0..col.layer_count as usize {
                    let mat = col.layers[li].material;
                    if !is_diggable(mat) || col.layers[li].thickness <= 0 {
                        continue;
                    }
                    let (top, bot) = col.layer_y_range(li, bedrock);
                    let mid = 0.5 * (top + bot);
                    let d = (mid - target_y).abs();
                    if best.map(|b| d < b.3).unwrap_or(true) {
                        best = Some((li, mat, volume_kg.min(col.layers[li].thickness), d));
                    }
                }
                if let Some((li, mat, take, _)) = best {
                    found = Some((li, mat, take));
                }
            }
            match found {
                Some(v) => v,
                None => return DigResult::refused(),
            }
        };

        let (removed, dh, mid_y, roof_mat) = {
            let Some(chunk) = self.chunks.get_mut(&coord) else {
                return DigResult::refused();
            };
            let col = &mut chunk.columns[local];
            if layer_idx >= col.layer_count as usize
                || col.layers[layer_idx].material != material
            {
                return DigResult::refused();
            }
            let take = take.min(col.layers[layer_idx].thickness);
            if take <= 0 {
                return DigResult::refused();
            }
            let (top, bot) = col.layer_y_range(layer_idx, bedrock);
            let mid = 0.5 * (top + bot);
            // Roof = solid immediately above this layer (lower index).
            let roof = if layer_idx > 0 {
                col.layers[layer_idx - 1].material
            } else {
                material
            };
            let dh = col.mass_to_height_delta(material, take);
            col.layers[layer_idx].thickness -= take;
            if col.layers[layer_idx].thickness <= 0 {
                for j in layer_idx..(col.layer_count as usize).saturating_sub(1) {
                    col.layers[j] = col.layers[j + 1];
                }
                if col.layer_count > 0 {
                    col.layer_count -= 1;
                }
            }
            col.grow_void_at(mid, dh, roof, VoidOrigin::Burrow);
            col.activity = Activity::HydrologyActive;
            // Tailings mound on the surface — same material that was dug.
            col.deposit_to_top(material, take, 0);
            col.recompute_surface_y(bedrock);
            (take, dh, mid, roof)
        };

        // Roof-span check: sand/clay (span 0) always trenches; longer
        // tunnels under stone/limestone are fine until the limit.
        let span = void_span_m(self, world_x, mid_y, 1.5);
        let limit = MaterialRegistry::props(roof_mat).roof_span_max_m;
        let mut collapsed = false;
        if span > limit + 1e-4 {
            collapsed = self.open_burrow_as_trench(world_x, mid_y);
        }

        DigResult {
            removed_kg: removed,
            material,
            void_delta_m: dh,
            collapsed_to_trench: collapsed,
            refused: false,
        }
    }

    /// Stretch the burrow void at `mid_y` up to the surface and mark it
    /// ventilated — a collapsed trench / doline mouth.
    fn open_burrow_as_trench(&mut self, world_x: i32, mid_y: f32) -> bool {
        let coord = Self::chunk_coord_for_world_x(world_x);
        let local = Self::local_x(world_x);
        let bedrock = match self.chunks.get(&coord) {
            Some(c) => c.bedrock_y,
            None => return false,
        };
        let Some(chunk) = self.chunks.get_mut(&coord) else {
            return false;
        };
        let col = &mut chunk.columns[local];
        let mut opened = false;
        for v in &mut col.voids {
            if (v.mid_y() - mid_y).abs() <= 1.5 && v.height_m > 1e-4 {
                let floor = v.floor_y();
                // Grow upward to the current surface (before recompute
                // the surface already includes this void's height).
                let surface = col.surface_y;
                if surface > floor {
                    v.top_y = surface;
                    v.height_m = (surface - floor).max(v.height_m);
                }
                v.light = 255;
                v.origin = VoidOrigin::Collapse;
                opened = true;
            }
        }
        if opened {
            col.activity = Activity::HydrologyActive;
            col.recompute_surface_y(bedrock);
        }
        opened
    }

    /// Solid material at absolute elevation `target_y`, if any.
    pub fn material_at(&self, world_x: i32, target_y: f32) -> Option<MaterialId> {
        let coord = Self::chunk_coord_for_world_x(world_x);
        let bedrock = self.chunks.get(&coord)?.bedrock_y;
        let col = self.column_at(world_x)?;
        for li in 0..col.layer_count as usize {
            let mat = col.layers[li].material;
            if !mat.is_solid() {
                continue;
            }
            let (top, bot) = col.layer_y_range(li, bedrock);
            if target_y <= top + 1e-3 && target_y >= bot - 1e-3 {
                return Some(mat);
            }
        }
        None
    }

    /// Convert up to `volume_kg` of diggable solid at `target_y` into
    /// [`MaterialId::Organic`] in place. No void, no surface dump — live
    /// root tissue occupies the bore (fungi later open the cavity).
    pub fn root_bore(&mut self, world_x: i32, target_y: f32, volume_kg: i64, tick: u64) -> RootBoreResult {
        if volume_kg <= 0 {
            return RootBoreResult::refused();
        }
        let coord = Self::chunk_coord_for_world_x(world_x);
        let local = Self::local_x(world_x);
        let bedrock = match self.chunks.get(&coord) {
            Some(c) => c.bedrock_y,
            None => return RootBoreResult::refused(),
        };

        let (layer_idx, material, take) = {
            let Some(col) = self.column_at(world_x) else {
                return RootBoreResult::refused();
            };
            let mut found = None;
            for li in 0..col.layer_count as usize {
                let mat = col.layers[li].material;
                if !is_diggable(mat) || mat == MaterialId::Organic {
                    // Already organic — nothing to convert; treat as free path.
                    if mat == MaterialId::Organic {
                        let (top, bot) = col.layer_y_range(li, bedrock);
                        if target_y <= top + 1e-3 && target_y >= bot - 1e-3 {
                            return RootBoreResult {
                                converted_kg: 0,
                                from_material: MaterialId::Organic,
                                refused: false,
                            };
                        }
                    }
                    continue;
                }
                let (top, bot) = col.layer_y_range(li, bedrock);
                if target_y <= top + 1e-3 && target_y >= bot - 1e-3 {
                    let take = volume_kg.min(col.layers[li].thickness);
                    if take > 0 {
                        found = Some((li, mat, take));
                    }
                    break;
                }
            }
            match found {
                Some(v) => v,
                None => return RootBoreResult::refused(),
            }
        };

        let Some(chunk) = self.chunks.get_mut(&coord) else {
            return RootBoreResult::refused();
        };
        let col = &mut chunk.columns[local];
        if layer_idx >= col.layer_count as usize
            || col.layers[layer_idx].material != material
        {
            return RootBoreResult::refused();
        }
        let take = take.min(col.layers[layer_idx].thickness);
        if take <= 0 {
            return RootBoreResult::refused();
        }

        // Peel mass from the host solid.
        col.layers[layer_idx].thickness -= take;
        col.layers[layer_idx].age_end = tick;
        if col.layers[layer_idx].thickness <= 0 {
            for j in layer_idx..(col.layer_count as usize).saturating_sub(1) {
                col.layers[j] = col.layers[j + 1];
            }
            if col.layer_count > 0 {
                col.layer_count -= 1;
            }
        }

        // Insert / merge Organic at the same stratigraphic index.
        let insert_at = layer_idx.min(col.layer_count as usize);
        if insert_at < col.layer_count as usize
            && col.layers[insert_at].material == MaterialId::Organic
        {
            col.layers[insert_at].thickness += take;
            col.layers[insert_at].age_end = tick;
        } else if insert_at > 0
            && col.layers[insert_at - 1].material == MaterialId::Organic
        {
            col.layers[insert_at - 1].thickness += take;
            col.layers[insert_at - 1].age_end = tick;
        } else if (col.layer_count as usize) < MAX_LAYERS {
            for i in (insert_at..col.layer_count as usize).rev() {
                col.layers[i + 1] = col.layers[i];
            }
            col.layers[insert_at] = Layer {
                material: MaterialId::Organic,
                thickness: take,
                age_start: tick,
                age_end: tick,
            };
            col.layer_count += 1;
        } else {
            // Stack full — fold into the nearest organic or top solid.
            if let Some(oi) = (0..col.layer_count as usize)
                .find(|&i| col.layers[i].material == MaterialId::Organic)
            {
                col.layers[oi].thickness += take;
                col.layers[oi].age_end = tick;
            } else if col.layer_count > 0 {
                col.layers[0].thickness += take;
                col.layers[0].age_end = tick;
            }
        }

        col.activity = Activity::HydrologyActive;
        col.recompute_surface_y(bedrock);
        // Mass conserved (solid → organic); no audit delta.
        RootBoreResult {
            converted_kg: take,
            from_material: material,
            refused: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terrain::generate_flat_sand;

    #[test]
    fn dig_removes_mass_and_dumps_tailings() {
        let mut world = World::new(42);
        world.insert_chunk(generate_flat_sand(0, 0.0, 10.0));
        let wx = 10;
        let before_layers: i64 = {
            let col = world.column_at(wx).unwrap();
            (0..col.layer_count as usize)
                .map(|i| col.layers[i].thickness)
                .sum()
        };
        let y = world.column_at(wx).unwrap().climate_elevation() - 1.0;
        let res = world.dig(wx, y, 500);
        assert!(!res.refused);
        assert_eq!(res.removed_kg, 500);
        assert!(res.void_delta_m > 0.0 || res.collapsed_to_trench);
        let col = world.column_at(wx).unwrap();
        // Tailings redeposited on top — total layer mass unchanged
        // (removed then deposited), plus a void annotation.
        let after_layers: i64 = (0..col.layer_count as usize)
            .map(|i| col.layers[i].thickness)
            .sum();
        assert_eq!(before_layers, after_layers);
        assert!(!col.voids.is_empty() || res.collapsed_to_trench);
    }

    #[test]
    fn root_bore_converts_sand_to_organic_in_place() {
        let mut world = World::new(7);
        world.insert_chunk(generate_flat_sand(0, 0.0, 10.0));
        let wx = 4;
        let y = world.column_at(wx).unwrap().surface_y - 0.5;
        let sand_before: i64 = {
            let c = world.column_at(wx).unwrap();
            (0..c.layer_count as usize)
                .filter(|&i| c.layers[i].material == MaterialId::Sand)
                .map(|i| c.layers[i].thickness)
                .sum()
        };
        let res = world.root_bore(wx, y, 200, 10);
        assert!(!res.refused, "bore should succeed");
        assert_eq!(res.converted_kg, 200);
        assert_eq!(res.from_material, MaterialId::Sand);
        let col = world.column_at(wx).unwrap();
        let organic: i64 = (0..col.layer_count as usize)
            .filter(|&i| col.layers[i].material == MaterialId::Organic)
            .map(|i| col.layers[i].thickness)
            .sum();
        let sand_after: i64 = (0..col.layer_count as usize)
            .filter(|&i| col.layers[i].material == MaterialId::Sand)
            .map(|i| col.layers[i].thickness)
            .sum();
        assert!(organic >= 200, "organic={organic}");
        assert_eq!(sand_before - sand_after, 200);
        assert!(col.voids.is_empty(), "live root bore must not open a void");
    }

    #[test]
    fn stone_costs_more_to_penetrate_than_organic() {
        let o = root_penetrate_cost(MaterialId::Organic).unwrap();
        let s = root_penetrate_cost(MaterialId::Sand).unwrap();
        let st = root_penetrate_cost(MaterialId::Stone).unwrap();
        assert!(o < s && s < st);
        assert!(root_penetrate_cost(MaterialId::Bedrock).is_none());
    }
}
