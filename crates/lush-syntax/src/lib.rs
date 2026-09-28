//! Syntax frontend for Lush: lexer, parser, AST, and formatter.
//!
//! This crate implements the surface language defined in `spec.md` §§3, 5, and 12.
//! Later stages (name resolution, typing) live in sibling crates.

#![deny(missing_docs)]

/// Crate version used by tooling and diagnostics.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Returns a short identity string for the syntax crate.
pub fn identity() -> String {
    format!("lush-syntax {VERSION}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_includes_crate_name() {
        assert!(identity().starts_with("lush-syntax "));
    }
}
