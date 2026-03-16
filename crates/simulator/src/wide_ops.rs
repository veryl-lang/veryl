//! Helper functions for wide (>128-bit) arithmetic operations.
//! Called from JIT-compiled code via `call_indirect`.
//! All data is stored as little-endian u64 chunks.
//!
//! Safety: These functions are called from JIT-compiled code via function pointers.
//! The caller guarantees that pointers are valid and that `nb` is a multiple of 8
//! matching the buffer sizes.  Pointers may NOT be 8-byte aligned (variable layout
//! can place wide values at 4-byte-aligned offsets), so all accesses use
//! read_unaligned / write_unaligned.

#![allow(clippy::missing_safety_doc)]

/// Pack nb (byte count) and width (bit count) into a single u32.
#[inline]
pub fn pack_nb_width(nb: usize, width: usize) -> u32 {
    debug_assert!(nb < 65536 && width < 65536);
    (nb as u32) | ((width as u32) << 16)
}

/// Unpack nb and width from a packed u32.
#[inline]
fn unpack_nb_width(packed: u32) -> (u32, u32) {
    (packed & 0xFFFF, packed >> 16)
}

#[inline]
fn nw(nb: u32) -> usize {
    nb as usize / 8
}

#[inline]
unsafe fn rd(ptr: *const u8, i: usize) -> u64 {
    unsafe { (ptr.add(i * 8) as *const u64).read_unaligned() }
}

#[inline]
unsafe fn wr(ptr: *mut u8, i: usize, v: u64) {
    unsafe { (ptr.add(i * 8) as *mut u64).write_unaligned(v) }
}

// ── Bitwise binary ops ─────────────────────────────────────────────

pub unsafe extern "C" fn wide_band(dst: *mut u8, a: *const u8, b: *const u8, nb: u32) {
    unsafe {
        for i in 0..nw(nb) {
            wr(dst, i, rd(a, i) & rd(b, i));
        }
    }
}

pub unsafe extern "C" fn wide_bor(dst: *mut u8, a: *const u8, b: *const u8, nb: u32) {
    unsafe {
        for i in 0..nw(nb) {
            wr(dst, i, rd(a, i) | rd(b, i));
        }
    }
}

pub unsafe extern "C" fn wide_bxor(dst: *mut u8, a: *const u8, b: *const u8, nb: u32) {
    unsafe {
        for i in 0..nw(nb) {
            wr(dst, i, rd(a, i) ^ rd(b, i));
        }
    }
}

pub unsafe extern "C" fn wide_bxor_not(dst: *mut u8, a: *const u8, b: *const u8, nb: u32) {
    unsafe {
        for i in 0..nw(nb) {
            wr(dst, i, !(rd(a, i) ^ rd(b, i)));
        }
    }
}

pub unsafe extern "C" fn wide_band_not(dst: *mut u8, a: *const u8, b: *const u8, nb: u32) {
    unsafe {
        for i in 0..nw(nb) {
            wr(dst, i, rd(a, i) & !rd(b, i));
        }
    }
}

// ── Bitwise unary ops ───────────────────────────────────────────────

pub unsafe extern "C" fn wide_bnot(dst: *mut u8, a: *const u8, nb: u32) {
    unsafe {
        for i in 0..nw(nb) {
            wr(dst, i, !rd(a, i));
        }
    }
}

// ── Arithmetic ──────────────────────────────────────────────────────

pub unsafe extern "C" fn wide_add(dst: *mut u8, a: *const u8, b: *const u8, nb: u32) {
    unsafe {
        let mut carry = 0u64;
        for i in 0..nw(nb) {
            let (sum1, c1) = rd(a, i).overflowing_add(rd(b, i));
            let (sum2, c2) = sum1.overflowing_add(carry);
            wr(dst, i, sum2);
            carry = (c1 as u64) + (c2 as u64);
        }
    }
}

pub unsafe extern "C" fn wide_sub(dst: *mut u8, a: *const u8, b: *const u8, nb: u32) {
    unsafe {
        let mut borrow = 0u64;
        for i in 0..nw(nb) {
            let (diff1, b1) = rd(a, i).overflowing_sub(rd(b, i));
            let (diff2, b2) = diff1.overflowing_sub(borrow);
            wr(dst, i, diff2);
            borrow = (b1 as u64) + (b2 as u64);
        }
    }
}

