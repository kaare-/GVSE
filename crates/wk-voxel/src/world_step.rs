//! One product world step — the owner `wk-voxel-app` and the profile
//! harness must share.
//!
//! Isolation: wk-voxel only. No column-stack imports.
//!
//! This is **not** a tick budget and not a client protocol. It is the
//! existing play-app order, lifted out of `main.rs` so the CA, climate,
//! and life cannot drift. `tick_with_life` stays the water/grain
//! subroutine; atmosphere stays outside its mass checkpoints. The play
//! app wraps this in [`crate::SimClock`] (max one step per present;
//! leftover dropped). The budget is **not** inside `step_world` — CA
//! tests stay 1:1.
//!
//! Order (cadence comments are load-bearing):
//!
//! 1. Wind heatmap rebuild if due (`WIND_FIELD_PERIOD` or empty field)
//! 2. E evap → humidity (climate rate: T + near-surface wind)
//! 3. Humidity advect with the local wind field
//! 4. CloudStore leftover parcel dump / buoyant rise
//! 5. Thermal surplus → water (must not wait on the drizzle lottery)
//! 6. C condensation (falling rain, or a flake below freeze)
//! 7. K karst
//! 8. Geotech map (if due) → support / landscape fall
//! 9. `tick_with_life` (CA; increments `world.tick`)
//! 10. Shift organisms that rode competent rock
//! 11. Airborne snow drift (once; not inside grain settle)
//! 12. Carbon buckets
//! 13. Wind rafts / floating Organic / current shove
//! 14. Flow erosion + clay suspension
//! 15. Geotech map again (post-CA dirty)
//! 16. Humidity diffuse if due (**after** the CA increment)
//! 17. Temperature step if due, then surplus again (hold shrank)
//! 18. Cold avalanche (same period as phase)
//! 19. Phase (I) — after T so a Tab snap applies this step
//! 20. Organisms (`O`) last so they see ice that just formed / melted
//!
//! Shell scans enable rayon; the CA follows [`PerfConfig::parallel_physics`].
//! Climatic `apply_rain` is **not** part of this step (tests/scenarios only).

use std::time::{Duration, Instant};

use crate::active::plan_active;
use crate::carbon::{step_carbon_budget, CarbonBudget, CarbonConfig};
use crate::climate::ClimateConfig;
use crate::clouds::{CloudConfig, CloudStore};
use crate::failure::{FailureConfig, FailureStats};
use crate::fungi::FungiConfig;
use crate::geotech_map::{geotech_map_due, GeotechMap};
use crate::grid::World;
use crate::humidity::{humidity_diffuse_due, Humidity};
use crate::landscape_body::{apply_landscape_fall, LandscapeBodyStore};
use crate::organism::{OrganismStepOutcome, OrganismStore};
use crate::parallel::set_parallel_enabled;
use crate::phase::{apply_phase, PhaseConfig};
use crate::plant::{collect_live_root_world_cells, sail_plants_on_wind_rafts_cfg};
use crate::rules::{
    apply_cold_avalanche_bound, apply_condensation_rain_phased, apply_evaporation_into_humidity_climate,
    apply_flow_erosion_bound, apply_karst_dissolution, apply_snow_wind_drift,
    drift_floating_organic_cfg, precipitate_thermal_surplus, shove_floating_organic_with_current,
    tick_with_life, tick_with_life_profiled, CompetentFallConfig, CondensationConfig, EvapConfig,
    GrainConfig, KarstConfig, OrographicConfig, PerfConfig, PhysicsTimings,
};
use crate::sediment::apply_suspension;
use crate::support_map::{support_map_due, SupportMap};
use crate::temperature::{temperature_step_due, Temperature};
use crate::wind::{Wind, WIND_FIELD_PERIOD};

/// Sidecars one [`step_world`] mutates. Configs live on [`WorldStepConfig`].
pub struct WorldStep<'a> {
    pub world: &'a mut World,
    pub humidity: &'a mut Humidity,
    pub wind: &'a mut Wind,
    pub temperature: &'a mut Temperature,
    pub clouds: &'a mut CloudStore,
    pub carbon: &'a mut CarbonBudget,
    pub organisms: Option<&'a mut OrganismStore>,
    pub landscape: Option<&'a mut LandscapeBodyStore>,
    pub geotech: Option<&'a mut GeotechMap>,
    pub support: Option<&'a mut SupportMap>,
}

