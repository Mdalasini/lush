//! Compile-time constant expression evaluation (§5.8 / §6).

use std::collections::HashMap;
use std::rc::Rc;

use lush_syntax::ast::*;
use lush_syntax::span::Span;
use lush_syntax::token::IntBase;

use crate::codes;
use crate::diag::TypeSink;
use crate::numeric::{self, NumericError};
use crate::ty::Type;

#[derive(Clone, Debug, PartialEq)]
pub enum ConstValue {
    Int(i64),
    Float(f64),
    String(String),
    Bool(bool),
    Nil,
    Tuple(Rc<Vec<ConstValue>>),
    List(Rc<Vec<ConstValue>>),
    Adt {
        name: String,
        fields: Rc<Vec<ConstValue>>,
    },
}

pub struct ConstEnv {
    pub values: HashMap<String, ConstValue>,
    pub types: HashMap<String, Type>,
    pub visiting: Vec<String>,
    /// Forward references: name → unevaluated expression body.
    pub pending: HashMap<String, Expr>,
}

impl ConstEnv {
    pub fn new() -> Self {
        Self {
            values: HashMap::new(),
            types: HashMap::new(),
            visiting: Vec::new(),
            pending: HashMap::new(),
        }
    }
}

impl Default for ConstEnv {
    fn default() -> Self {
        Self::new()
    }
}

