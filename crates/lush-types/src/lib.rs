//! Early type-checker prototype for Lush (`spec.md` §15.3 step 2 start).
//!
//! Provides enough inference, prelude stubs, and `use` desugaring to type-check
//! complete documentation modules from §11.4. This is not yet a full
//! Hindley-Milner implementation of §4.3 (exhaustiveness, sealed `Eq`/`Neg`
//! constraints, and complete generics remain future work).

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
