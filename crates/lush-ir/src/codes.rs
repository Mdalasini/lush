//! Compile-stage diagnostic codes for step 3 (`E2xxx`).
//! Does not reuse any `lush-syntax` or `lush-types` code strings.

pub const E2000_UNAVAILABLE_BUILTIN: &str = "E2000";
pub const E2001_TOO_MANY_REGISTERS: &str = "E2001";
pub const E2002_CTOR_ARITY: &str = "E2002";
pub const E2003_TOO_MANY_FUNCTIONS: &str = "E2003";
pub const E2004_TOO_MANY_CONSTANTS: &str = "E2004";
pub const E2005_TOO_MANY_INSTRUCTIONS: &str = "E2005";
pub const E2006_DECISION_TOO_COMPLEX: &str = "E2006";
pub const E2007_INLINE_BUDGET: &str = "E2007";
pub const E2008_VERIFY: &str = "E2008";
pub const E2009_TOO_MANY_ERRORS: &str = "E2009";
pub const E2010_NO_ENTRY: &str = "E2010";
pub const E2011_LOWER: &str = "E2011";

/// All defined codes (for registry coverage tests).
pub fn all_codes() -> &'static [&'static str] {
    &[
        E2000_UNAVAILABLE_BUILTIN,
        E2001_TOO_MANY_REGISTERS,
        E2002_CTOR_ARITY,
        E2003_TOO_MANY_FUNCTIONS,
        E2004_TOO_MANY_CONSTANTS,
        E2005_TOO_MANY_INSTRUCTIONS,
        E2006_DECISION_TOO_COMPLEX,
        E2007_INLINE_BUDGET,
        E2008_VERIFY,
        E2009_TOO_MANY_ERRORS,
        E2010_NO_ENTRY,
        E2011_LOWER,
    ]
}
