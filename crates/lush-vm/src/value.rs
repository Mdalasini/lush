//! Tagged 64-bit values (§7.1).
use crate::heap::{Heap, HeapPtr, ObjectKind};
const TAG_MASK: u64 = 0b111;
const TAG_INT: u64 = 0b001;
const TAG_BOOL_FALSE: u64 = 0b010;
const TAG_BOOL_TRUE: u64 = 0b110;
const TAG_NIL: u64 = 0b100;
const TAG_PTR: u64 = 0b000;
pub const IMM_MIN: i64 = -(1 << 62);
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
    pub fn int(heap: &mut Heap, v: i64) -> Self {
        if (IMM_MIN..=IMM_MAX).contains(&v) {
            Value(((v as u64) << 3) | TAG_INT)
        } else {
            Self::from_ptr(heap.alloc_boxed_int(v))
        }
    }
    pub fn as_int(self, heap: &Heap) -> Option<i64> {
        if self.0 & TAG_MASK == TAG_INT {
            Some((self.0 as i64) >> 3)
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
