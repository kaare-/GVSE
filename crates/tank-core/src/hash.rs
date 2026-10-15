//! Deterministic mixer. Same construction as GvsE `hash_u64`
//! (`wk-voxel` blueprint / organism): every random choice in the tank
//! is a pure function of `(world_seed, a, b, salt)`.

#[inline]
pub fn hash_u64(seed: u64, a: u64, b: u64, salt: u64) -> u64 {
    let mut x = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(a)
        .wrapping_add(b)
        .wrapping_add(salt);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    x
}

/// FNV-1a 64. Used for the pixel-hash determinism contract.
pub fn hash_bytes(data: &[u8]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixer_is_stable_and_seed_sensitive() {
        let a = hash_u64(1, 2, 3, 4);
        assert_eq!(a, hash_u64(1, 2, 3, 4));
        assert_ne!(a, hash_u64(2, 2, 3, 4));
        assert_ne!(a, 0);
    }
}