/// Toggles and knobs for one [`step_world`]. Prep (wind mean, oro sign,
/// temp config) stays on the caller so Tab can mutate live without
/// this function knowing about hotkeys.
pub struct WorldStepConfig<'a> {
    pub perf: &'a PerfConfig,
    pub failure: &'a FailureConfig,
    pub evap: &'a EvapConfig,
    pub cond: &'a CondensationConfig,
    pub oro: Option<&'a OrographicConfig>,
    pub karst: &'a KarstConfig,
    pub cloud: &'a CloudConfig,
    pub phase: &'a PhaseConfig,
    pub climate: &'a ClimateConfig,
    pub carbon: &'a CarbonConfig,
    pub grain: &'a GrainConfig,
    pub fungi: &'a FungiConfig,
    pub competent: &'a CompetentFallConfig,
    pub humidity_diffusion_alpha: f32,
    pub sea_level_y: i32,
    pub sky_ceiling_y: i32,
    /// `E` — surface water → humidity.
    pub evap_on: bool,
    /// `C` — condensation / dew (the real rain).
    pub cond_rain_on: bool,
    /// `K` — limestone + groundwater dissolve.
    pub karst_on: bool,
    /// `O` — step living creatures. Ignored if [`WorldStep::organisms`] is `None`.
    pub organisms_on: bool,
}

/// What one [`step_world`] did. Spore FX and geotech HUD read this;
/// the play app currently ignores failure stats.
pub struct WorldStepOutcome {
    pub failure: FailureStats,
    pub organisms: Option<OrganismStepOutcome>,
    /// Climate wind at the **start** of the step (before CA increments
    /// `world.tick`). Rafts, drizzle seating, and spore puffs used this.
    pub wind_vx: f32,
    pub wind_vy: f32,
}

/// Optional wall buckets for the profile harness. `physics` is the
/// same [`PhysicsTimings`] `tick_with_life_profiled` already fills.
#[derive(Debug, Clone, Default)]
pub struct WorldStepTimings {
    pub wind_rebuild: Duration,
    pub evap: Duration,
    pub humidity_advect: Duration,
    pub clouds: Duration,
    pub condensation: Duration,
    pub karst: Duration,
    pub landscape: Duration,
    pub physics_tick: Duration,
    pub physics: PhysicsTimings,
    pub snow_drift: Duration,
    pub carbon: Duration,
    pub rafts: Duration,
    pub erosion: Duration,
    pub suspension: Duration,
    pub humidity_diffuse: Duration,
    pub humidity_diffuse_calls: u64,
    pub temperature: Duration,
    pub temperature_calls: u64,
    pub cold_avalanche: Duration,
    pub phase: Duration,
    pub organisms: Duration,
}

