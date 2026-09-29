//! Iterative `inspect` rendering for `echo` (issue #24 / spec pin-down 4).

use crate::heap::{Heap, ObjectKind};
use crate::value::Value;

pub fn inspect(heap: &Heap, v: Value) -> String {
    let mut out = String::new();
    inspect_into(heap, v, &mut out, 0);
    out
}

fn inspect_into(heap: &Heap, v: Value, out: &mut String, depth: usize) {
    if depth > 64 || out.len() > 1_000_000 {
        out.push('…');
        return;
    }
    if let Some(b) = v.as_bool() {
        out.push_str(if b { "True" } else { "False" });
        return;
    }
    if v.is_nil() {
        out.push_str("Nil");
        return;
    }
    if let Some(n) = v.as_int(heap) {
        out.push_str(&n.to_string());
        return;
    }
    if let Some(f) = v.as_float(heap) {
        let mut s = format!("{f}");
        if !s.contains('.') && !s.contains('e') && !s.contains('E') {
            s.push_str(".0");
        }
        out.push_str(&s);
        return;
    }
    let Some(p) = v.as_ptr() else {
        out.push('?');
        return;
    };
    match heap.kind(p) {
        ObjectKind::String => {
            out.push('"');
            for &b in heap.string_bytes(p) {
                match b {
                    b'\n' => out.push_str("\\n"),
                    b'\r' => out.push_str("\\r"),
                    b'\t' => out.push_str("\\t"),
                    b'\\' => out.push_str("\\\\"),
                    b'"' => out.push_str("\\\""),
                    c if c < 0x20 => out.push_str(&format!("\\u{{{c:x}}}")),
                    c => out.push(c as char),
                }
            }
            out.push('"');
        }
        ObjectKind::EmptyList => out.push_str("[]"),
        ObjectKind::Cons => {
            out.push('[');
            let mut cur = v;
            let mut first = true;
            let mut n = 0usize;
            while let Some(cp) = cur.as_ptr() {
                if heap.kind(cp) != ObjectKind::Cons {
                    break;
                }
                if !first {
                    out.push_str(", ");
                }
                first = false;
                inspect_into(heap, heap.cons_head(cp), out, depth + 1);
                cur = heap.cons_tail(cp);
                n += 1;
                if n > 100_000 {
                    out.push_str(", …");
                    break;
                }
            }
            out.push(']');
        }
        ObjectKind::Tuple => {
            out.push_str("#(");
            let len = heap.tuple_len(p);
            for i in 0..len {
                if i > 0 {
                    out.push_str(", ");
                }
                inspect_into(heap, heap.tuple_field(p, i), out, depth + 1);
            }
            out.push(')');
        }
        ObjectKind::Adt => {
            let variant = heap.adt_variant(p);
            let name = match variant {
                0 => "Ok",
                1 => "Error",
                _ => "Adt",
            };
            out.push_str(name);
            let len = heap.adt_len(p);
            if len == 0 {
                return;
            }
            out.push('(');
            for i in 0..len {
                if i > 0 {
                    out.push_str(", ");
                }
                inspect_into(heap, heap.adt_field(p, i), out, depth + 1);
            }
            out.push(')');
        }
        ObjectKind::BoxedInt => out.push_str(&heap.boxed_int(p).to_string()),
        ObjectKind::BoxedFloat => {
            let mut s = format!("{}", heap.boxed_float(p));
            if !s.contains('.') && !s.contains('e') && !s.contains('E') {
                s.push_str(".0");
            }
            out.push_str(&s);
        }
        ObjectKind::Closure => out.push_str("//fn(?)"),
        ObjectKind::BitArray => out.push_str("<<…>>"),
    }
}
