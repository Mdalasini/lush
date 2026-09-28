//! Module interfaces and deterministic interface hashes (§6 incremental cache).

use std::collections::{BTreeMap, BTreeSet};

use crate::ty::{ConstraintSet, Scheme, Type, TypeDefId, TypeDefKind};

/// An exported value (function, constant, or constructor).
#[derive(Clone, Debug)]
pub struct ExportedValue {
    pub name: String,
    pub scheme: Scheme,
    /// Constructor of this ADT, if any.
    pub constructor_of: Option<TypeDefId>,
    /// Parameter labels for named functions/constructors (external labels).
    pub labels: Vec<Option<String>>,
    pub is_const: bool,
}

/// An exported type.
#[derive(Clone, Debug)]
pub struct ExportedType {
    pub name: String,
    pub def: TypeDefId,
    pub kind: TypeDefKind,
    pub params: Vec<String>,
    /// Opaque equality requirements: which type parameters must be Eq.
    pub eq_params: BTreeSet<usize>,
    pub always_eq: bool,
    pub never_eq: bool,
    /// For aliases: whether the alias is public (aliases are never opaque).
    pub public: bool,
}

/// Checked module interface consumed by dependents.
#[derive(Clone, Debug)]
pub struct ModuleInterface {
    pub path: String,
    pub values: BTreeMap<String, ExportedValue>,
    pub types: BTreeMap<String, ExportedType>,
    /// Opaque type definitions fully described for in-module use only; dependents
    /// see constructors as private.
    pub type_defs: BTreeMap<TypeDefId, crate::ty::TypeDefInfo>,
    pub interface_hash: u64,
}

impl ModuleInterface {
    pub fn empty(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            values: BTreeMap::new(),
            types: BTreeMap::new(),
            type_defs: BTreeMap::new(),
            interface_hash: 0,
        }
    }

    pub fn recompute_hash(&mut self) {
        self.interface_hash = compute_interface_hash(self);
    }
}

/// Deterministic FNV-1a style hash over exports, schemes, constraints, and
/// opaque equality requirements. Independent of comments, formatting, private
/// definitions, and function bodies.
pub fn compute_interface_hash(iface: &ModuleInterface) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    hash_str(&mut h, &iface.path);
    for (name, v) in &iface.values {
        hash_str(&mut h, "v");
        hash_str(&mut h, name);
        hash_scheme(&mut h, &v.scheme);
        hash_str(&mut h, if v.is_const { "c" } else { "f" });
        for lab in &v.labels {
            match lab {
                Some(s) => hash_str(&mut h, s),
                None => hash_str(&mut h, "-"),
            }
        }
    }
    for (name, t) in &iface.types {
        hash_str(&mut h, "t");
        hash_str(&mut h, name);
        hash_u64(&mut h, t.def.0);
        hash_str(
            &mut h,
            match t.kind {
                TypeDefKind::Adt => "adt",
                TypeDefKind::Opaque => "opaque",
                TypeDefKind::Alias => "alias",
                TypeDefKind::Builtin => "builtin",
            },
        );
        for p in &t.params {
            hash_str(&mut h, p);
        }
        hash_str(&mut h, if t.always_eq { "ae" } else { "" });
        hash_str(&mut h, if t.never_eq { "ne" } else { "" });
        for i in &t.eq_params {
            hash_u64(&mut h, *i as u64);
        }
    }
    h
}

fn hash_str(h: &mut u64, s: &str) {
    for b in s.as_bytes() {
        *h ^= u64::from(*b);
        *h = h.wrapping_mul(0x100000001b3);
    }
    *h ^= 0xff;
    *h = h.wrapping_mul(0x100000001b3);
}

fn hash_u64(h: &mut u64, v: u64) {
    for b in v.to_le_bytes() {
        *h ^= u64::from(b);
        *h = h.wrapping_mul(0x100000001b3);
    }
}

fn hash_scheme(h: &mut u64, scheme: &Scheme) {
    hash_u64(h, scheme.vars.len() as u64);
    for v in &scheme.vars {
        hash_u64(h, v.0);
        if let Some(c) = scheme.constraints.get(v) {
            hash_constraints(h, c);
        }
    }
    hash_type(h, &scheme.body);
}

fn hash_constraints(h: &mut u64, c: &ConstraintSet) {
    hash_str(h, if c.eq { "Eq" } else { "" });
    hash_str(h, if c.neg { "Neg" } else { "" });
}

fn hash_type(h: &mut u64, ty: &Type) {
    match ty {
        Type::Var(id) => {
            hash_str(h, "V");
            hash_u64(h, id.0);
        }
        Type::Rigid(id) => {
            hash_str(h, "R");
            hash_u64(h, id.0);
        }
        Type::Error => hash_str(h, "?"),
        Type::Int => hash_str(h, "Int"),
        Type::Float => hash_str(h, "Float"),
        Type::String => hash_str(h, "String"),
        Type::Bool => hash_str(h, "Bool"),
        Type::Nil => hash_str(h, "Nil"),
        Type::BitArray => hash_str(h, "BitArray"),
        Type::List(t) => {
            hash_str(h, "List");
            hash_type(h, t);
        }
        Type::Tuple(ts) => {
            hash_str(h, "Tuple");
            hash_u64(h, ts.len() as u64);
            for t in ts {
                hash_type(h, t);
            }
        }
        Type::Fun { params, ret } => {
            hash_str(h, "Fun");
            hash_u64(h, params.len() as u64);
            for p in params {
                hash_type(h, p);
            }
            hash_type(h, ret);
        }
        Type::App { def, args } => {
            hash_str(h, "App");
            hash_u64(h, def.0);
            for a in args {
                hash_type(h, a);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ty::Type;

    #[test]
    fn hash_stable_on_private_irrelevant() {
        let mut a = ModuleInterface::empty("app/main");
        a.values.insert(
            "f".into(),
            ExportedValue {
                name: "f".into(),
                scheme: Scheme::mono(Type::fun(vec![Type::Int], Type::Int)),
                constructor_of: None,
                labels: vec![None],
                is_const: false,
            },
        );
        a.recompute_hash();
        let h1 = a.interface_hash;
        a.recompute_hash();
        assert_eq!(h1, a.interface_hash);
    }

    #[test]
    fn hash_changes_when_export_changes() {
        let mut a = ModuleInterface::empty("m");
        a.values.insert(
            "f".into(),
            ExportedValue {
                name: "f".into(),
                scheme: Scheme::mono(Type::Int),
                constructor_of: None,
                labels: vec![],
                is_const: false,
            },
        );
        a.recompute_hash();
        let h1 = a.interface_hash;
        a.values.get_mut("f").unwrap().scheme = Scheme::mono(Type::Bool);
        a.recompute_hash();
        assert_ne!(h1, a.interface_hash);
    }
}
