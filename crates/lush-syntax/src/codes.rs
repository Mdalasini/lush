//! Diagnostic error and warning codes (§11.6).
//!
//! Single source of truth for codes used by the lexer and parser. Future
//! `lush explain` can document these from this module.

// --- Lexer ---------------------------------------------------------------

/// Unexpected character in the source.
pub const E0001_UNEXPECTED_CHAR: &str = "E0001";
/// Malformed or incomplete numeric literal.
pub const E0002_BAD_NUMBER: &str = "E0002";
/// Malformed float literal (missing digits after exponent, etc.).
pub const E0003_BAD_FLOAT: &str = "E0003";
/// Invalid string escape sequence.
pub const E0004_BAD_ESCAPE: &str = "E0004";
/// Unterminated string literal.
pub const E0005_UNTERMINATED_STRING: &str = "E0005";

// --- Parser --------------------------------------------------------------

/// Expected a different token.
pub const E0100_EXPECTED_TOKEN: &str = "E0100";
/// Invalid token after `pub`.
pub const E0101_AFTER_PUB: &str = "E0101";
/// Expected a module item.
pub const E0102_MODULE_ITEM: &str = "E0102";
/// Expected an import path segment.
pub const E0103_IMPORT_PATH: &str = "E0103";
/// Chained comparison or equality without parentheses.
pub const E0110_CHAINED_CMP: &str = "E0110";
/// Expected an expression.
pub const E0120_EXPECTED_EXPR: &str = "E0120";
/// Expected a string literal.
pub const E0121_EXPECTED_STRING: &str = "E0121";
/// Expected a field or constructor name after `.`.
pub const E0122_EXPECTED_FIELD: &str = "E0122";
/// List spread must be final / misplaced.
pub const E0130_SPREAD_FINAL: &str = "E0130";
/// Missing comma before list spread.
pub const E0131_SPREAD_COMMA: &str = "E0131";
/// Tuple arity less than 2.
pub const E0132_TUPLE_ARITY: &str = "E0132";
/// Expected a bit-array segment option.
pub const E0133_BIT_OPTION: &str = "E0133";
/// Unknown bit-array segment option name.
pub const E0134_UNKNOWN_BIT_OPTION: &str = "E0134";
/// Case arm pattern count mismatches subjects.
pub const E0140_CASE_ARITY: &str = "E0140";
/// Expected a pattern.
pub const E0150_EXPECTED_PATTERN: &str = "E0150";
/// Expected a type.
pub const E0160_EXPECTED_TYPE: &str = "E0160";
/// Expected a name.
pub const E0170_EXPECTED_NAME: &str = "E0170";
/// Expected a type/constructor name.
pub const E0171_EXPECTED_UNAME: &str = "E0171";
/// Discard `_` used where a binding name is required.
pub const E0172_DISCARD_NAME: &str = "E0172";
/// Discard `_` used as an expression (only valid as a call hole).
pub const E0180_DISCARD_EXPR: &str = "E0180";
/// More than one capture hole in a call.
pub const E0181_MULTI_HOLE: &str = "E0181";
/// Nesting depth exceeded.
pub const E0190_TOO_DEEP: &str = "E0190";
/// Too many errors; parsing stopped.
pub const E0191_TOO_MANY_ERRORS: &str = "E0191";

// --- Warnings ------------------------------------------------------------

/// Identifier does not match conventional casing.
pub const W0001_CASING: &str = "W0001";