pub unsafe extern "C" fn wide_mul(dst: *mut u8, a: *const u8, b: *const u8, nb: u32) {
    unsafe {
        let n = nw(nb);
        for i in 0..n {
            wr(dst, i, 0);
        }
        for i in 0..n {
            let ai = rd(a, i);
            if ai == 0 {
                continue;
            }
            let mut carry = 0u128;
            for j in 0..n {
                if i + j >= n {
                    break;
                }
                let prod = (ai as u128) * (rd(b, j) as u128) + (rd(dst, i + j) as u128) + carry;
                wr(dst, i + j, prod as u64);
                carry = prod >> 64;
            }
        }
    }
}

pub unsafe extern "C" fn wide_negate(dst: *mut u8, a: *const u8, nb: u32) {
    unsafe {
        let mut carry = 1u64;
        for i in 0..nw(nb) {
            let (sum, c) = (!rd(a, i)).overflowing_add(carry);
            wr(dst, i, sum);
            carry = c as u64;
        }
    }
}

// ── Memory ──────────────────────────────────────────────────────────

pub unsafe extern "C" fn wide_copy(dst: *mut u8, src: *const u8, nb: u32) {
    unsafe {
        for i in 0..nw(nb) {
            wr(dst, i, rd(src, i));
        }
    }
}

// ── Comparisons ─────────────────────────────────────────────────────

pub unsafe extern "C" fn wide_eq(a: *const u8, b: *const u8, nb: u32) -> i64 {
    unsafe {
        for i in 0..nw(nb) {
            if rd(a, i) != rd(b, i) {
                return 0;
            }
        }
        1
    }
}

pub unsafe extern "C" fn wide_ne(a: *const u8, b: *const u8, nb: u32) -> i64 {
    unsafe {
        for i in 0..nw(nb) {
            if rd(a, i) != rd(b, i) {
                return 1;
            }
        }
        0
    }
}

/// Unsigned compare: returns -1 if a < b, 0 if a == b, 1 if a > b.
pub unsafe extern "C" fn wide_ucmp(a: *const u8, b: *const u8, nb: u32) -> i64 {
    unsafe {
        for i in (0..nw(nb)).rev() {
            let ai = rd(a, i);
            let bi = rd(b, i);
            if ai < bi {
                return -1;
            }
            if ai > bi {
                return 1;
            }
        }
        0
    }
}

/// Signed compare using the sign bit at position (width-1).
pub unsafe extern "C" fn wide_scmp(a: *const u8, b: *const u8, packed_nb_width: u32) -> i64 {
    let (nb, width) = unpack_nb_width(packed_nb_width);
    if width == 0 || nb == 0 {
        return 0;
    }
    unsafe {
        let sign_word = (width as usize - 1) / 64;
        let sign_bit = (width as usize - 1) % 64;
        let a_sign = (rd(a, sign_word) >> sign_bit) & 1;
        let b_sign = (rd(b, sign_word) >> sign_bit) & 1;
        if a_sign != b_sign {
            return if a_sign == 1 { -1 } else { 1 };
        }
        wide_ucmp(a, b, nb)
    }
}

// ── Shifts ───────────────────────────────────────────────────────────

pub unsafe extern "C" fn wide_shl(dst: *mut u8, a: *const u8, amount: u64, nb: u32) {
    unsafe {
        let n = nw(nb);
        let word_shift = (amount / 64) as usize;
        let bit_shift = (amount % 64) as u32;

        if word_shift >= n {
            for i in 0..n {
                wr(dst, i, 0);
            }
            return;
        }

        for i in (0..n).rev() {
            let src_idx = i as isize - word_shift as isize;
            let lo = if src_idx >= 0 {
                rd(a, src_idx as usize)
            } else {
                0
            };
            let hi = if src_idx > 0 {
                rd(a, src_idx as usize - 1)
            } else {
                0
            };
            wr(
                dst,
                i,
                if bit_shift == 0 {
                    lo
                } else {
                    (lo << bit_shift) | (hi >> (64 - bit_shift))
                },
            );
        }
    }
}

