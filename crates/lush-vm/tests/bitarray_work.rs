//! Bit-array match views must charge O(n) words for a byte-by-byte peel (#24 §13 / §7.4).

use lush_vm::bitarray::cursor_from_value;
use lush_vm::heap::Heap;
use lush_vm::value::Value;

#[test]
fn byte_peel_is_linear_in_words() {
    let n = 65_536usize;
    let mut heap = Heap::new();
    let p = heap.alloc_bit_array(vec![0u8; n], (n as u64) * 8);
    let before = heap.words_allocated;
    let mut cur_v = Value::from_ptr(p);
    for _ in 0..n {
        let mut cur = cursor_from_value(&heap, cur_v).expect("cursor");
        assert!(cur.take_int(8, false, false).is_some());
        let (off, len) = cur.rest_view();
        // Unaligned `:bytes` remainder must fail (absolute offset after 4 bits).
        let bits = heap.bit_array_rc(cur_v.as_ptr().unwrap()).expect("rc");
        let next = heap.alloc_bit_array_view(bits, off, len);
        cur_v = Value::from_ptr(next);
    }
    let grew = heap.words_allocated.saturating_sub(before);
    // One O(1) view (~3 words) per byte. Reject quadratic growth.
    let limit = (n as u64) * 8;
    assert!(
        grew < limit,
        "expected O(n) heap growth peeling {n} bytes, grew {grew} words (limit {limit})"
    );
    assert_eq!(
        cursor_from_value(&heap, cur_v).map(|c| c.remaining()),
        Some(0)
    );
}

#[test]
fn bytes_rest_rejects_unaligned_offset() {
    let mut heap = Heap::new();
    let p = heap.alloc_bit_array(vec![0u8], 8);
    let mut cur = cursor_from_value(&heap, Value::from_ptr(p)).unwrap();
    assert!(cur.take_int(4, false, false).is_some());
    assert!(!cur.rest_aligned(true));
    assert!(cur.rest_aligned(false));
}
