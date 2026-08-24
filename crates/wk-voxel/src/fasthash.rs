//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app.
//!
//! Small non-cryptographic hasher for hot `(i32, i32)` cell-coordinate sets.
//!
//! The standard library defaults to SipHash-1-3, which is DoS-resistant but
//! costs far more than the lookup it protects. Body building floods, weld
//! splitting, and cargo gathering hash millions of coordinate pairs per second
//! and never see untrusted input, so they use this instead.
//!
//! Same multiply-xor-rotate construction as rustc's `FxHasher`.

use std::hash::{BuildHasherDefault, Hasher};

/// Odd 64-bit constant close to 2^64 / phi.
const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;

#[derive(Default, Clone, Copy)]
pub struct FxHasher {
  hash: u64,
}

impl FxHasher {
  #[inline]
  fn add(&mut self, word: u64) {
    self.hash = (self.hash.rotate_left(5) ^ word).wrapping_mul(SEED);
  }
}

impl Hasher for FxHasher {
  #[inline]
  fn write(&mut self, bytes: &[u8]) {
    for chunk in bytes.chunks(8) {
      let mut buf = [0u8; 8];
      buf[..chunk.len()].copy_from_slice(chunk);
      self.add(u64::from_le_bytes(buf));
    }
  }

  #[inline]
  fn write_u8(&mut self, n: u8) {
    self.add(n as u64);
  }

  #[inline]
  fn write_u32(&mut self, n: u32) {
    self.add(n as u64);
  }

  #[inline]
  fn write_u64(&mut self, n: u64) {
    self.add(n);
  }

  #[inline]
  fn write_i32(&mut self, n: i32) {
    self.add(n as u32 as u64);
  }

  #[inline]
  fn write_usize(&mut self, n: usize) {
    self.add(n as u64);
  }

  #[inline]
  fn finish(&self) -> u64 {
    self.hash
  }
}

pub type FxBuildHasher = BuildHasherDefault<FxHasher>;
pub type FxHashSet<T> = std::collections::HashSet<T, FxBuildHasher>;
pub type FxHashMap<K, V> = std::collections::HashMap<K, V, FxBuildHasher>;

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn set_and_map_behave_like_std() {
    let mut s: FxHashSet<(i32, i32)> = FxHashSet::default();
    assert!(s.insert((1, 2)));
    assert!(!s.insert((1, 2)));
    assert!(s.contains(&(1, 2)));
    assert!(!s.contains(&(2, 1)));
    s.remove(&(1, 2));
    assert!(s.is_empty());

    let mut m: FxHashMap<(i32, i32), u32> = FxHashMap::default();
    m.insert((-5, 9), 7);
    assert_eq!(m.get(&(-5, 9)), Some(&7));
    assert_eq!(m.get(&(9, -5)), None);
  }

  #[test]
  fn negative_and_large_coords_are_distinct() {
    let mut s: FxHashSet<(i32, i32)> = FxHashSet::default();
    for x in [-1000, -1, 0, 1, 1000, i32::MIN, i32::MAX] {
      for y in [-1000, -1, 0, 1, 1000, i32::MIN, i32::MAX] {
        assert!(s.insert((x, y)), "duplicate for ({x},{y})");
      }
    }
    assert_eq!(s.len(), 49);
  }
}
