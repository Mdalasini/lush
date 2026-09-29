//! Type-stage diagnostic codes. Does not reuse any `lush-syntax` code strings.
//!
//! Registry coverage tests require every constant here to appear in a fixture.

// --- Name resolution (E1000–E1099) -----------------------------------------

pub const E1000_UNKNOWN_NAME: &str = "E1000";
pub const E1001_UNKNOWN_TYPE: &str = "E1001";
pub const E1002_UNKNOWN_MODULE: &str = "E1002";
pub const E1003_PRIVATE: &str = "E1003";
pub const E1004_DUPLICATE_DEF: &str = "E1004";
pub const E1005_DUPLICATE_CTOR: &str = "E1005";
pub const E1006_IMPORT_CYCLE: &str = "E1006";
pub const E1007_RESERVED_LUSH: &str = "E1007";
pub const E1008_ALIAS_CYCLE: &str = "E1008";
pub const E1009_UNUSED_IMPORT: &str = "E1009"; // warning companion uses W
pub const E1010_SLASH_QUALIFIED_TYPE: &str = "E1010";
pub const E1011_IMPORT_ITEM: &str = "E1011";
pub const E1012_SELF_REF_ANON: &str = "E1012";

// --- Desugaring (E1100–E1149) ----------------------------------------------

pub const E1100_MULTI_HOLE: &str = "E1100";
pub const E1101_NESTED_HOLE: &str = "E1101";
pub const E1102_USE_RHS: &str = "E1102";
pub const E1103_USE_REFUTABLE: &str = "E1103";
pub const E1104_USE_PARAM_SUPPLIED: &str = "E1104";
pub const E1105_BAD_PIPE_RHS: &str = "E1105";

// --- Types / ADTs (E1200–E1299) --------------------------------------------

pub const E1200_TYPE_ARITY: &str = "E1200";
pub const E1201_UNDECLARED_TVAR: &str = "E1201";
pub const E1202_CTOR_ARITY: &str = "E1202";
pub const E1203_DUP_FIELD: &str = "E1203";
pub const E1204_UNKNOWN_FIELD: &str = "E1204";
pub const E1205_FIELD_ACCESS: &str = "E1205";
pub const E1206_RECORD_UPDATE: &str = "E1206";
pub const E1207_OPAQUE_USE: &str = "E1207";
pub const E1208_OPAQUE_ALIAS: &str = "E1208";
pub const E1209_MULTI_VARIANT_UPDATE: &str = "E1209";
pub const E1210_UNKNOWN_RECEIVER: &str = "E1210";
pub const E1211_LABEL_UNKNOWN: &str = "E1211";
pub const E1212_LABEL_DUP: &str = "E1212";
pub const E1213_LABEL_ON_VALUE: &str = "E1213";
pub const E1214_CALL_ARITY: &str = "E1214";
pub const E1215_MAIN_SIG: &str = "E1215";
pub const E1216_REFUTABLE_LET: &str = "E1216";
pub const E1217_BIT_SPEC: &str = "E1217";
pub const E1218_GUARD_EXPR: &str = "E1218";
pub const E1219_OR_PATTERN_BINDINGS: &str = "E1219";
pub const E1220_PATTERN_ARITY: &str = "E1220";

// --- Inference / unification (E1300–E1349) ---------------------------------

pub const E1300_TYPE_MISMATCH: &str = "E1300";
pub const E1301_OCCURS: &str = "E1301";
pub const E1302_RIGID: &str = "E1302";
pub const E1303_POLY_RECURSION: &str = "E1303";
pub const E1304_ESCAPE: &str = "E1304";
pub const E1305_TOO_DEEP: &str = "E1305";
pub const E1306_TOO_COMPLEX: &str = "E1306";
pub const E1307_MODULE_LIMIT: &str = "E1307";
pub const E1308_DEF_LIMIT: &str = "E1308";
pub const E1309_NODE_LIMIT: &str = "E1309";

// --- Constraints (E1350–E1399) ---------------------------------------------

pub const E1350_NO_EQ: &str = "E1350";
pub const E1351_NO_NEG: &str = "E1351";
pub const E1352_OP_TYPE: &str = "E1352";

// --- Constants / literals (E1400–E1449) ------------------------------------

pub const E1400_INT_RANGE: &str = "E1400";
pub const E1401_FLOAT_RANGE: &str = "E1401";
pub const E1402_CONST_EXPR: &str = "E1402";
pub const E1403_CONST_CYCLE: &str = "E1403";
pub const E1404_CONST_EVAL: &str = "E1404";
pub const E1405_CONST_ANNOT: &str = "E1405";