/// Advance the world one product step. Same order as the play app.
pub fn step_world(
    step: WorldStep<'_>,
    cfg: &WorldStepConfig<'_>,
    mut timings: Option<&mut WorldStepTimings>,
) -> WorldStepOutcome {
    let WorldStep {
        world,
        humidity,
        wind,
        temperature,
        clouds,
        carbon,
        mut organisms,
        mut landscape,
        mut geotech,
        mut support,
    } = step;

    let tick_no = world.tick;
    let wind_vx = wind.effective_vx(tick_no);
    let wind_vy = wind.effective_vy(tick_no);
    let organisms_on = cfg.organisms_on && organisms.is_some();
    let profile = timings.is_some();

    // Frame-shell scans touch many loaded chunks — always worth rayon.
    // CA physics stays on the Tab toggle (demo dirty plans are too
    // narrow for parallel to win).
    set_parallel_enabled(true);

    {
        let t0 = profile.then(Instant::now);
        if tick_no % WIND_FIELD_PERIOD == 0 || wind.field.is_empty() {
            let occupied: Vec<(i32, i32)> = humidity.cells.keys().copied().collect();
            wind.rebuild_field(Some(world), Some(temperature), tick_no, &occupied, None);
        }
        if let (true, Some(t0), Some(t)) = (profile, t0, timings.as_mut()) {
            t.wind_rebuild += t0.elapsed();
        }
    }

    {
        let t0 = profile.then(Instant::now);
        if cfg.evap_on {
            let evap_wind = wind.near_surface_abs(Some(world));
            apply_evaporation_into_humidity_climate(
                world,
                humidity,
                cfg.evap,
                Some(temperature),
                evap_wind,
            );
        }
        if let (true, Some(t0), Some(t)) = (profile, t0, timings.as_mut()) {
            t.evap += t0.elapsed();
        }
    }

    {
        let t0 = profile.then(Instant::now);
        humidity.advect_with_surface(wind_vx, wind_vy, wind, world);
        if let (true, Some(t0), Some(t)) = (profile, t0, timings.as_mut()) {
            t.humidity_advect += t0.elapsed();
        }
    }

    {
        let t0 = profile.then(Instant::now);
        clouds.step_with_precip(
            world,
            humidity,
            wind,
            cfg.sea_level_y,
            cfg.sky_ceiling_y,
            tick_no,
            cfg.cloud,
            Some(temperature),
            Some(cfg.phase),
        );
        if let (true, Some(t0), Some(t)) = (profile, t0, timings.as_mut()) {
            t.clouds += t0.elapsed();
        }
    }

    {
        let t0 = profile.then(Instant::now);
        // Surplus the local air cannot hold becomes water here.
        // The drizzle lottery below is a different gate — a missed
        // roll must not be how we "solve" a cold snap.
        precipitate_thermal_surplus(world, humidity, temperature, Some(cfg.phase));
        if cfg.cond_rain_on {
            apply_condensation_rain_phased(
                world,
                humidity,
                cfg.cond,
                cfg.oro,
                Some(temperature),
                Some(cfg.phase),
            );
        }
        if let (true, Some(t0), Some(t)) = (profile, t0, timings.as_mut()) {
            t.condensation += t0.elapsed();
        }
    }

    {
        let t0 = profile.then(Instant::now);
        if cfg.karst_on {
            apply_karst_dissolution(world, cfg.karst);
        }
        if let (true, Some(t0), Some(t)) = (profile, t0, timings.as_mut()) {
            t.karst += t0.elapsed();
        }
    }

    let geotech_due = geotech_map_due(tick_no);
    if geotech_due {
        if let Some(map) = geotech.as_mut() {
            map.rebuild_smart(world);
        }
    }

    {
        let t0 = profile.then(Instant::now);
        if let (Some(support_map), Some(landscape_store)) = (support.as_mut(), landscape.as_mut()) {
            let support_due = support_map_due(tick_no);
            let landscape_busy = !landscape_store.is_empty();
            if support_due || landscape_busy {
                support_map.rebuild(world);
            }
            let dirty = plan_active(world);
            if landscape_busy || !dirty.is_empty() || support_due {
                // Seed walk exact-skips !has_competent chunks once any
                // occupancy flag is set — rain-dirty ocean/sky is leftover.
                let coords: Vec<_> = if dirty.is_empty() && support_due {
                    world.chunks.keys().copied().collect()
                } else {
                    dirty.iter().map(|a| a.coord).collect()
                };
                let _ = apply_landscape_fall(world, landscape_store, support_map, &coords);
            }
        }
        if let (true, Some(t0), Some(t)) = (profile, t0, timings.as_mut()) {
            t.landscape += t0.elapsed();
        }
    }

    let rooted = if organisms_on {
        organisms
            .as_ref()
            .map(|store| collect_live_root_world_cells(&store.atoms))
    } else {
        None
    };

    set_parallel_enabled(cfg.perf.parallel_physics);
    let t_phys = profile.then(Instant::now);
    let geotech_ref = geotech.as_deref();
    let failure = if let Some(t) = timings.as_mut() {
        tick_with_life_profiled(
            world,
            cfg.perf,
            cfg.failure,
            geotech_ref,
            rooted.as_ref(),
            Some(cfg.grain),
            Some(cfg.fungi),
            Some(cfg.competent),
            &mut t.physics,
        )
    } else {
        tick_with_life(
            world,
            cfg.perf,
            cfg.failure,
            geotech_ref,
            rooted.as_ref(),
            Some(cfg.grain),
            Some(cfg.fungi),
            Some(cfg.competent),
        )
    };
    if let (true, Some(t0), Some(t)) = (profile, t_phys, timings.as_mut()) {
        t.physics_tick += t0.elapsed();
    }

    if organisms_on {
        if let Some(store) = organisms.as_mut() {
            let moves = std::mem::take(&mut world.competent_cell_moves);
            store.shift_atoms_with_moved_cells(world, &moves);
        }
    }

    set_parallel_enabled(true);

    {
        let t0 = profile.then(Instant::now);
        let _ = apply_snow_wind_drift(world, wind_vx, wind.tile_cols);
        if let (true, Some(t0), Some(t)) = (profile, t0, timings.as_mut()) {
            t.snow_drift += t0.elapsed();
        }
    }

    {
        let t0 = profile.then(Instant::now);
        step_carbon_budget(carbon, world, cfg.carbon);
        if let (true, Some(t0), Some(t)) = (profile, t0, timings.as_mut()) {
            t.carbon += t0.elapsed();
        }
    }

    {
        let t0 = profile.then(Instant::now);
        if organisms_on {
            if let Some(store) = organisms.as_mut() {
                sail_plants_on_wind_rafts_cfg(
                    world,
                    &mut store.atoms,
                    wind_vx,
                    wind.tile_cols,
                    cfg.grain,
                );
            }
        } else {
            let _ = drift_floating_organic_cfg(
                world,
                wind_vx,
                wind.tile_cols,
                None,
                None,
                cfg.grain,
            );
        }
        let _ = shove_floating_organic_with_current(world);
        if let (true, Some(t0), Some(t)) = (profile, t0, timings.as_mut()) {
            t.rafts += t0.elapsed();
        }
    }

    {
        let t0 = profile.then(Instant::now);
        apply_flow_erosion_bound(world, cfg.grain, rooted.as_ref());
        if let (true, Some(t0), Some(t)) = (profile, t0, timings.as_mut()) {
            t.erosion += t0.elapsed();
        }
    }

    {
        let t0 = profile.then(Instant::now);
        apply_suspension(world);
        if let (true, Some(t0), Some(t)) = (profile, t0, timings.as_mut()) {
            t.suspension += t0.elapsed();
        }
    }

    if geotech_due {
        if let Some(map) = geotech.as_mut() {
            map.rebuild_smart(world);
        }
    }

    // Atmospheric diffusion is periodic. Evap still deposits every
    // tick; only the spread step is throttled. Due-checks here use
    // `world.tick` **after** the CA increment — same as the play app.
    if humidity_diffuse_due(world.tick) {
        let t0 = profile.then(Instant::now);
        humidity.diffuse(cfg.humidity_diffusion_alpha);
        if let (true, Some(t0), Some(t)) = (profile, t0, timings.as_mut()) {
            t.humidity_diffuse += t0.elapsed();
            t.humidity_diffuse_calls += 1;
        }
    }
    if temperature_step_due(world.tick) {
        let t0 = profile.then(Instant::now);
        let t_now = world.tick;
        temperature.step(Some(world), humidity, t_now, Some(wind));
        // Hold just shrank. Dump the surplus now, not next step's lottery.
        precipitate_thermal_surplus(world, humidity, temperature, Some(cfg.phase));
        if let (true, Some(t0), Some(t)) = (profile, t0, timings.as_mut()) {
            t.temperature += t0.elapsed();
            t.temperature_calls += 1;
        }
    }

    if cfg.phase.enabled
        && cfg.phase.enable_cold_avalanche
        && world.tick % cfg.phase.period_ticks.max(1) == 0
    {
        let t0 = profile.then(Instant::now);
        let avalanche_roots = if organisms_on {
            organisms
                .as_ref()
                .map(|store| collect_live_root_world_cells(&store.atoms))
        } else {
            None
        };
        apply_cold_avalanche_bound(
            world,
            temperature,
            cfg.phase.freeze_point_c,
            avalanche_roots.as_ref(),
        );
        if let (true, Some(t0), Some(t)) = (profile, t0, timings.as_mut()) {
            t.cold_avalanche += t0.elapsed();
        }
    }

    {
        let t0 = profile.then(Instant::now);
        apply_phase(world, temperature, cfg.phase);
        if let (true, Some(t0), Some(t)) = (profile, t0, timings.as_mut()) {
            t.phase += t0.elapsed();
        }
    }

    let mut org_outcome = None;
    if organisms_on {
        if let Some(store) = organisms.as_mut() {
            let t0 = profile.then(Instant::now);
            let t_now = world.tick;
            org_outcome = Some(store.step_with_weather(
                world,
                t_now,
                cfg.climate,
                Some(humidity),
                wind_vx,
                Some(temperature),
                Some(carbon),
                cfg.carbon,
                Some(clouds),
                cfg.cloud.downpour_mass,
            ));
            if let (true, Some(t0), Some(t)) = (profile, t0, timings.as_mut()) {
                t.organisms += t0.elapsed();
            }
        }
    }

    WorldStepOutcome {
        failure,
        organisms: org_outcome,
        wind_vx,
        wind_vy,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::carbon::{CarbonBudget, CarbonConfig};
    use crate::climate::ClimateConfig;
    use crate::clouds::CloudStore;
    use crate::failure::FailureConfig;
    use crate::fungi::FungiConfig;
    use crate::humidity::Humidity;
    use crate::organism::OrganismStore;
    use crate::phase::PhaseConfig;
    use crate::rules::{
        CompetentFallConfig, CondensationConfig, EvapConfig, GrainConfig, KarstConfig,
        OrographicConfig, PerfConfig,
    };
    use crate::temperature::Temperature;
    use crate::wind::Wind;
    use crate::worldgen::{stamp_world, WorldgenParams};

    fn stamped_step_world() -> (World, Humidity, Wind, Temperature, CloudStore, CarbonBudget) {
        let params = WorldgenParams::default();
        let mut world = World::new(params.seed);
        stamp_world(&mut world, &params);
        let mut humidity = Humidity::with_world_bounds(
            4,
            0,
            params.bedrock_floor_y,
            params.width_cols,
            params.sky_ceiling_y,
        );
        humidity.wrap_x = params.wrap_x;
        let wind = Wind::climate(
            4,
            0.05,
            params.seed,
            params.width_cols,
            params.sea_level_y,
            params.bedrock_floor_y,
            params.sky_ceiling_y,
            params.wrap_x,
        );
        let temperature = Temperature::with_world_bounds(
            4,
            0,
            params.bedrock_floor_y,
            params.width_cols,
            params.sky_ceiling_y,
            params.seed,
            params.width_cols,
            params.sea_level_y,
            params.wrap_x,
        );
        (
            world,
            humidity,
            wind,
            temperature,
            CloudStore::new(),
            CarbonBudget::default(),
        )
    }

    fn demo_cfg<'a>(
        params: &'a WorldgenParams,
        perf: &'a PerfConfig,
        failure: &'a FailureConfig,
        evap: &'a EvapConfig,
        cond: &'a CondensationConfig,
        oro: &'a OrographicConfig,
        karst: &'a KarstConfig,
        cloud: &'a CloudConfig,
        phase: &'a PhaseConfig,
        climate: &'a ClimateConfig,
        carbon: &'a CarbonConfig,
        grain: &'a GrainConfig,
        fungi: &'a FungiConfig,
        competent: &'a CompetentFallConfig,
    ) -> WorldStepConfig<'a> {
        WorldStepConfig {
            perf,
            failure,
            evap,
            cond,
            oro: Some(oro),
            karst,
            cloud,
            phase,
            climate,
            carbon,
            grain,
            fungi,
            competent,
            humidity_diffusion_alpha: 0.15,
            sea_level_y: params.sea_level_y,
            sky_ceiling_y: params.sky_ceiling_y,
            evap_on: true,
            cond_rain_on: true,
            karst_on: true,
            organisms_on: false,
        }
    }

    #[test]
    fn step_world_increments_tick_once() {
        let params = WorldgenParams::default();
        let (mut world, mut humidity, mut wind, mut temperature, mut clouds, mut carbon) =
            stamped_step_world();
        let start = world.tick;
        let perf = PerfConfig::default();
        let failure = FailureConfig::default();
        let evap = EvapConfig {
            rate_per_tick: 1,
            dry_above_max: 200,
            period_ticks: 5,
        };
        let mut cond = CondensationConfig::default();
        cond.top_y = params.sky_ceiling_y - 2;
        let mut oro = OrographicConfig::default();
        oro.width_cols = params.width_cols;
        oro.sea_level_y = params.sea_level_y;
        let karst = KarstConfig::default();
        let cloud = CloudConfig::default();
        let phase = PhaseConfig::default();
        let climate = ClimateConfig::default();
        let carbon_cfg = CarbonConfig::default();
        let grain = GrainConfig::default();
        let fungi = FungiConfig::default();
        let competent = CompetentFallConfig::default();
        let cfg = demo_cfg(
            &params,
            &perf,
            &failure,
            &evap,
            &cond,
            &oro,
            &karst,
            &cloud,
            &phase,
            &climate,
            &carbon_cfg,
            &grain,
            &fungi,
            &competent,
        );
        let _ = step_world(
            WorldStep {
                world: &mut world,
                humidity: &mut humidity,
                wind: &mut wind,
                temperature: &mut temperature,
                clouds: &mut clouds,
                carbon: &mut carbon,
                organisms: None,
                landscape: None,
                geotech: None,
                support: None,
            },
            &cfg,
            None,
        );
        assert_eq!(world.tick, start.wrapping_add(1));
    }

    #[test]
    fn step_world_is_deterministic_for_a_fresh_stamp() {
        let params = WorldgenParams::default();
        let run = || {
            let (mut world, mut humidity, mut wind, mut temperature, mut clouds, mut carbon) =
                stamped_step_world();
            let perf = PerfConfig::default();
            let failure = FailureConfig::default();
            let evap = EvapConfig {
                rate_per_tick: 1,
                dry_above_max: 200,
                period_ticks: 5,
            };
            let mut cond = CondensationConfig::default();
            cond.top_y = params.sky_ceiling_y - 2;
            cond.min_mass_to_rain = 140.0;
            cond.max_prob_per_tick = 0.10;
            cond.mass_per_droplet = 255.0;
            let mut oro = OrographicConfig::default();
            oro.width_cols = params.width_cols;
            oro.sea_level_y = params.sea_level_y;
            let karst = KarstConfig::default();
            let cloud = CloudConfig::default();
            let phase = PhaseConfig::default();
            let climate = ClimateConfig::default();
            let carbon_cfg = CarbonConfig::default();
            let grain = GrainConfig::default();
            let fungi = FungiConfig::default();
            let competent = CompetentFallConfig::default();
            let cfg = demo_cfg(
                &params,
                &perf,
                &failure,
                &evap,
                &cond,
                &oro,
                &karst,
                &cloud,
                &phase,
                &climate,
                &carbon_cfg,
                &grain,
                &fungi,
                &competent,
            );
            for _ in 0..8 {
                let _ = step_world(
                    WorldStep {
                        world: &mut world,
                        humidity: &mut humidity,
                        wind: &mut wind,
                        temperature: &mut temperature,
                        clouds: &mut clouds,
                        carbon: &mut carbon,
                        organisms: None,
                        landscape: None,
                        geotech: None,
                        support: None,
                    },
                    &cfg,
                    None,
                );
            }
            (
                world.tick,
                humidity.total_mass(),
                carbon.atmosphere.to_bits(),
            )
        };
        let a = run();
        let b = run();
        assert_eq!(a.0, b.0);
        assert_eq!(a.2, b.2);
        // Shell rayon + HashMap key order can nudge vapour by a rounding
        // unit; the owner must not diverge on tick or carbon.
        assert!(
            (a.1 - b.1).abs() < 1.0,
            "humidity mass {0} vs {1}",
            a.1,
            b.1
        );
    }

    #[test]
    fn step_world_skips_organisms_when_flag_off() {
        let params = WorldgenParams::default();
        let (mut world, mut humidity, mut wind, mut temperature, mut clouds, mut carbon) =
            stamped_step_world();
        let mut organisms = OrganismStore::new();
        let perf = PerfConfig::default();
        let failure = FailureConfig::default();
        let evap = EvapConfig::default();
        let cond = CondensationConfig::default();
        let oro = OrographicConfig::default();
        let karst = KarstConfig::default();
        let cloud = CloudConfig::default();
        let phase = PhaseConfig::default();
        let climate = ClimateConfig::default();
        let carbon_cfg = CarbonConfig::default();
        let grain = GrainConfig::default();
        let fungi = FungiConfig::default();
        let competent = CompetentFallConfig::default();
        let mut cfg = demo_cfg(
            &params,
            &perf,
            &failure,
            &evap,
            &cond,
            &oro,
            &karst,
            &cloud,
            &phase,
            &climate,
            &carbon_cfg,
            &grain,
            &fungi,
            &competent,
        );
        cfg.organisms_on = false;
        let out = step_world(
            WorldStep {
                world: &mut world,
                humidity: &mut humidity,
                wind: &mut wind,
                temperature: &mut temperature,
                clouds: &mut clouds,
                carbon: &mut carbon,
                organisms: Some(&mut organisms),
                landscape: None,
                geotech: None,
                support: None,
            },
            &cfg,
            None,
        );
        assert!(out.organisms.is_none());
    }
}
