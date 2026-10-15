//! 5×7 bitmap font. Latin letters are small-caps; æ ø å and Æ Ø Å are
//! drawn. Bit 4 of each row is the leftmost pixel.
//!
//! Phase 6 replaces this with the full UI face. The shapes live in the
//! core so the Swift shell and the tests share one alphabet.

/// Seven rows, low 5 bits used, bit 4 = left.
pub fn glyph_rows(codepoint: u32) -> [u8; 7] {
    let cp = fold_ascii(codepoint);
    match cp {
        32 => [0, 0, 0, 0, 0, 0, 0],
        ch if ch == u32::from(b'A') => pat(
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ),
        ch if ch == u32::from(b'B') => pat(
            0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110,
        ),
        ch if ch == u32::from(b'C') => pat(
            0b01111, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b01111,
        ),
        ch if ch == u32::from(b'D') => pat(
            0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110,
        ),
        ch if ch == u32::from(b'E') => pat(
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ),
        ch if ch == u32::from(b'F') => pat(
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000,
        ),
        ch if ch == u32::from(b'G') => pat(
            0b01111, 0b10000, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111,
        ),
        ch if ch == u32::from(b'H') => pat(
            0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ),
        ch if ch == u32::from(b'I') => pat(
            0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ),
        ch if ch == u32::from(b'J') => pat(
            0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100,
        ),
        ch if ch == u32::from(b'K') => pat(
            0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001,
        ),
        ch if ch == u32::from(b'L') => pat(
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ),
        ch if ch == u32::from(b'M') => pat(
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ),
        ch if ch == u32::from(b'N') => pat(
            0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
        ),
        ch if ch == u32::from(b'O') => pat(
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ),
        ch if ch == u32::from(b'P') => pat(
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ),
        ch if ch == u32::from(b'Q') => pat(
            0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101,
        ),
        ch if ch == u32::from(b'R') => pat(
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ),
        ch if ch == u32::from(b'S') => pat(
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ),
        ch if ch == u32::from(b'T') => pat(
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ),
        ch if ch == u32::from(b'U') => pat(
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ),
        ch if ch == u32::from(b'V') => pat(
            0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100,
        ),
        ch if ch == u32::from(b'W') => pat(
            0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010,
        ),
        ch if ch == u32::from(b'X') => pat(
            0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001,
        ),
        ch if ch == u32::from(b'Y') => pat(
            0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100,
        ),
        ch if ch == u32::from(b'Z') => pat(
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111,
        ),
        ch if ch == u32::from(b'0') => pat(
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ),
        ch if ch == u32::from(b'1') => pat(
            0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ),
        ch if ch == u32::from(b'2') => pat(
            0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111,
        ),
        ch if ch == u32::from(b'3') => pat(
            0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110,
        ),
        ch if ch == u32::from(b'4') => pat(
            0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010,
        ),
        ch if ch == u32::from(b'5') => pat(
            0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110,
        ),
        ch if ch == u32::from(b'6') => pat(
            0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110,
        ),
        ch if ch == u32::from(b'7') => pat(
            0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000,
        ),
        ch if ch == u32::from(b'8') => pat(
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ),
        ch if ch == u32::from(b'9') => pat(
            0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100,
        ),
        ch if ch == u32::from(b'.') => pat(0, 0, 0, 0, 0, 0b01100, 0b01100),
        ch if ch == u32::from(b',') => pat(0, 0, 0, 0, 0b00100, 0b00100, 0b01000),
        ch if ch == u32::from(b'-') => pat(0, 0, 0, 0b11111, 0, 0, 0),
        ch if ch == u32::from(b'!') => pat(0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0, 0b00100),
        ch if ch == u32::from(b'?') => pat(0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0, 0b00100),
        // æ ø å — not folded into ASCII.
        0xE6 => pat(
            0b01101, 0b10011, 0b10100, 0b11010, 0b10100, 0b10011, 0b01101,
        ),
        0xF8 => pat(
            0b01110, 0b10001, 0b10101, 0b10101, 0b10101, 0b10001, 0b01110,
        ),
        0xE5 => pat(
            0b00100, 0b01010, 0b00100, 0b01110, 0b10001, 0b10001, 0b01110,
        ),
        0xC6 => pat(
            0b01111, 0b10000, 0b11110, 0b10001, 0b11111, 0b10000, 0b10000,
        ),
        0xD8 => pat(
            0b01110, 0b10011, 0b10101, 0b10101, 0b10101, 0b11001, 0b01110,
        ),
        0xC5 => pat(
            0b00100, 0b01010, 0b00100, 0b01110, 0b10001, 0b11111, 0b10001,
        ),
        _ => pat(
            0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110,
        ),
    }
}

/// Pack [`glyph_rows`] into 35 bits: row 0 in the lowest 5 bits.
pub fn glyph_bits(codepoint: u32) -> u64 {
    let rows = glyph_rows(codepoint);
    let mut bits = 0u64;
    for (i, row) in rows.iter().enumerate() {
        bits |= u64::from(*row & 0x1f) << (i * 5);
    }
    bits
}

fn fold_ascii(cp: u32) -> u32 {
    if (u32::from(b'a')..=u32::from(b'z')).contains(&cp) {
        cp - 32
    } else {
        cp
    }
}

fn pat(a: u8, b: u8, c: u8, d: u8, e: u8, f: u8, g: u8) -> [u8; 7] {
    [a, b, c, d, e, f, g]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nordic_letters_are_drawn() {
        for cp in ['æ', 'ø', 'å', 'Æ', 'Ø', 'Å', 'A', '0', 'a'] {
            let rows = glyph_rows(cp as u32);
            assert!(rows.iter().any(|&r| r != 0), "{cp} is blank");
            assert_ne!(glyph_bits(cp as u32), 0);
        }
    }

    #[test]
    fn lowercase_latin_matches_small_caps() {
        assert_eq!(glyph_rows('a' as u32), glyph_rows('A' as u32));
        assert_ne!(glyph_rows('æ' as u32), glyph_rows('Æ' as u32));
    }

    #[test]
    fn space_is_blank_and_bit4_is_the_left_pixel() {
        assert_eq!(glyph_bits(' ' as u32), 0);
        // 'T' top row is 0b11111, stored in the lowest 5 bits.
        assert_eq!(glyph_bits('T' as u32) & 0x1f, 0b11111);
    }
}
