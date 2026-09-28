//! Type checker for Lush (`spec.md` §15.3 step 2).
//!
//! Includes case exhaustiveness and redundancy warnings (§5.4). Value
//! restriction and sealed `Eq`/`Neg` constraints are separate follow-on work
//! on this branch.

#![deny(missing_docs)]

mod check;
mod desugar;
mod env;
mod error;
mod exhaust;
mod ty;
mod unify;

pub use check::{
    typecheck_module, typecheck_module_with_warnings, typecheck_source,
    typecheck_source_with_warnings,
};
pub use error::{TypeError, TypeWarning};
pub use ty::Type;

/// Crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
