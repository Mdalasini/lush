//! Unification.

use crate::error::TypeError;
use crate::ty::{apply, Subst, Type};
use lush_syntax::Span;

pub(crate) fn unify(
    subst: &mut Subst,
    left: &Type,
    right: &Type,
    span: Span,
) -> Result<(), TypeError> {
    let left = apply(subst, left);
    let right = apply(subst, right);
    match (left, right) {
        (Type::Var(a), Type::Var(b)) if a == b => Ok(()),
        (Type::Var(v), ty) | (ty, Type::Var(v)) => {
            if occurs(v, &ty) {
                Err(TypeError::Mismatch {
                    span,
                    message: "occurs check failed".into(),
                })
            } else {
                subst.insert(v, ty);
                Ok(())
            }
        }
        (
            Type::Named {
                module: m1,
                name: n1,
                args: a1,
            },
            Type::Named {
                module: m2,
                name: n2,
                args: a2,
            },
        ) => {
            if n1 != n2 || m1 != m2 || a1.len() != a2.len() {
                return Err(TypeError::Mismatch {
                    span,
                    message: format!("cannot unify {n1} with {n2}"),
                });
            }
            for (x, y) in a1.iter().zip(a2.iter()) {
                unify(subst, x, y, span)?;
            }
            Ok(())
        }
        (
            Type::Fn {
                params: p1,
                ret: r1,
            },
            Type::Fn {
                params: p2,
                ret: r2,
            },
        ) => {
            if p1.len() != p2.len() {
                return Err(TypeError::Mismatch {
                    span,
                    message: format!("function arity mismatch: {} vs {}", p1.len(), p2.len()),
                });
            }
            for (x, y) in p1.iter().zip(p2.iter()) {
                unify(subst, x, y, span)?;
            }
            unify(subst, &r1, &r2, span)
        }
        (Type::Tuple(a), Type::Tuple(b)) => {
            if a.len() != b.len() {
                return Err(TypeError::Mismatch {
                    span,
                    message: "tuple arity mismatch".into(),
                });
            }
            for (x, y) in a.iter().zip(b.iter()) {
                unify(subst, x, y, span)?;
            }
            Ok(())
        }
        (l, r) => Err(TypeError::Mismatch {
            span,
            message: format!("cannot unify {l:?} with {r:?}"),
        }),
    }
}

fn occurs(v: u32, ty: &Type) -> bool {
    match ty {
        Type::Var(u) => *u == v,
        Type::Named { args, .. } => args.iter().any(|a| occurs(v, a)),
        Type::Fn { params, ret } => params.iter().any(|p| occurs(v, p)) || occurs(v, ret),
        Type::Tuple(elems) => elems.iter().any(|e| occurs(v, e)),
    }
}
