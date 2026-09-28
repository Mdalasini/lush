//! Type representations.

use std::collections::HashMap;

/// A monotype / type scheme component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    /// Unification variable.
    Var(u32),
    /// Named type application, e.g. `List(Int)` or qualified `actor.Next(Int)`.
    Named {
        /// Optional module qualifier.
        module: Option<String>,
        /// Type name.
        name: String,
        /// Arguments.
        args: Vec<Type>,
    },
    /// Function type.
    Fn {
        /// Parameter types.
        params: Vec<Type>,
        /// Return type.
        ret: Box<Type>,
    },
    /// Tuple type.
    Tuple(Vec<Type>),
}

impl Type {
    /// `Int`
    pub fn int() -> Self {
        Type::Named {
            module: None,
            name: "Int".into(),
            args: vec![],
        }
    }

    /// `Float`
    pub fn float() -> Self {
        Type::Named {
            module: None,
            name: "Float".into(),
            args: vec![],
        }
    }

    /// `String`
    pub fn string() -> Self {
        Type::Named {
            module: None,
            name: "String".into(),
            args: vec![],
        }
    }

    /// `Bool`
    pub fn bool() -> Self {
        Type::Named {
            module: None,
            name: "Bool".into(),
            args: vec![],
        }
    }

    /// `Nil`
    pub fn nil() -> Self {
        Type::Named {
            module: None,
            name: "Nil".into(),
            args: vec![],
        }
    }

    /// `List(a)`
    pub fn list(a: Type) -> Self {
        Type::Named {
            module: None,
            name: "List".into(),
            args: vec![a],
        }
    }

    /// `Result(ok, err)`
    pub fn result(ok: Type, err: Type) -> Self {
        Type::Named {
            module: None,
            name: "Result".into(),
            args: vec![ok, err],
        }
    }

    /// `Option(a)`
    pub fn option(a: Type) -> Self {
        Type::Named {
            module: None,
            name: "Option".into(),
            args: vec![a],
        }
    }

    /// `BitArray`
    pub fn bit_array() -> Self {
        Type::Named {
            module: None,
            name: "BitArray".into(),
            args: vec![],
        }
    }

    /// `Subject(msg)`
    pub fn subject(msg: Type) -> Self {
        Type::Named {
            module: None,
            name: "Subject".into(),
            args: vec![msg],
        }
    }
}

/// Polymorphic type scheme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scheme {
    /// Quantified variables.
    pub vars: Vec<u32>,
    /// Body.
    pub body: Type,
}

impl Scheme {
    /// Monomorphic scheme.
    pub fn mono(ty: Type) -> Self {
        Scheme {
            vars: vec![],
            body: ty,
        }
    }
}

/// Substitution map.
pub type Subst = HashMap<u32, Type>;

/// Apply a substitution to a type.
pub fn apply(subst: &Subst, ty: &Type) -> Type {
    match ty {
        Type::Var(v) => subst
            .get(v)
            .map(|t| apply(subst, t))
            .unwrap_or_else(|| Type::Var(*v)),
        Type::Named { module, name, args } => Type::Named {
            module: module.clone(),
            name: name.clone(),
            args: args.iter().map(|a| apply(subst, a)).collect(),
        },
        Type::Fn { params, ret } => Type::Fn {
            params: params.iter().map(|p| apply(subst, p)).collect(),
            ret: Box::new(apply(subst, ret)),
        },
        Type::Tuple(elems) => Type::Tuple(elems.iter().map(|e| apply(subst, e)).collect()),
    }
}

/// Free unification variables in a type.
pub fn free_vars(ty: &Type) -> Vec<u32> {
    match ty {
        Type::Var(v) => vec![*v],
        Type::Named { args, .. } => {
            let mut vs = Vec::new();
            for a in args {
                for v in free_vars(a) {
                    if !vs.contains(&v) {
                        vs.push(v);
                    }
                }
            }
            vs
        }
        Type::Fn { params, ret } => {
            let mut vs = Vec::new();
            for p in params {
                for v in free_vars(p) {
                    if !vs.contains(&v) {
                        vs.push(v);
                    }
                }
            }
            for v in free_vars(ret) {
                if !vs.contains(&v) {
                    vs.push(v);
                }
            }
            vs
        }
        Type::Tuple(elems) => {
            let mut vs = Vec::new();
            for e in elems {
                for v in free_vars(e) {
                    if !vs.contains(&v) {
                        vs.push(v);
                    }
                }
            }
            vs
        }
    }
}
