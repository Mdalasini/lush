//! Minimal Hindley-Milner type checker for Lush (`spec.md` §4.3, §15.3 step 2 start).
//!
//! This crate provides enough inference and prelude/stdlib stubs to type-check
//! documentation fixtures from §11.4. Full exhaustiveness and sealed `Eq`/`Neg`
//! constraint propagation continue to deepen in later work.

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
