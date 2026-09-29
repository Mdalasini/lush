//! Tagged 64-bit values (§7.1).
//!
//! Layout (low bits):
//! - `…xxx000` — heap pointer (`HeapPtr` indices are shifted by 3)
//! - `…xxxxxx1` — immediate Int (1-bit tag; signed payload in bits 1..63 ⇒ range
//!   `[-2^62, 2^62)`)
//! - `0b010` / `0b110` / `0b100` — `False` / `True` / `Nil` (even, non-pointer)

use crate::heap::{Heap, HeapPtr, ObjectKind};

const TAG_MASK: u64 = 0b111;
const TAG_BOOL_FALSE: u64 = 0b010;
const TAG_BOOL_TRUE: u64 = 0b110;
const TAG_NIL: u64 = 0b100;
const TAG_PTR: u64 = 0b000;

/// Inclusive lower bound of the immediate Int range (`-2^62`).
pub const IMM_MIN: i64 = -(1 << 62);
/// Inclusive upper bound of the immediate Int range (`2^62 - 1`).
pub const IMM_MAX: i64 = (1 << 62) - 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Value(pub u64);

impl Value {
    pub fn nil() -> Self {
        Value(TAG_NIL)
    }

    pub fn bool(v: bool) -> Self {
        Value(if v { TAG_BOOL_TRUE } else { TAG_BOOL_FALSE })
    }

    pub fn from_bool(v: bool) -> Self {
        Self::bool(v)
    }

    pub fn is_nil(self) -> bool {
        self.0 == TAG_NIL
    }

    pub fn as_bool(self) -> Option<bool> {
        match self.0 {
            TAG_BOOL_TRUE => Some(true),
            TAG_BOOL_FALSE => Some(false),
            _ => None,
        }
    }

    pub fn is_falsey(self) -> bool {
        matches!(self.as_bool(), Some(false)) || self.is_nil()
    }

    pub fn is_ptr(self) -> bool {
        // HeapPtr encodes index in the high bits; index 0 is a valid object.
        self.0 & TAG_MASK == TAG_PTR
    }

    pub fn from_ptr(p: HeapPtr) -> Self {
        Value(p.0)
    }

    pub fn as_ptr(self) -> Option<HeapPtr> {
        if self.is_ptr() {
            Some(HeapPtr(self.0))
        } else {
            None
        }
    }

    /// True when this word is an immediate Int (low bit set). Bool/Nil are even.
    pub fn is_imm_int(self) -> bool {
        self.0 & 1 == 1
    }

    pub fn int(heap: &mut Heap, v: i64) -> Self {
        if (IMM_MIN..=IMM_MAX).contains(&v) {
            // 1-bit tag: payload is a signed 62-bit integer in bits 1..63.
            Value(((v as u64) << 1) | 1)
        } else {
            Self::from_ptr(heap.alloc_boxed_int(v))
        }
    }

    pub fn as_int(self, heap: &Heap) -> Option<i64> {
        if self.is_imm_int() {
            Some((self.0 as i64) >> 1)
        } else if let Some(p) = self.as_ptr() {
            match heap.kind(p) {
                ObjectKind::BoxedInt => Some(heap.boxed_int(p)),
                _ => None,
            }
        } else {
            None
        }
    }

    pub fn float(heap: &mut Heap, v: f64) -> Self {
        Self::from_ptr(heap.alloc_boxed_float(v))
    }

    pub fn as_float(self, heap: &Heap) -> Option<f64> {
        let p = self.as_ptr()?;
        match heap.kind(p) {
            ObjectKind::BoxedFloat => Some(heap.boxed_float(p)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn immediate_range_round_trips() {
        let mut heap = Heap::new();
        for v in [
            0,
            1,
            -1,
            42,
            IMM_MAX,
            IMM_MIN,
            IMM_MAX - 1,
            IMM_MIN + 1,
            1 << 61,
            -(1 << 61),
        ] {
            let w = Value::int(&mut heap, v);
            assert!(w.is_imm_int(), "v={v} should be immediate");
            assert_eq!(w.as_int(&heap), Some(v), "v={v}");
        }
    }

    #[test]
    fn outside_immediate_range_is_boxed() {
        let mut heap = Heap::new();
        let hi = IMM_MAX + 1;
        let lo = IMM_MIN - 1;
        let a = Value::int(&mut heap, hi);
        let b = Value::int(&mut heap, lo);
        assert!(!a.is_imm_int());
        assert!(!b.is_imm_int());
        assert_eq!(a.as_int(&heap), Some(hi));
        assert_eq!(b.as_int(&heap), Some(lo));
    }

    #[test]
    fn bool_nil_not_ints() {
        let heap = Heap::new();
        assert!(Value::nil().as_int(&heap).is_none());
        assert!(Value::bool(true).as_int(&heap).is_none());
        assert!(Value::bool(false).as_int(&heap).is_none());
    }
}
