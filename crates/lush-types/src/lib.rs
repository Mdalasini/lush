//! Type checker for Lush (`spec.md` §15.3 step 2).
//!
//! Implements sealed `Eq`/`Neg` constraints (§4.3), prelude stubs, and `use`
//! desugaring sufficient to type-check complete documentation modules from
//! §11.4. Value restriction and exhaustiveness are separate follow-on work
//! on this branch.

#![deny(missing_docs)]

mod check;
mod desugar;
mod env;
mod error;
mod ty;
mod unify;

pub use check::{typecheck_module, typecheck_source};
pub use error::TypeError;
pub use ty::Type;

/// Crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
