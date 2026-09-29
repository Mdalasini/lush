//! Bytecode verifier diagnostic codes.
//!
//! E2000–E2099 are reserved for the compiler IR and bytecode verifier.

/// The program entry function index is out of range.
pub const E2000_ENTRY_OUT_OF_RANGE: &str = "E2000";
/// A function declares more registers than the VM supports.
pub const E2001_REGISTER_LIMIT: &str = "E2001";
/// A function's parameter count exceeds its register count.
pub const E2002_ARITY_EXCEEDS_REGISTERS: &str = "E2002";
/// A function has no instructions.
pub const E2003_EMPTY_FUNCTION: &str = "E2003";
/// The function's source-location table does not match its code length.
pub const E2004_DEBUG_INFO_MISMATCH: &str = "E2004";
/// An instruction references a register outside the function frame.
pub const E2005_REGISTER_OUT_OF_RANGE: &str = "E2005";
/// An instruction jumps outside the function's code.
pub const E2006_JUMP_TARGET_OUT_OF_RANGE: &str = "E2006";
/// A function does not end in a supported terminator.
pub const E2007_MISSING_TERMINATOR: &str = "E2007";
/// Verification stopped after reaching the diagnostic limit.
pub const E2099_TOO_MANY_ERRORS: &str = "E2099";

/// All verifier diagnostic codes, in stable numeric order.
pub const fn all_codes() -> &'static [&'static str] {
    &[
        E2000_ENTRY_OUT_OF_RANGE,
        E2001_REGISTER_LIMIT,
        E2002_ARITY_EXCEEDS_REGISTERS,
        E2003_EMPTY_FUNCTION,
        E2004_DEBUG_INFO_MISMATCH,
        E2005_REGISTER_OUT_OF_RANGE,
        E2006_JUMP_TARGET_OUT_OF_RANGE,
        E2007_MISSING_TERMINATOR,
        E2099_TOO_MANY_ERRORS,
    ]
}
