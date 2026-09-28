//! Type checker for Lush (`spec.md` §15.3 step 2).
//!
//! Implements Hindley-Milner inference with a syntactic value restriction and
//! sealed `Eq`/`Neg` constraints (§4.3), plus prelude stubs and `use`
//! desugaring for §11.4 documentation modules. Exhaustiveness is merged next.

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
