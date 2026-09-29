//! Documented input-size and recursion budgets for the type stage (§11 non-functional).

/// Maximum nesting depth for recursive type-stage walks.
pub const MAX_DEPTH: usize = 256;

/// Maximum left-associative chain length (matches the parser's `MAX_CHAIN`).
pub const MAX_CHAIN: usize = 4096;

/// Maximum modules in one checked graph.
pub const MAX_MODULES: usize = 10_000;

/// Maximum top-level definitions (fn/const/type) per module.
pub const MAX_DEFS_PER_MODULE: usize = 50_000;

/// Approximate AST node budget per module (expressions + patterns + types).
pub const MAX_NODES_PER_MODULE: usize = 100_000;

/// Exhaustiveness work units before the match is rejected as too complex.
pub const MAX_EXHAUST_WORK: u64 = 250_000;

/// Maximum warnings retained across a module graph (then W1500).
pub const MAX_WARNINGS: usize = 100;

/// Type-printing depth before elision.
pub const MAX_PRINT_DEPTH: usize = 32;

/// Type-printing node count before elision.
pub const MAX_PRINT_NODES: usize = 256;

/// Edit-distance bound for did-you-mean hints.
pub const MAX_EDIT_DISTANCE: usize = 2;
