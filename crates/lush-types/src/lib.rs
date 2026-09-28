//! Name resolution, HM inference, exhaustiveness, and sealed constraints for Lush.
//!
//! Build step 2 (`spec.md` §15.3). Pipeline: resolve → desugar → infer →
//! exhaustiveness → typed module interface + diagnostics.

pub mod codes;
pub mod const_eval;
pub mod desugar;
pub mod diag;
pub mod eq_capability;
pub mod exhaust;
pub mod infer;
pub mod interface;
pub mod limits;
pub mod numeric;
pub mod resolve;
pub mod stubs;
pub mod ty;
pub mod unify;

use std::collections::BTreeMap;

use lush_syntax::ast::Module;
use lush_syntax::diagnostic::{Diagnostic, Severity};
use lush_syntax::span::Span;

use crate::diag::TypeSink;
use crate::interface::ModuleInterface;
use crate::resolve::{find_import_cycle, ResolveCtx};
use crate::ty::TypeStore;

/// Result of checking one module.
#[derive(Debug)]
pub struct CheckResult {
    pub interface: ModuleInterface,
    /// Desugared module (captures/pipes/use rewritten).
    pub module: Module,
    pub diagnostics: Vec<Diagnostic>,
    pub work: u64,
}

/// Options for checking.
#[derive(Clone, Debug, Default)]
pub struct CheckOptions {
    /// When true, require `pub fn main() -> Nil`.
    pub entry: bool,
}

/// Check a single module against already-checked dependency interfaces.
///
/// `deps` should include stub interfaces for any `lush/*` imports. Prelude
/// bindings are installed automatically.
pub fn check_module(
    path: &str,
    module: &Module,
    deps: &BTreeMap<String, ModuleInterface>,
    prefixes: &[String],
    prior_diagnostics: Vec<Diagnostic>,
    opts: CheckOptions,
) -> CheckResult {
    let mut sink = TypeSink::from_existing(prior_diagnostics);
    let mut store = TypeStore::new();
    let mut deps = deps.clone();
    // Prefer the caller's prelude so TypeDefIds match stub schemes. Reinstalling
    // prelude here would allocate a second Result/Option and break unification.
    let prelude = if let Some(existing) = deps.get("prelude").cloned() {
        existing
    } else {
        let fresh = stubs::install_prelude_builtins(&mut store);
        deps.insert("prelude".into(), fresh.clone());
        fresh
    };
    for iface in deps.values() {
        for (id, def) in &iface.type_defs {
            store.defs.entry(*id).or_insert_with(|| def.clone());
        }
    }

    let mut module = module.clone();
    crate::desugar::desugar_module(&mut module, &mut sink);

    let mut ctx = ResolveCtx {
        store: &mut store,
        sink: &mut sink,
        deps: &deps,
        prelude: &prelude,
        prefixes,
    };
    let mut resolved = resolve::resolve_module(path, &module, &mut ctx);

    // Copy dep values that are accessed via module alias into resolved on demand
    // Enrich resolved with all public values from imported module paths
    enrich_qualified_imports(&mut resolved, &deps);

    let interface = infer::infer_module(
        path,
        &module,
        &mut resolved,
        &mut store,
        &mut sink,
        &deps,
        opts.entry,
    );

    let work = store.work;
    CheckResult {
        interface,
        module,
        diagnostics: sink.into_diagnostics(),
        work,
    }
}

fn enrich_qualified_imports(
    resolved: &mut resolve::ResolvedModule,
    deps: &BTreeMap<String, ModuleInterface>,
) {
    // For each module alias, copy that module's public values under their bare names
    // if not already present (selective wins). Also keep path association.
    let aliases: Vec<(String, String)> = resolved
        .values
        .iter()
        .filter_map(|(alias, v)| {
            v.from_module
                .strip_prefix("module:")
                .map(|p| (alias.clone(), p.to_string()))
        })
        .collect();
    for (_alias, path) in aliases {
        let Some(iface) = deps.get(&path) else {
            continue;
        };
        for (name, v) in &iface.values {
            resolved
                .values
                .entry(name.clone())
                .or_insert_with(|| resolve::ValueInfo {
                    scheme: v.scheme.clone(),
                    public: true,
                    from_module: path.clone(),
                    labels: v.labels.clone(),
                    constructor_of: v.constructor_of,
                    is_const: v.is_const,
                    span: Span::default(),
                });
        }
        for (name, t) in &iface.types {
            resolved
                .types
                .entry(name.clone())
                .or_insert_with(|| resolve::TypeInfo {
                    def: t.def,
                    public: t.public,
                    from_module: path.clone(),
                    span: Span::default(),
                });
        }
        for (id, def) in &iface.type_defs {
            let _ = (id, def);
        }
    }
}

