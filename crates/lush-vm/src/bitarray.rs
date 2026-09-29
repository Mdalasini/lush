//! Bit-array construction, matching helpers, and inspect (spec §5.9).

use lush_ir::bytecode::BitSegEnc;

use crate::heap::{Heap, ObjectKind};
use crate::value::Value;

/// Mutable bit buffer used while building a bit array.
#[derive(Clone, Debug, Default)]
pub struct BitBuf {
    pub bytes: Vec<u8>,
    pub bit_len: u64,
}

impl BitBuf {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_parts(bytes: Vec<u8>, bit_len: u64) -> Self {
        let mut b = Self { bytes, bit_len };
        b.zero_padding();
        b
    }

    fn zero_padding(&mut self) {
        if self.bit_len.is_multiple_of(8) {
            return;
        }
        let used = (self.bit_len % 8) as u8;
        if let Some(last) = self.bytes.last_mut() {
            let mask = !((1u8 << (8 - used)) - 1);
            *last &= mask;
        }
    }

    fn ensure_byte(&mut self) {
        let need = self.bit_len.div_ceil(8) as usize;
        if self.bytes.len() < need {
            self.bytes.resize(need, 0);
        }
    }

    /// Append `n` bits of `value` (low `n` bits), MSB-first into the stream.
    pub fn push_bits(&mut self, value: u64, n: u8) {
        debug_assert!(n <= 64);
        for i in (0..n).rev() {
            let bit = ((value >> i) & 1) as u8;
            let pos = self.bit_len;
            self.bit_len += 1;
            self.ensure_byte();
            let byte_i = (pos / 8) as usize;
            let bit_i = (pos % 8) as u8; // 0 = MSB
            if bit != 0 {
                self.bytes[byte_i] |= 1 << (7 - bit_i);
            }
        }
        self.zero_padding();
    }

    pub fn push_bytes(&mut self, data: &[u8]) {
        for &b in data {
            self.push_bits(b as u64, 8);
        }
    }

    pub fn append_buf(&mut self, other: &BitBuf) {
        // Copy bit-by-bit for exact length (handles partial final byte).
        let mut remaining = other.bit_len;
        let mut byte_i = 0usize;
        while remaining > 0 {
            let take = remaining.min(8) as u8;
            let b = other.bytes.get(byte_i).copied().unwrap_or(0);
            // Top `take` bits of this byte.
            let shift = 8 - take;
            let val = (b >> shift) as u64;
            self.push_bits(val, take);
            remaining -= take as u64;
            byte_i += 1;
        }
    }
}

pub fn int_fits(value: i64, size: u8, signed: bool) -> bool {
    if size == 0 || size > 64 {
        return false;
    }
    if signed {
        if size == 64 {
            return true;
        }
        let min = -(1i64 << (size - 1));
        let max = (1i64 << (size - 1)) - 1;
        value >= min && value <= max
    } else if size == 64 {
        value >= 0
    } else {
        value >= 0 && (value as u64) < (1u64 << size)
    }
}

fn encode_int_bits(value: i64, size: u8, signed: bool, little: bool) -> Result<u64, String> {
    if !int_fits(value, size, signed) {
        return Err(format!(
            "bit array integer segment out of range for size({size}){}",
            if signed { "-signed" } else { "" }
        ));
    }
    let mut bits = if signed {
        (value as u64) & mask(size)
    } else {
        value as u64
    };
    if little {
        bits = swap_endian(bits, size);
    }
    Ok(bits)
}

fn mask(size: u8) -> u64 {
    if size >= 64 {
        u64::MAX
    } else {
        (1u64 << size) - 1
    }
}

/// Byte-swap within an `size`-bit value (size multiple of 8).
fn swap_endian(bits: u64, size: u8) -> u64 {
    debug_assert!(size.is_multiple_of(8));
    let nbytes = (size / 8) as usize;
    let mut bytes = [0u8; 8];
    for (i, slot) in bytes.iter_mut().enumerate().take(nbytes) {
        *slot = ((bits >> (8 * (nbytes - 1 - i))) & 0xff) as u8;
    }
    // little: reverse byte order for the payload
    bytes[..nbytes].reverse();
    let mut out = 0u64;
    for &b in bytes.iter().take(nbytes) {
        out = (out << 8) | b as u64;
    }
    out
}

