//! Core IR and bytecode compiler for Lush (build step 3).
//!
//! Pipeline: typed AST → bytecode emission → verify.

pub mod bytecode;
pub mod codes;
pub mod dump;
pub mod emit;
pub mod limits;
pub mod lushc;

use std::collections::BTreeMap;

use lush_syntax::ast::Module;
use lush_syntax::diagnostic::{Diagnostic, Severity};
use lush_types::{check_graph, ok};

pub use bytecode::{Builtin, Constant, FuncId, Function, Op, Program, Reg};

/// Compile a module graph to a verified bytecode [`Program`].
pub fn compile_graph(
    modules: &[(String, Module)],
    prefixes: &[String],
    entry: &str,
) -> Result<Program, Vec<Diagnostic>> {
    compile_graph_with_sources(modules, prefixes, entry, BTreeMap::new())
}

/// Like [`compile_graph`], attaching module sources for panic line/column info.
pub fn compile_graph_with_sources(
    modules: &[(String, Module)],
    prefixes: &[String],
    entry: &str,
    sources: BTreeMap<String, String>,
) -> Result<Program, Vec<Diagnostic>> {
    let (results, graph_diags) = check_graph(modules, prefixes, vec![], &[entry.to_string()]);
    let mut diags = graph_diags;
    for r in results.values() {
        diags.extend(r.diagnostics.iter().cloned());
    }
    if !ok(&diags) {
        return Err(diags);
    }

    let mut typed_modules: Vec<(String, lush_types::typed::TypedModule)> = Vec::new();
    for (path, result) in results {
        if path.starts_with("lush/") {
            continue;
        }
        typed_modules.push((path, result.typed));
    }
    typed_modules.sort_by(|a, b| {
        if a.0 == entry {
            std::cmp::Ordering::Less
        } else if b.0 == entry {
            std::cmp::Ordering::Greater
        } else {
            a.0.cmp(&b.0)
        }
    });

    match emit::emit_program(&typed_modules, entry) {
        Ok(mut program) => {
            program.sources = sources;
            Ok(program)
        }
        Err(cdiags) => {
            let mut all = diags;
            all.extend(cdiags);
            Err(all)
        }
    }
}

/// Parse sources and compile. `modules` is `(path, source)`.
pub fn compile_sources(
    modules: &[(String, String)],
    entry: &str,
) -> Result<Program, Vec<Diagnostic>> {
    let mut parsed = Vec::new();
    let mut diags = Vec::new();
    let mut sources = BTreeMap::new();
    for (path, src) in modules {
        let outcome = lush_syntax::parse_module(src);
        diags.extend(outcome.diagnostics);
        sources.insert(path.clone(), outcome.source);
        if let Some(m) = outcome.module {
            parsed.push((path.clone(), m));
        }
    }
    if diags.iter().any(|d| d.severity == Severity::Error) {
        return Err(diags);
    }
    match compile_graph_with_sources(&parsed, &[], entry, sources) {
        Ok(p) => Ok(p),
        Err(mut e) => {
            e.splice(0..0, diags);
            Err(e)
        }
    }
}

/// Convenience: parse, check, and compile a single module as the entry point.
pub fn compile_source(path: &str, source: &str) -> Result<Program, Vec<Diagnostic>> {
    compile_sources(&[(path.to_string(), source.to_string())], path)
}

pub use dump::dump_program;
pub use lushc::{decode as decode_lushc, encode as encode_lushc};