/// Check a module graph: `(path, module)` pairs plus declared dependency path prefixes.
///
/// Loads `lush/*` stubs automatically. Detects import cycles. Caps diagnostics
/// across the whole graph.
pub fn check_graph(
    modules: &[(String, Module)],
    prefixes: &[String],
    prior_diagnostics: Vec<Diagnostic>,
    entry_paths: &[String],
) -> (BTreeMap<String, CheckResult>, Vec<Diagnostic>) {
    // Reject lush/ shadowing for user modules first
    let mut sink = TypeSink::from_existing(prior_diagnostics);
    for (path, module) in modules {
        if path.starts_with("lush/") && !crate::stubs::is_stub_path(path) {
            sink.error(
                codes::E1007_RESERVED_LUSH,
                format!("module path `{path}` shadows the reserved `lush/` namespace"),
                module.span,
                Some("choose a path outside `lush/`".into()),
            );
        }
    }

    let mut store = TypeStore::new();
    let prelude = stubs::install_prelude_builtins(&mut store);
    let stubs_map = stubs::load_all_stubs(&mut store);

    let mut deps: BTreeMap<String, ModuleInterface> = BTreeMap::new();
    deps.insert("prelude".into(), prelude);
    for (k, v) in stubs_map {
        deps.insert(k, v);
    }

    // Build import graph for cycle detection
    let mut import_graph: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (path, module) in modules {
        let mut imports = Vec::new();
        for item in &module.items {
            if let lush_syntax::ast::ModuleItem::Import(imp) = item {
                let joined = imp.path.segments.join("/");
                imports.push(joined);
            }
        }
        import_graph.insert(path.clone(), imports);
    }
    if let Some(cycle) = find_import_cycle(&import_graph) {
        sink.error(
            codes::E1006_IMPORT_CYCLE,
            format!("import cycle: {}", cycle.join(" -> ")),
            Span::default(),
            None,
        );
    }

    // Limits
    if modules.len() > limits::MAX_MODULES {
        sink.error(
            codes::E1307_MODULE_LIMIT,
            format!(
                "too many modules in graph ({} > {})",
                modules.len(),
                limits::MAX_MODULES
            ),
            Span::default(),
            None,
        );
    }

    let mut results = BTreeMap::new();
    let mut accumulated = sink.into_diagnostics();

    // Topological-ish: just process in order; deps among user modules use prior results
    for (path, module) in modules {
        if path.starts_with("lush/") {
            continue;
        }
        // Def / node limits
        let def_count = module
            .items
            .iter()
            .filter(|i| {
                matches!(
                    i,
                    lush_syntax::ast::ModuleItem::Fn(_)
                        | lush_syntax::ast::ModuleItem::Const(_)
                        | lush_syntax::ast::ModuleItem::Type(_)
                )
            })
            .count();
        let mut local_prior = accumulated.clone();
        if def_count > limits::MAX_DEFS_PER_MODULE {
            local_prior.push(Diagnostic::error(
                codes::E1308_DEF_LIMIT,
                format!(
                    "too many definitions in module ({} > {})",
                    def_count,
                    limits::MAX_DEFS_PER_MODULE
                ),
                module.span,
                None,
                lush_syntax::diagnostic::DiagnosticKind::Type,
            ));
        }

        let opts = CheckOptions {
            entry: entry_paths.iter().any(|p| p == path),
        };
        let result = check_module(path, module, &deps, prefixes, local_prior, opts);
        deps.insert(path.clone(), result.interface.clone());
        accumulated = result.diagnostics.clone();
        results.insert(path.clone(), result);
    }

    let diagnostics = if results.is_empty() {
        accumulated
    } else {
        results
            .values()
            .last()
            .map(|r| r.diagnostics.clone())
            .unwrap_or(accumulated)
    };

    (results, diagnostics)
}

/// Convenience: parse source and type-check a single module with stubs.
pub fn check_source(path: &str, source: &str, entry: bool) -> CheckResult {
    let parsed = lush_syntax::parse_module(source);
    let prior = parsed.diagnostics;
    let module = match parsed.module {
        Some(m) => m,
        None => {
            return CheckResult {
                interface: ModuleInterface::empty(path),
                module: Module {
                    items: vec![],
                    span: Span::default(),
                },
                diagnostics: prior,
                work: 0,
            };
        }
    };
    let mut store = TypeStore::new();
    let prelude = stubs::install_prelude_builtins(&mut store);
    let mut deps = stubs::load_all_stubs(&mut store);
    deps.insert("prelude".into(), prelude);
    check_module(path, &module, &deps, &[], prior, CheckOptions { entry })
}

/// True when there are no error-severity diagnostics.
pub fn ok(diagnostics: &[Diagnostic]) -> bool {
    diagnostics.iter().all(|d| d.severity != Severity::Error)
}
