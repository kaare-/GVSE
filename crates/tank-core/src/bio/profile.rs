//! B1 protein profiles. A mode carries ≤6 block entries; units sum to
//! at most `P · w · h` (P = 8). Coefficients are derived on the fly for
//! B1; a lazy cache lands when the genotype table gets hot.

use crate::consts::P_UNITS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Block {
    Photo = 1,
    Store = 2,
    Wall = 3,
    Repair = 4,
    UptakeGlucose = 5,
    Excrete = 6,
    Buoy = 7,
    Pigment = 8,
    Transparent = 9,
}

impl Block {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::Photo),
            2 => Some(Self::Store),
            3 => Some(Self::Wall),
            4 => Some(Self::Repair),
            5 => Some(Self::UptakeGlucose),
            6 => Some(Self::Excrete),
            7 => Some(Self::Buoy),
            8 => Some(Self::Pigment),
            9 => Some(Self::Transparent),
            _ => None,
        }
    }

    pub fn upkeep_fp(self) -> i32 {
        // Fixed-point 256 = 1 g/s. Seeds from cell-language §a.4.
        match self {
            Self::Photo => 5, // 0.02
            Self::Store => 1, // ~0.005
            Self::Wall => 2,
            Self::Repair => 3,
            Self::UptakeGlucose => 2,
            Self::Excrete => 2,
            Self::Buoy => 2,
            Self::Pigment => 1,
            Self::Transparent => 1,
        }
    }

    pub fn colour(self) -> [u8; 3] {
        match self {
            Self::Photo => [0x3F, 0xA3, 0x4D],
            Self::Store => [0xE8, 0xD5, 0xA3],
            Self::Wall => [0x8B, 0x6F, 0x47],
            Self::Repair => [0xF5, 0xDD, 0xE0],
            Self::UptakeGlucose => [0xC4, 0xA3, 0x6A],
            Self::Excrete => [0x6B, 0x5B, 0x4B],
            Self::Buoy => [0xF0, 0xF0, 0xF0],
            Self::Pigment => [0xE0, 0x80, 0xC0],
            Self::Transparent => [0xC0, 0xD8, 0xE0],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Entry {
    pub block: Block,
    pub units: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Mode {
    pub shape_w: u8,
    pub shape_h: u8,
    pub entries: [Entry; 6],
    pub n_entries: u8,
}

impl Mode {
    pub fn algae() -> Self {
        let mut m = Self {
            shape_w: 1,
            shape_h: 1,
            entries: [Entry {
                block: Block::Photo,
                units: 0,
            }; 6],
            n_entries: 1,
        };
        m.entries[0] = Entry {
            block: Block::Photo,
            units: 5,
        };
        m
    }

    pub fn capacity(&self) -> u16 {
        P_UNITS as u16 * self.shape_w as u16 * self.shape_h as u16
    }

    pub fn used_units(&self) -> u16 {
        (0..self.n_entries as usize)
            .map(|i| self.entries[i].units as u16)
            .sum()
    }

    pub fn units_of(&self, block: Block) -> u8 {
        (0..self.n_entries as usize)
            .filter(|&i| self.entries[i].block == block)
            .map(|i| self.entries[i].units)
            .sum()
    }

    pub fn upkeep_fp(&self) -> i32 {
        let mut u = 0i32;
        for i in 0..self.n_entries as usize {
            let e = self.entries[i];
            u += e.block.upkeep_fp() * e.units as i32;
            u += 2; // ~0.01 g/s structure per block entry (fp 256)
        }
        u + 3 // baseline structure
    }

    /// Units-weighted colour blend; unused capacity → pale amber.
    pub fn colour(&self, metab_eff: u8) -> [u8; 3] {
        let cap = self.capacity().max(1);
        let used = self.used_units().min(cap);
        let unused = cap - used;
        let mut r = 0u32;
        let mut g = 0u32;
        let mut b = 0u32;
        let mut wsum = 0u32;
        for i in 0..self.n_entries as usize {
            let e = self.entries[i];
            let mut w = e.units as u32;
            if e.block == Block::Pigment {
                w = ((e.units as u32) * 10).min(60) * cap as u32 / 100;
            }
            let c = e.block.colour();
            r += c[0] as u32 * w;
            g += c[1] as u32 * w;
            b += c[2] as u32 * w;
            wsum += w;
        }
        if unused > 0 {
            // Pale amber unused capacity.
            let w = unused as u32;
            r += 0xE6 * w;
            g += 0xC8 * w;
            b += 0x8A * w;
            wsum += w;
        }
        if wsum == 0 {
            return [0x3F, 0xA3, 0x4D];
        }
        let bright = metab_eff as u32;
        [
            ((r / wsum) * bright / 128).min(255) as u8,
            ((g / wsum) * bright / 128).min(255) as u8,
            ((b / wsum) * bright / 128).min(255) as u8,
        ]
    }

    pub fn mutate_units(&mut self, seed: u64, salt: u64) {
        use crate::hash::hash_u64;
        if self.n_entries == 0 {
            return;
        }
        let i = (hash_u64(seed, salt, 1, 0xB1_01) as usize) % self.n_entries as usize;
        let d = (hash_u64(seed, salt, 2, 0xB1_02) % 5) as i32 - 2;
        let u = (self.entries[i].units as i32 + d).clamp(0, 8) as u8;
        self.entries[i].units = u;
        // Drop zero entries.
        if u == 0 && self.n_entries > 1 {
            for j in i..self.n_entries as usize - 1 {
                self.entries[j] = self.entries[j + 1];
            }
            self.n_entries -= 1;
        }
        // Cap to capacity.
        while self.used_units() > self.capacity() && self.n_entries > 0 {
            let last = self.n_entries as usize - 1;
            if self.entries[last].units > 0 {
                self.entries[last].units -= 1;
            } else {
                self.n_entries -= 1;
            }
        }
    }
}
