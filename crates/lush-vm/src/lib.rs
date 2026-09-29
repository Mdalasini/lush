//! Single-process bytecode interpreter for Lush (build step 3).

pub mod bitarray;
pub mod builtin;
pub mod heap;
pub mod inspect;
pub mod interp;
pub mod panic_report;
pub mod value;

pub use interp::{RunResult, SliceResult, Vm, VmConfig, DEFAULT_QUANTUM, MAX_STACK_FRAMES};
pub use lush_ir::{
    compile_graph, compile_source, compile_source_with_opt, compile_sources,
    compile_sources_with_opt, OptLevel, Program,
};
