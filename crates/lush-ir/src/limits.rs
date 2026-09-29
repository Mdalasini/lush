//! Documented compile-time limits (§6 / issue #24 spec point 8).

pub const MAX_REGISTERS: usize = 256;
pub const MAX_CONSTRUCTOR_ARITY: usize = 255;
pub const MAX_FUNCTIONS_PER_MODULE: usize = 20_000;
pub const MAX_CONSTANTS_PER_MODULE: usize = 100_000;
pub const MAX_INSTRUCTIONS_PER_FUNCTION: usize = 1_000_000;
pub const MAX_DECISION_NODES: usize = 100_000;
pub const MAX_INLINE_SIZE: usize = 64;
pub const MAX_INLINE_DEPTH: usize = 8;
