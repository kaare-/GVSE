use wk_material::FIXED_SCALE;

#[derive(Debug, Clone, Copy, Default)]
pub struct ResidualAccumulator {
    pub erosion: i64,
    pub infiltration: i64,
    pub evaporation: i64,
}

impl ResidualAccumulator {
    pub fn drain(field: &mut i64, rate_per_tick: f32) -> i64 {
        let add = (rate_per_tick * FIXED_SCALE as f32).round() as i64;
        *field += add;
        let transfer = *field / FIXED_SCALE;
        *field %= FIXED_SCALE;
        transfer
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn residual_produces_integer_transfer_at_1000_ticks() {
        let mut field = 0i64;
        let mut total = 0i64;
        for _ in 0..1000 {
            total += ResidualAccumulator::drain(&mut field, 1.0);
        }
        assert_eq!(total, 1000);
        assert_eq!(field, 0);
    }
}