pub fn eval_const_expr(expr: &Expr, env: &mut ConstEnv, sink: &mut TypeSink) -> Option<ConstValue> {
    match &expr.kind {
        ExprKind::Int(lit) => {
            let base = match lit.base {
                IntBase::Decimal => 10,
                IntBase::Hex => 16,
                IntBase::Octal => 8,
                IntBase::Binary => 2,
            };
            match numeric::int_literal_value(&lit.digits, base) {
                Ok(v) => Some(ConstValue::Int(v)),
                Err(_) => {
                    sink.error(
                        codes::E1400_INT_RANGE,
                        "integer literal out of range for Int",
                        expr.span,
                        Some("Int is a signed 64-bit integer".into()),
                    );
                    None
                }
            }
        }
        ExprKind::Float(lit) => match numeric::parse_float_literal(&lit.raw) {
            Ok(v) => Some(ConstValue::Float(v)),
            Err(_) => {
                sink.error(
                    codes::E1401_FLOAT_RANGE,
                    "float literal outside the finite binary64 range",
                    expr.span,
                    None,
                );
                None
            }
        },
        ExprKind::String(s) => Some(ConstValue::String(s.value.clone())),
        ExprKind::Var(n) => {
            if env.visiting.contains(&n.text) {
                sink.error(
                    codes::E1403_CONST_CYCLE,
                    format!("constant cycle involving `{}`", n.text),
                    n.span,
                    None,
                );
                return None;
            }
            if let Some(v) = env.values.get(&n.text) {
                return Some(v.clone());
            }
            // Forward reference: evaluate pending const
            if let Some(expr) = env.pending.remove(&n.text) {
                env.visiting.push(n.text.clone());
                let val = eval_const_expr(&expr, env, sink);
                env.visiting.pop();
                if let Some(ref v) = val {
                    env.values.insert(n.text.clone(), v.clone());
                }
                return val;
            }
            sink.error(
                codes::E1402_CONST_EXPR,
                format!("`{}` is not a constant expression", n.text),
                n.span,
                Some("constants may only reference other constants".into()),
            );
            None
        }
        ExprKind::Constructor(c) if c.module.is_none() => match c.name.text.as_str() {
            "True" => Some(ConstValue::Bool(true)),
            "False" => Some(ConstValue::Bool(false)),
            "Nil" => Some(ConstValue::Nil),
            name => Some(ConstValue::Adt {
                name: name.into(),
                fields: Rc::new(vec![]),
            }),
        },
        ExprKind::Tuple(items) => {
            let mut vals = Vec::new();
            for e in items {
                vals.push(eval_const_expr(e, env, sink)?);
            }
            Some(ConstValue::Tuple(Rc::new(vals)))
        }
        ExprKind::List { items, spread } => {
            if spread.is_some() {
                sink.error(
                    codes::E1402_CONST_EXPR,
                    "list spreads are not allowed in constant expressions",
                    expr.span,
                    None,
                );
                return None;
            }
            let mut vals = Vec::new();
            for e in items {
                vals.push(eval_const_expr(e, env, sink)?);
            }
            Some(ConstValue::List(Rc::new(vals)))
        }
        ExprKind::Unary {
            op: UnaryOp::Neg,
            expr: inner,
        } => {
            // Direct negation of int literal → MIN_INT rule
            if let ExprKind::Int(lit) = &inner.kind {
                let base = match lit.base {
                    IntBase::Decimal => 10,
                    IntBase::Hex => 16,
                    IntBase::Octal => 8,
                    IntBase::Binary => 2,
                };
                return match numeric::negated_int_literal_value(&lit.digits, base) {
                    Ok(v) => Some(ConstValue::Int(v)),
                    Err(_) => {
                        sink.error(
                            codes::E1400_INT_RANGE,
                            "integer literal out of range for Int",
                            expr.span,
                            None,
                        );
                        None
                    }
                };
            }
            match eval_const_expr(inner, env, sink)? {
                ConstValue::Int(v) => match numeric::int_neg(v) {
                    Ok(v) => Some(ConstValue::Int(v)),
                    Err(e) => {
                        const_eval_err(sink, expr.span, &e);
                        None
                    }
                },
                ConstValue::Float(v) => match numeric::float_neg(v) {
                    Ok(v) => Some(ConstValue::Float(v)),
                    Err(e) => {
                        const_eval_err(sink, expr.span, &e);
                        None
                    }
                },
                _ => {
                    sink.error(
                        codes::E1402_CONST_EXPR,
                        "invalid operand for constant negation",
                        expr.span,
                        None,
                    );
                    None
                }
            }
        }
        ExprKind::Unary {
            op: UnaryOp::Not,
            expr: inner,
        } => match eval_const_expr(inner, env, sink)? {
            ConstValue::Bool(b) => Some(ConstValue::Bool(!b)),
            _ => {
                sink.error(
                    codes::E1402_CONST_EXPR,
                    "constant `!` requires Bool",
                    expr.span,
                    None,
                );
                None
            }
        },
        ExprKind::Binary { left, op, right } => {
            // Short-circuit
            if matches!(op, BinOp::And | BinOp::Or) {
                let l = eval_const_expr(left, env, sink)?;
                match (op, &l) {
                    (BinOp::And, ConstValue::Bool(false)) => return Some(ConstValue::Bool(false)),
                    (BinOp::Or, ConstValue::Bool(true)) => return Some(ConstValue::Bool(true)),
                    _ => {}
                }
                let r = eval_const_expr(right, env, sink)?;
                return match (l, r) {
                    (ConstValue::Bool(a), ConstValue::Bool(b)) => {
                        Some(ConstValue::Bool(match op {
                            BinOp::And => a && b,
                            BinOp::Or => a || b,
                            _ => unreachable!(),
                        }))
                    }
                    _ => {
                        sink.error(
                            codes::E1402_CONST_EXPR,
                            "constant boolean operator requires Bool",
                            expr.span,
                            None,
                        );
                        None
                    }
                };
            }
            let l = eval_const_expr(left, env, sink)?;
            let r = eval_const_expr(right, env, sink)?;
            eval_binop(*op, l, r, expr.span, sink)
        }
        ExprKind::Paren(inner) => eval_const_expr(inner, env, sink),
        ExprKind::Call { .. }
        | ExprKind::Fn { .. }
        | ExprKind::Pipe { .. }
        | ExprKind::Field { .. }
        | ExprKind::RecordUpdate { .. }
        | ExprKind::Case { .. }
        | ExprKind::Block(_)
        | ExprKind::Todo { .. }
        | ExprKind::Panic { .. }
        | ExprKind::Assert { .. }
        | ExprKind::Echo(_) => {
            sink.error(
                codes::E1402_CONST_EXPR,
                "expression is not allowed in a constant",
                expr.span,
                Some(
                    "constants allow literals, constant references, tuple/list/ADT construction, \
                     and primitive operators"
                        .into(),
                ),
            );
            None
        }
        ExprKind::Constructor(_) => {
            sink.error(
                codes::E1402_CONST_EXPR,
                "qualified constructors in constants are not supported here",
                expr.span,
                None,
            );
            None
        }
        _ => {
            sink.error(
                codes::E1402_CONST_EXPR,
                "expression is not allowed in a constant",
                expr.span,
                None,
            );
            None
        }
    }
}