pub fn append_segment(
    buf: &mut BitBuf,
    heap: &Heap,
    value: Value,
    spec: &BitSegEnc,
) -> Result<(), String> {
    match spec {
        BitSegEnc::Int {
            size,
            signed,
            little,
        } => {
            let n = value
                .as_int(heap)
                .ok_or_else(|| "bit array integer segment expects Int".to_string())?;
            let bits = encode_int_bits(n, *size, *signed, *little)?;
            buf.push_bits(bits, *size);
            Ok(())
        }
        BitSegEnc::Utf8 => {
            let p = value
                .as_ptr()
                .ok_or_else(|| "bit array utf8 segment expects String".to_string())?;
            if heap.kind(p) != ObjectKind::String {
                return Err("bit array utf8 segment expects String".into());
            }
            buf.push_bytes(heap.string_bytes(p));
            Ok(())
        }
        BitSegEnc::Bits { size_bits } => append_bits_value(buf, heap, value, *size_bits, false),
        BitSegEnc::Bytes { size_bytes } => {
            let size_bits = size_bytes.map(|b| b.saturating_mul(8));
            append_bits_value(buf, heap, value, size_bits, true)
        }
    }
}

fn append_bits_value(
    buf: &mut BitBuf,
    heap: &Heap,
    value: Value,
    size_bits: Option<u32>,
    require_aligned: bool,
) -> Result<(), String> {
    let p = value
        .as_ptr()
        .ok_or_else(|| "bit array bytes/bits segment expects BitArray".to_string())?;
    if heap.kind(p) != ObjectKind::BitArray {
        return Err("bit array bytes/bits segment expects BitArray".into());
    }
    let src_len = heap.bit_array_len(p);
    let src_off = heap.bit_array_off(p);
    if require_aligned && (!src_off.is_multiple_of(8) || !src_len.is_multiple_of(8)) {
        return Err("bit array bytes segment source is not byte-aligned".into());
    }
    let take = match size_bits {
        Some(n) => {
            let n = n as u64;
            if src_len != n {
                return Err(format!(
                    "bit array segment size mismatch: expected {n} bits, got {src_len}"
                ));
            }
            n
        }
        None => src_len,
    };
    // Copy only the logical view (respecting bit_off), not the whole buffer.
    let mut cur = BitCursor::new(heap.bit_array_bits(p), src_off, take);
    let mut src = BitBuf::new();
    let mut left = take;
    while left > 0 {
        let n = left.min(8) as u8;
        let v = cur
            .read_bits_pub(n)
            .ok_or_else(|| "bit array segment truncated".to_string())?;
        src.push_bits(v, n);
        left -= n as u64;
    }
    buf.append_buf(&src);
    Ok(())
}

/// View into a bit array at an absolute bit offset (for matching).
#[derive(Clone, Copy, Debug)]
pub struct BitCursor<'a> {
    bytes: &'a [u8],
    /// Exclusive end bit position in `bytes`.
    end: u64,
    /// Current absolute bit position in `bytes`.
    offset: u64,
}

impl<'a> BitCursor<'a> {
    pub fn new(bytes: &'a [u8], bit_off: u64, bit_len: u64) -> Self {
        Self {
            bytes,
            end: bit_off.saturating_add(bit_len),
            offset: bit_off,
        }
    }

    pub fn remaining(&self) -> u64 {
        self.end.saturating_sub(self.offset)
    }

    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }

    /// Absolute bit offset of the current read position (for alignment checks).
    pub fn absolute_offset(&self) -> u64 {
        self.offset
    }

    /// `(bit_off, bit_len)` describing the unread suffix as a view.
    pub fn rest_view(&self) -> (u64, u64) {
        (self.offset, self.remaining())
    }

    pub fn read_bits_pub(&mut self, n: u8) -> Option<u64> {
        self.read_bits(n)
    }

    fn read_bits(&mut self, n: u8) -> Option<u64> {
        if n as u64 > self.remaining() {
            return None;
        }
        let mut out = 0u64;
        for _ in 0..n {
            let pos = self.offset;
            self.offset += 1;
            let byte_i = (pos / 8) as usize;
            let bit_i = (pos % 8) as u8;
            let bit = (self.bytes.get(byte_i).copied().unwrap_or(0) >> (7 - bit_i)) & 1;
            out = (out << 1) | bit as u64;
        }
        Some(out)
    }

    pub fn take_int(&mut self, size: u8, signed: bool, little: bool) -> Option<i64> {
        let mut bits = self.read_bits(size)?;
        if little {
            bits = swap_endian(bits, size);
        }
        if signed {
            // two's complement
            if size < 64 {
                let sign_bit = 1u64 << (size - 1);
                if bits & sign_bit != 0 {
                    let ext = (!mask(size)) | bits;
                    return Some(ext as i64);
                }
            }
            Some(bits as i64)
        } else if size == 64 {
            // Unsigned 64-bit above MAX_INT fails the match.
            if bits > i64::MAX as u64 {
                return None;
            }
            Some(bits as i64)
        } else {
            Some(bits as i64)
        }
    }

    pub fn take_bytes_prefix(&mut self, want: &[u8]) -> bool {
        if (want.len() as u64) * 8 > self.remaining() {
            return false;
        }
        for &b in want {
            match self.read_bits(8) {
                Some(v) if v as u8 == b => {}
                _ => return false,
            }
        }
        true
    }

    /// Alignment check for `:bytes` remainder without copying.
    pub fn rest_aligned(&self, require_byte_aligned: bool) -> bool {
        !require_byte_aligned || self.offset.is_multiple_of(8)
    }
}

