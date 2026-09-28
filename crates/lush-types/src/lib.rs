//! Type checker for Lush (`spec.md` §15.3 step 2).
//!
//! Implements Hindley-Milner inference with a syntactic value restriction,
//! sealed `Eq`/`Neg` constraints (§4.3), case exhaustiveness/redundancy
//! (§5.4), and prelude stubs / `use` desugaring for §11.4 modules.

#![deny(missing_docs)]

mod check;
mod desugar;
mod env;
mod error;
mod exhaust;
mod ty;
mod unify;

pub use check::{typecheck_module, typecheck_source, typecheck_source_with_warnings};
pub use error::{TypeError, TypeWarning};
pub use ty::Type;

/// Crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