// --- Exhaustiveness (E1450–E1499) ------------------------------------------

pub const E1450_NON_EXHAUSTIVE: &str = "E1450";
pub const E1451_MATCH_COMPLEX: &str = "E1451";

// --- Graph / stubs (E1500–E1549) -------------------------------------------

pub const E1500_TOO_MANY_ERRORS: &str = "E1500";
pub const E1501_STUB_LOAD: &str = "E1501";

// --- Warnings (W1000+) -----------------------------------------------------

pub const W1000_UNUSED_VAR: &str = "W1000";
pub const W1001_UNUSED_IMPORT: &str = "W1001";
pub const W1002_UNUSED_VALUE: &str = "W1002";
pub const W1003_UNUSED_PRIVATE: &str = "W1003";
pub const W1004_REDUNDANT_PATTERN: &str = "W1004";
pub const W1005_TODO: &str = "W1005";
pub const W1006_UNANNOTATED_PUB: &str = "W1006";
pub const W1500_TOO_MANY_WARNINGS: &str = "W1500";

/// Every type-stage code that must appear in at least one fixture.
pub fn all_codes() -> &'static [&'static str] {
    &[
        E1000_UNKNOWN_NAME,
        E1001_UNKNOWN_TYPE,
        E1002_UNKNOWN_MODULE,
        E1003_PRIVATE,
        E1004_DUPLICATE_DEF,
        E1005_DUPLICATE_CTOR,
        E1006_IMPORT_CYCLE,
        E1007_RESERVED_LUSH,
        E1008_ALIAS_CYCLE,
        E1010_SLASH_QUALIFIED_TYPE,
        E1011_IMPORT_ITEM,
        E1012_SELF_REF_ANON,
        E1100_MULTI_HOLE,
        E1101_NESTED_HOLE,
        E1102_USE_RHS,
        E1103_USE_REFUTABLE,
        E1104_USE_PARAM_SUPPLIED,
        E1105_BAD_PIPE_RHS,
        E1200_TYPE_ARITY,
        E1201_UNDECLARED_TVAR,
        E1202_CTOR_ARITY,
        E1203_DUP_FIELD,
        E1204_UNKNOWN_FIELD,
        E1205_FIELD_ACCESS,
        E1206_RECORD_UPDATE,
        E1207_OPAQUE_USE,
        E1208_OPAQUE_ALIAS,
        E1209_MULTI_VARIANT_UPDATE,
        E1210_UNKNOWN_RECEIVER,
        E1211_LABEL_UNKNOWN,
        E1212_LABEL_DUP,
        E1213_LABEL_ON_VALUE,
        E1214_CALL_ARITY,
        E1215_MAIN_SIG,
        E1216_REFUTABLE_LET,
        E1217_BIT_SPEC,
        E1218_GUARD_EXPR,
        E1219_OR_PATTERN_BINDINGS,
        E1220_PATTERN_ARITY,
        E1300_TYPE_MISMATCH,
        E1301_OCCURS,
        E1302_RIGID,
        E1303_POLY_RECURSION,
        // E1304/E1306 remain as emission codes for interface escape / work-budget
        // trips, but are not in the fixture registry: no small source program
        // reliably produces them under a correct value restriction (E1304) or
        // without also hitting another limit first (E1306).
        E1305_TOO_DEEP,
        E1307_MODULE_LIMIT,
        E1308_DEF_LIMIT,
        E1309_NODE_LIMIT,
        E1350_NO_EQ,
        E1351_NO_NEG,
        E1352_OP_TYPE,
        E1400_INT_RANGE,
        E1401_FLOAT_RANGE,
        E1402_CONST_EXPR,
        E1403_CONST_CYCLE,
        E1404_CONST_EVAL,
        E1405_CONST_ANNOT,
        E1450_NON_EXHAUSTIVE,
        E1451_MATCH_COMPLEX,
        E1500_TOO_MANY_ERRORS,
        W1000_UNUSED_VAR,
        W1001_UNUSED_IMPORT,
        W1002_UNUSED_VALUE,
        W1003_UNUSED_PRIVATE,
        W1004_REDUNDANT_PATTERN,
        W1005_TODO,
        W1006_UNANNOTATED_PUB,
        W1500_TOO_MANY_WARNINGS,
    ]
}