pub fn cursor_from_value<'a>(heap: &'a Heap, v: Value) -> Option<BitCursor<'a>> {
    let p = v.as_ptr()?;
    if heap.kind(p) != ObjectKind::BitArray {
        return None;
    }
    Some(BitCursor::new(
        heap.bit_array_bits(p),
        heap.bit_array_off(p),
        heap.bit_array_len(p),
    ))
}

pub fn eq_bit_arrays(heap: &Heap, a: Value, b: Value, charge: &mut u64) -> bool {
    let (Some(mut ca), Some(mut cb)) = (cursor_from_value(heap, a), cursor_from_value(heap, b))
    else {
        return false;
    };
    let la = ca.remaining();
    let lb = cb.remaining();
    if la != lb {
        return false;
    }
    let nbytes = la.div_ceil(8);
    *charge += nbytes.div_ceil(8);
    let mut left = la;
    while left > 0 {
        let n = left.min(8) as u8;
        let Some(x) = ca.read_bits(n) else {
            return false;
        };
        let Some(y) = cb.read_bits(n) else {
            return false;
        };
        if x != y {
            return false;
        }
        left -= n as u64;
    }
    true
}

/// Render `<<1, 2, 3>>` with `:size(n)` on a trailing partial byte.
pub fn inspect_bit_array_view(heap: &Heap, p: crate::heap::HeapPtr) -> String {
    let mut cur = BitCursor::new(
        heap.bit_array_bits(p),
        heap.bit_array_off(p),
        heap.bit_array_len(p),
    );
    let bit_len = cur.remaining();
    let mut out = String::from("<<");
    if bit_len == 0 {
        out.push_str(">>");
        return out;
    }
    let full = bit_len / 8;
    let rem = bit_len % 8;
    let mut first = true;
    for _ in 0..full {
        if !first {
            out.push_str(", ");
        }
        first = false;
        let b = cur.read_bits(8).unwrap_or(0) as u8;
        out.push_str(&b.to_string());
    }
    if rem != 0 {
        if !first {
            out.push_str(", ");
        }
        let val = cur.read_bits(rem as u8).unwrap_or(0);
        out.push_str(&format!("{val}:size({rem})"));
    }
    out.push_str(">>");
    out
}

/// Convenience for tests that hold a raw buffer starting at bit 0.
pub fn inspect_bit_array(bytes: &[u8], bit_len: u64) -> String {
    let mut heap = Heap::new();
    let p = heap.alloc_bit_array(bytes.to_vec(), bit_len);
    inspect_bit_array_view(&heap, p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padding_bits_ignored_by_eq() {
        let mut heap = Heap::new();
        // Same 3 bits, different garbage in low padding.
        let a = heap.alloc_bit_array(vec![0b1010_0000], 3);
        let b = heap.alloc_bit_array(vec![0b1010_0111], 3);
        let mut charge = 0u64;
        assert!(eq_bit_arrays(
            &heap,
            Value::from_ptr(a),
            Value::from_ptr(b),
            &mut charge
        ));
    }

    #[test]
    fn different_bit_len_not_eq() {
        let mut heap = Heap::new();
        let a = heap.alloc_bit_array(vec![0b1000_0000], 3);
        let b = heap.alloc_bit_array(vec![0b1000_0000], 4);
        let mut charge = 0u64;
        assert!(!eq_bit_arrays(
            &heap,
            Value::from_ptr(a),
            Value::from_ptr(b),
            &mut charge
        ));
    }

    #[test]
    fn inspect_full_and_partial() {
        assert_eq!(inspect_bit_array(&[1, 2, 3], 24), "<<1, 2, 3>>");
        assert_eq!(inspect_bit_array(&[0b1010_0000], 3), "<<5:size(3)>>");
    }

    #[test]
    fn encode_unsigned_8_range() {
        assert!(int_fits(255, 8, false));
        assert!(!int_fits(256, 8, false));
        assert!(!int_fits(-1, 8, false));
        assert!(int_fits(-1, 8, true));
        assert!(!int_fits(128, 8, true));
    }
}
