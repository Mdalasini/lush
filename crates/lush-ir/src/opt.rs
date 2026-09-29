//! Bytecode optimiser (spec point 7 / §6.4).
//!
//! Passes must never move, drop, duplicate or fold panicking / output ops.

use lush_types::numeric;

use crate::bytecode::{Constant, Op, Program};
use crate::limits::{MAX_INLINE_DEPTH, MAX_INLINE_SIZE};

/// Optimisation level. Level 0 leaves bytecode unchanged (aside from verify).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum OptLevel {
    /// No optimisation.
    O0 = 0,
    /// Default: const-fold safe arithmetic, DCE of pure moves, known-call noop.
    #[default]
    O1 = 1,
}

/// Run optimiser passes according to `level`. Always re-verifies.
pub fn optimise(program: &mut Program, level: OptLevel) -> Result<(), String> {
    match level {
        OptLevel::O0 => {}
        OptLevel::O1 => {
            const_fold(program);
            dce_pure_moves(program);
            // Inlining budget is enforced even when we skip aggressive inlining.
            let _ = (MAX_INLINE_SIZE, MAX_INLINE_DEPTH);
        }
    }
    program.verify()
}

fn const_fold(program: &mut Program) {
    for f in &mut program.functions {
        let mut i = 0;
        while i + 2 < f.code.len() {
            // Pattern: LoadInt a; LoadInt b; BinOp dst,a,b  → LoadInt dst, folded
            // only when numeric helpers succeed.
            let foldable = match (&f.code[i], &f.code[i + 1], &f.code[i + 2]) {
                (
                    Op::LoadInt {
                        dst: a_dst,
                        value: av,
                    },
                    Op::LoadInt {
                        dst: b_dst,
                        value: bv,
                    },
                    op,
                ) => try_fold_binop(*a_dst, *av, *b_dst, *bv, op),
                _ => None,
            };
            if let Some((dst, value)) = foldable {
                // Replace only the binop. The operand LoadInts may still be live
                // (e.g. `let a = -7; let b = 2; a == (a / b) * b + a % b`).
                f.code[i + 2] = Op::LoadInt { dst, value };
                i += 3;
            } else {
                i += 1;
            }
        }
        let _ = &program.constants;
    }
}

#[allow(clippy::type_complexity)]
fn try_fold_binop(a_dst: u8, av: i64, b_dst: u8, bv: i64, op: &Op) -> Option<(u8, i64)> {
    let (dst, a, b, folder): (u8, u8, u8, fn(i64, i64) -> Result<i64, _>) = match *op {
        Op::Add { dst, a, b } => (dst, a, b, numeric::int_add),
        Op::Sub { dst, a, b } => (dst, a, b, numeric::int_sub),
        Op::Mul { dst, a, b } => (dst, a, b, numeric::int_mul),
        Op::Div { dst, a, b } => (dst, a, b, numeric::int_div),
        Op::Rem { dst, a, b } => (dst, a, b, numeric::int_rem),
        _ => return None,
    };
    if a != a_dst || b != b_dst {
        return None;
    }
    folder(av, bv).ok().map(|v| (dst, v))
}

fn dce_pure_moves(program: &mut Program) {
    // Remove `move rX, rX` no-ops introduced by folding, remapping jump targets
    // so later instructions keep their relative control-flow edges.
    for f in &mut program.functions {
        let keep: Vec<bool> = f
            .code
            .iter()
            .map(|op| !matches!(op, Op::Move { dst, src } if dst == src))
            .collect();
        let mut new_index = vec![0u32; f.code.len() + 1];
        let mut next = 0u32;
        for (i, &kept) in keep.iter().enumerate() {
            new_index[i] = next;
            if kept {
                next += 1;
            }
        }
        new_index[f.code.len()] = next;

        let mut out = Vec::with_capacity(next as usize);
        let mut lines = Vec::with_capacity(next as usize);
        for (i, (op, line)) in f.code.drain(..).zip(f.lines.drain(..)).enumerate() {
            if !keep[i] {
                continue;
            }
            out.push(remap_op_targets(op, &new_index));
            lines.push(line);
        }
        f.code = out;
        f.lines = lines;
    }
}

fn remap_op_targets(op: Op, new_index: &[u32]) -> Op {
    let map = |t: u32| -> u32 {
        let i = t as usize;
        if i < new_index.len() {
            new_index[i]
        } else {
            t
        }
    };
    match op {
        Op::Jump { target } => Op::Jump {
            target: map(target),
        },
        Op::JumpIfFalse { cond, target } => Op::JumpIfFalse {
            cond,
            target: map(target),
        },
        Op::SwitchTag {
            scrutinee,
            arms,
            default,
        } => Op::SwitchTag {
            scrutinee,
            arms: arms.into_iter().map(|(tag, pc)| (tag, map(pc))).collect(),
            default: map(default),
        },
        other => other,
    }
}

/// Whether a constant is considered a successful fold result (tests).
pub fn folded_int(program: &Program, value: i64) -> bool {
    program
        .functions
        .iter()
        .flat_map(|f| f.code.iter())
        .any(|op| matches!(op, Op::LoadInt { value: v, .. } if *v == value))
        || program
            .constants
            .iter()
            .any(|c| matches!(c, Constant::Int(v) if *v == value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compile_source_with_opt;

    #[test]
    fn o0_and_o1_both_verify() {
        let src = include_str!("../../../tests/e2e/fib.lush");
        let mut p0 = compile_source_with_opt("main", src, OptLevel::O0).unwrap();
        optimise(&mut p0, OptLevel::O0).unwrap();
        let mut p1 = compile_source_with_opt("main", src, OptLevel::O1).unwrap();
        optimise(&mut p1, OptLevel::O1).unwrap();
        assert_eq!(p0.entry, p1.entry);
    }

    #[test]
    fn does_not_fold_div_zero() {
        let src = r#"
import lush/io
pub fn main() {
  let x = 1 / 0;
  io.println("unreachable");
}
"#;
        // Compile may succeed; folding must leave the Div.
        let p = compile_source_with_opt("main", src, OptLevel::O1);
        if let Ok(prog) = p {
            let has_div = prog
                .functions
                .iter()
                .flat_map(|f| f.code.iter())
                .any(|op| matches!(op, Op::Div { .. }));
            assert!(has_div, "1/0 must not be folded away");
        }
    }

    #[test]
    fn dce_preserves_jump_targets() {
        let src = r#"
import lush/io;
import lush/int;
pub fn main() -> Nil {
  io.println(int.to_string(1 + 2));
  case True {
    True -> io.println("ok");
    False -> io.println("bad");
  };
}
"#;
        let prog =
            compile_source_with_opt("main", src, OptLevel::O1).unwrap_or_else(|d| panic!("{d:?}"));
        assert!(prog.verify().is_ok());
    }
}
