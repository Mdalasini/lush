//! Step-3 builtins: lush/io print/println/eprintln and lush/int to_string.

use lush_ir::bytecode::Builtin;

use crate::heap::{Heap, ObjectKind};
use crate::value::Value;

pub fn call_builtin(
    builtin: Builtin,
    args: &[Value],
    heap: &mut Heap,
    stdout: &mut dyn std::io::Write,
    stderr: &mut dyn std::io::Write,
) -> Result<(Value, u64), String> {
    match builtin {
        Builtin::Print => {
            let p = args[0].as_ptr().ok_or("print expects String")?;
            if heap.kind(p) != ObjectKind::String {
                return Err("print expects String".into());
            }
            let s = heap.string_bytes(p);
            stdout.write_all(s).map_err(|e| e.to_string())?;
            Ok((Value::nil(), s.len() as u64 / 8 + 1))
        }
        Builtin::Println => {
            let p = args[0].as_ptr().ok_or("println expects String")?;
            if heap.kind(p) != ObjectKind::String {
                return Err("println expects String".into());
            }
            let s = heap.string_bytes(p).to_vec();
            stdout.write_all(&s).map_err(|e| e.to_string())?;
            stdout.write_all(b"\n").map_err(|e| e.to_string())?;
            Ok((Value::nil(), (s.len() as u64 + 1) / 8 + 1))
        }
        Builtin::Eprintln => {
            let p = args[0].as_ptr().ok_or("eprintln expects String")?;
            if heap.kind(p) != ObjectKind::String {
                return Err("eprintln expects String".into());
            }
            let s = heap.string_bytes(p).to_vec();
            stderr.write_all(&s).map_err(|e| e.to_string())?;
            stderr.write_all(b"\n").map_err(|e| e.to_string())?;
            Ok((Value::nil(), (s.len() as u64 + 1) / 8 + 1))
        }
        Builtin::IntToString => {
            let n = args[0].as_int(heap).ok_or("to_string expects Int")?;
            let s = n.to_string();
            let charge = s.len() as u64 / 8 + 1;
            let p = heap.alloc_string(s.as_bytes());
            Ok((Value::from_ptr(p), charge))
        }
    }
}