pub unsafe extern "C" fn wide_lshr(dst: *mut u8, a: *const u8, amount: u64, nb: u32) {
    unsafe {
        let n = nw(nb);
        let word_shift = (amount / 64) as usize;
        let bit_shift = (amount % 64) as u32;

        if word_shift >= n {
            for i in 0..n {
                wr(dst, i, 0);
            }
            return;
        }

        for i in 0..n {
            let src_idx = i + word_shift;
            let lo = if src_idx < n { rd(a, src_idx) } else { 0 };
            let hi = if src_idx + 1 < n {
                rd(a, src_idx + 1)
            } else {
                0
            };
            wr(
                dst,
                i,
                if bit_shift == 0 {
                    lo
                } else {
                    (lo >> bit_shift) | (hi << (64 - bit_shift))
                },
            );
        }
    }
}

/// Arithmetic shift right: fills with sign bit at position (width-1).
pub unsafe extern "C" fn wide_ashr(dst: *mut u8, a: *const u8, amount: u64, packed_nb_width: u32) {
    let (nb, width) = unpack_nb_width(packed_nb_width);
    if nb == 0 || width == 0 {
        return;
    }
    unsafe {
        let n = nw(nb);
        let sign_word = (width as usize - 1) / 64;
        let sign_bit = (width as usize - 1) % 64;
        let sign = (rd(a, sign_word) >> sign_bit) & 1;

        wide_lshr(dst, a, amount, nb);

        if sign == 1 && amount > 0 {
            let fill_start = if amount >= width as u64 {
                0
            } else {
                (width as u64 - amount) as usize
            };
            for bit_pos in fill_start..width as usize {
                let word = bit_pos / 64;
                let bit = bit_pos % 64;
                if word < n {
                    wr(dst, word, rd(dst as *const u8, word) | (1u64 << bit));
                }
            }
        }
    }
}

// ── Reductions ───────────────────────────────────────────────────────

pub unsafe extern "C" fn wide_is_nonzero(a: *const u8, nb: u32) -> i64 {
    unsafe {
        for i in 0..nw(nb) {
            if rd(a, i) != 0 {
                return 1;
            }
        }
        0
    }
}

/// Check if all bits in [0..width) are set.
pub unsafe extern "C" fn wide_is_all_ones(a: *const u8, packed_nb_width: u32) -> i64 {
    let (_nb, width) = unpack_nb_width(packed_nb_width);
    if width == 0 {
        return 1;
    }
    unsafe {
        let full_words = width as usize / 64;
        let remaining = width as usize % 64;
        for i in 0..full_words {
            if rd(a, i) != u64::MAX {
                return 0;
            }
        }
        if remaining > 0 {
            let mask = (1u64 << remaining) - 1;
            if (rd(a, full_words) & mask) != mask {
                return 0;
            }
        }
        1
    }
}

/// Parity of popcount (reduction XOR): returns 0 or 1.
pub unsafe extern "C" fn wide_popcnt_parity(a: *const u8, nb: u32) -> i64 {
    unsafe {
        let mut total = 0u32;
        for i in 0..nw(nb) {
            total ^= rd(a, i).count_ones();
        }
        (total & 1) as i64
    }
}

/// Apply a width mask: clear bits >= width in dst.
pub unsafe extern "C" fn wide_apply_mask(dst: *mut u8, _unused: *const u8, packed_nb_width: u32) {
    let (nb, width) = unpack_nb_width(packed_nb_width);
    if width == 0 || nb == 0 {
        return;
    }
    unsafe {
        let n = nw(nb);
        let full_words = width as usize / 64;
        let remaining = width as usize % 64;
        if remaining > 0 && full_words < n {
            let mask = (1u64 << remaining) - 1;
            wr(dst, full_words, rd(dst as *const u8, full_words) & mask);
        }
        for i in (full_words + if remaining > 0 { 1 } else { 0 })..n {
            wr(dst, i, 0);
        }
    }
}

