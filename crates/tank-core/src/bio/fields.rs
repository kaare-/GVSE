//! B3 keyed chemistry fields: enzyme / toxin / signal.
//! Contact is primary; secreted fields decay and blur (K3#3).

use crate::pixels::PixelLayer;

/// Enzyme/toxin decay ×0.75/tick; signal ×0.92 (handoff).
pub fn decay_fields(pixels: &mut PixelLayer) {
    for a in &mut pixels.enzyme_amt {
        *a = ((*a as u16 * 192) / 256) as u8;
    }
    for a in &mut pixels.toxin_amt {
        *a = ((*a as u16 * 192) / 256) as u8;
    }
    for a in &mut pixels.signal_amt {
        *a = ((*a as u16 * 235) / 256) as u8;
    }
}

/// Cheap 4-neighbour blur of amount; dominant key wins on mix.
pub fn blur_keyed(amt: &mut [u8], key: &mut [u8], w: u32, h: u32) {
    let pw = w as usize;
    let ph = h as usize;
    let mut next_a = amt.to_vec();
    let mut next_k = key.to_vec();
    for y in 0..ph {
        for x in 0..pw {
            let i = y * pw + x;
            let mut sum = amt[i] as u32 * 4;
            let mut best_a = amt[i];
            let mut best_k = key[i];
            for (dx, dy) in [(-1i32, 0), (1, 0), (0, -1), (0, 1)] {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 {
                    continue;
                }
                let j = (ny as usize) * pw + nx as usize;
                sum += amt[j] as u32;
                if amt[j] > best_a {
                    best_a = amt[j];
                    best_k = key[j];
                }
            }
            next_a[i] = (sum / 8).min(255) as u8;
            if next_a[i] > 0 {
                next_k[i] = best_k;
            }
        }
    }
    amt.copy_from_slice(&next_a);
    key.copy_from_slice(&next_k);
}

pub fn step_fields(pixels: &mut PixelLayer, tick: u64) {
    if !pixels.fields_active {
        return;
    }
    decay_fields(pixels);
    // Blur every 2nd tick (reach 2–3 px).
    if tick % 2 == 0 {
        blur_keyed(
            &mut pixels.enzyme_amt,
            &mut pixels.enzyme_key,
            pixels.w,
            pixels.h,
        );
        blur_keyed(
            &mut pixels.toxin_amt,
            &mut pixels.toxin_key,
            pixels.w,
            pixels.h,
        );
        blur_keyed(
            &mut pixels.signal_amt,
            &mut pixels.signal_key,
            pixels.w,
            pixels.h,
        );
    }
    // Clear the active flag when everything has decayed away.
    let any = pixels.enzyme_amt.iter().any(|&a| a > 0)
        || pixels.toxin_amt.iter().any(|&a| a > 0)
        || pixels.signal_amt.iter().any(|&a| a > 0);
    pixels.fields_active = any;
}

/// Deposit a keyed amount at a pixel (dominant-key keep-if-higher).
pub fn deposit(pixels: &mut PixelLayer, which: KeyedField, idx: usize, add: u8, k: u8) {
    if add == 0 {
        return;
    }
    let (amt, key) = match which {
        KeyedField::Enzyme => (&mut pixels.enzyme_amt, &mut pixels.enzyme_key),
        KeyedField::Toxin => (&mut pixels.toxin_amt, &mut pixels.toxin_key),
        KeyedField::Signal => (&mut pixels.signal_amt, &mut pixels.signal_key),
    };
    if idx >= amt.len() {
        return;
    }
    let next = amt[idx].saturating_add(add);
    if add >= amt[idx] / 2 {
        key[idx] = k;
    }
    amt[idx] = next;
    pixels.fields_active = true;
}

#[derive(Clone, Copy)]
pub enum KeyedField {
    Enzyme,
    Toxin,
    Signal,
}