fn eval_binop(
    op: BinOp,
    l: ConstValue,
    r: ConstValue,
    span: Span,
    sink: &mut TypeSink,
) -> Option<ConstValue> {
    use BinOp::*;
    use ConstValue::*;
    let res: Result<ConstValue, NumericError> = match (op, l, r) {
        (Add, Int(a), Int(b)) => numeric::int_add(a, b).map(Int),
        (Sub, Int(a), Int(b)) => numeric::int_sub(a, b).map(Int),
        (Mul, Int(a), Int(b)) => numeric::int_mul(a, b).map(Int),
        (Div, Int(a), Int(b)) => numeric::int_div(a, b).map(Int),
        (Rem, Int(a), Int(b)) => numeric::int_rem(a, b).map(Int),
        (AddFloat, Float(a), Float(b)) => numeric::float_add(a, b).map(Float),
        (SubFloat, Float(a), Float(b)) => numeric::float_sub(a, b).map(Float),
        (MulFloat, Float(a), Float(b)) => numeric::float_mul(a, b).map(Float),
        (DivFloat, Float(a), Float(b)) => numeric::float_div(a, b).map(Float),
        (Concat, String(a), String(b)) => Ok(String(format!("{a}{b}"))),
        (Lt, Int(a), Int(b)) => Ok(Bool(a < b)),
        (LtEq, Int(a), Int(b)) => Ok(Bool(a <= b)),
        (Gt, Int(a), Int(b)) => Ok(Bool(a > b)),
        (GtEq, Int(a), Int(b)) => Ok(Bool(a >= b)),
        (LtFloat, Float(a), Float(b)) => Ok(Bool(a < b)),
        (LtEqFloat, Float(a), Float(b)) => Ok(Bool(a <= b)),
        (GtFloat, Float(a), Float(b)) => Ok(Bool(a > b)),
        (GtEqFloat, Float(a), Float(b)) => Ok(Bool(a >= b)),
        (Eq, a, b) => Ok(Bool(a == b)),
        (NotEq, a, b) => Ok(Bool(a != b)),
        _ => {
            sink.error(
                codes::E1402_CONST_EXPR,
                "invalid operand types for constant operator",
                span,
                None,
            );
            return None;
        }
    };
    match res {
        Ok(v) => Some(v),
        Err(e) => {
            const_eval_err(sink, span, &e);
            None
        }
    }
}

fn const_eval_err(sink: &mut TypeSink, span: Span, e: &NumericError) {
    sink.error(codes::E1404_CONST_EVAL, e.to_string(), span, None);
}

/// Check int literal in expression/pattern position (non-const).
pub fn check_int_lit(
    digits: &str,
    base: u32,
    directly_negated: bool,
    span: Span,
    sink: &mut TypeSink,
) -> Option<i64> {
    if directly_negated {
        match numeric::negated_int_literal_value(digits, base) {
            Ok(v) => Some(v),
            Err(_) => {
                sink.error(
                    codes::E1400_INT_RANGE,
                    "integer literal out of range for Int",
                    span,
                    None,
                );
                None
            }
        }
    } else {
        match numeric::int_literal_value(digits, base) {
            Ok(v) => Some(v),
            Err(_) => {
                sink.error(
                    codes::E1400_INT_RANGE,
                    "integer literal out of range for Int",
                    span,
                    Some(
                        "`9223372036854775808` is only valid when directly negated as \
                         `-9223372036854775808`"
                            .into(),
                    ),
                );
                None
            }
        }
    }
}