/// Fill dst with all-ones for width bits, zero above.
pub unsafe extern "C" fn wide_fill_ones(dst: *mut u8, _unused: *const u8, packed_nb_width: u32) {
    let (nb, width) = unpack_nb_width(packed_nb_width);
    if nb == 0 {
        return;
    }
    unsafe {
        let n = nw(nb);
        let full_words = width as usize / 64;
        let remaining = width as usize % 64;
        for i in 0..full_words.min(n) {
            wr(dst, i, u64::MAX);
        }
        if remaining > 0 && full_words < n {
            wr(dst, full_words, (1u64 << remaining) - 1);
        }
        for i in (full_words + if remaining > 0 { 1 } else { 0 })..n {
            wr(dst, i, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_buf(words: &[u64]) -> Vec<u8> {
        let mut buf = vec![0u8; words.len() * 8];
        for (i, &w) in words.iter().enumerate() {
            buf[i * 8..(i + 1) * 8].copy_from_slice(&w.to_le_bytes());
        }
        buf
    }

    fn read_words(buf: &[u8]) -> Vec<u64> {
        buf.chunks(8)
            .map(|c| u64::from_le_bytes(c.try_into().unwrap()))
            .collect()
    }

    #[test]
    fn test_wide_add() {
        let a = make_buf(&[u64::MAX, u64::MAX, 0, 0]);
        let b = make_buf(&[1, 0, 0, 0]);
        let mut dst = make_buf(&[0, 0, 0, 0]);
        unsafe { wide_add(dst.as_mut_ptr(), a.as_ptr(), b.as_ptr(), 32) };
        assert_eq!(read_words(&dst), vec![0, 0, 1, 0]);
    }

    #[test]
    fn test_wide_sub() {
        let a = make_buf(&[0, 0, 1, 0]);
        let b = make_buf(&[1, 0, 0, 0]);
        let mut dst = make_buf(&[0, 0, 0, 0]);
        unsafe { wide_sub(dst.as_mut_ptr(), a.as_ptr(), b.as_ptr(), 32) };
        assert_eq!(read_words(&dst), vec![u64::MAX, u64::MAX, 0, 0]);
    }

    #[test]
    fn test_wide_mul() {
        let a = make_buf(&[3, 0, 0, 0]);
        let b = make_buf(&[5, 0, 0, 0]);
        let mut dst = make_buf(&[0, 0, 0, 0]);
        unsafe { wide_mul(dst.as_mut_ptr(), a.as_ptr(), b.as_ptr(), 32) };
        assert_eq!(read_words(&dst), vec![15, 0, 0, 0]);
    }

    #[test]
    fn test_wide_shl() {
        let a = make_buf(&[1, 0, 0, 0]);
        let mut dst = make_buf(&[0, 0, 0, 0]);
        unsafe { wide_shl(dst.as_mut_ptr(), a.as_ptr(), 65, 32) };
        assert_eq!(read_words(&dst), vec![0, 2, 0, 0]);
    }

    #[test]
    fn test_wide_lshr() {
        let a = make_buf(&[0, 2, 0, 0]);
        let mut dst = make_buf(&[0, 0, 0, 0]);
        unsafe { wide_lshr(dst.as_mut_ptr(), a.as_ptr(), 65, 32) };
        assert_eq!(read_words(&dst), vec![1, 0, 0, 0]);
    }

    #[test]
    fn test_wide_ucmp() {
        let a = make_buf(&[0, 0, 1, 0]);
        let b = make_buf(&[u64::MAX, u64::MAX, 0, 0]);
        assert_eq!(unsafe { wide_ucmp(a.as_ptr(), b.as_ptr(), 32) }, 1);
        assert_eq!(unsafe { wide_ucmp(b.as_ptr(), a.as_ptr(), 32) }, -1);
        assert_eq!(unsafe { wide_ucmp(a.as_ptr(), a.as_ptr(), 32) }, 0);
    }

    #[test]
    fn test_wide_negate() {
        let a = make_buf(&[1, 0, 0, 0]);
        let mut dst = make_buf(&[0, 0, 0, 0]);
        unsafe { wide_negate(dst.as_mut_ptr(), a.as_ptr(), 32) };
        assert_eq!(
            read_words(&dst),
            vec![u64::MAX, u64::MAX, u64::MAX, u64::MAX]
        );
    }

    #[test]
    fn test_wide_popcnt_parity() {
        let a = make_buf(&[0b111, 0, 0, 0]);
        assert_eq!(unsafe { wide_popcnt_parity(a.as_ptr(), 32) }, 1);
        let b = make_buf(&[0b11, 0, 0, 0]);
        assert_eq!(unsafe { wide_popcnt_parity(b.as_ptr(), 32) }, 0);
    }

    #[test]
    fn test_unaligned_access() {
        let mut buf = vec![0u8; 36]; // 32 + 4 padding
        let ptr = unsafe { buf.as_mut_ptr().add(4) }; // 4-byte aligned, not 8
        unsafe { wide_add(ptr, ptr as *const u8, ptr as *const u8, 32) };
    }
}
