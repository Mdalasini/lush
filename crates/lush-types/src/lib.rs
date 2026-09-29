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
pub mod node_ids;
pub mod numeric;
pub mod resolve;
pub mod stubs;
pub mod ty;
pub mod typed;
pub mod unify;

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use lush_syntax::ast::Module;
use lush_syntax::diagnostic::{Diagnostic, Severity};
use lush_syntax::span::Span;

use crate::diag::TypeSink;
use crate::interface::ModuleInterface;
use crate::ty::TypeStore;

/// Result of checking one module.
pub struct CheckResult {
    pub interface: ModuleInterface,
    /// Desugared module (captures/pipes/use rewritten).
    pub module: Module,
    /// Typed-AST handoff for Core IR lowering. `None` on early error exits
    /// (parse failure, node/def limits, import cycles) where inference never ran.
    pub typed: Option<typed::TypedModule>,
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
    check_module_with_budget(path, module, deps, prefixes, prior_diagnostics, opts, None)
}

fn check_module_with_budget(
    path: &str,
    module: &Module,
    deps: &BTreeMap<String, ModuleInterface>,
    prefixes: &[String],
    prior_diagnostics: Vec<Diagnostic>,
    opts: CheckOptions,
    budget: Option<diag::DiagBudget>,
) -> CheckResult {
    let mut sink = if let Some(b) = budget {
        TypeSink::from_existing_with_budget(prior_diagnostics, b)
    } else {
        TypeSink::from_existing(prior_diagnostics)
    };
    let mut store = TypeStore::new();

    // Count AST nodes up-front (E1309).
    let nodes = count_ast_nodes(module);
    if nodes > limits::MAX_NODES_PER_MODULE {
        sink.error(
            codes::E1309_NODE_LIMIT,
            format!(
                "module exceeds AST node limit ({nodes} > {})",
                limits::MAX_NODES_PER_MODULE
            ),
            module.span,
            Some("split the module into smaller files".into()),
        );
        let module = module.clone();
        return CheckResult {
            interface: ModuleInterface::empty(path),
            typed: None,
            module,
            diagnostics: sink.into_diagnostics(),
            work: 0,
        };
    }

    // Prefer the caller's prelude so TypeDefIds match stub schemes.
    let prelude = if let Some(existing) = deps.get("prelude").cloned() {
        existing
    } else {
        stubs::install_prelude_builtins(&mut store)
    };
    for iface in deps.values() {
        for (id, def) in &iface.type_defs {
            store.defs.entry(*id).or_insert_with(|| def.clone());
        }
    }

    let mut module = module.clone();
    let gensyms = crate::desugar::desugar_module(&mut module, &mut sink);
    // Dense NodeIds after desugar so typed tables key by the AST's own ids.
    let _ = crate::node_ids::assign_node_ids(&mut module);

    let mut ctx = resolve::ResolveCtx {
        store: &mut store,
        sink: &mut sink,
        deps,
        prelude: &prelude,
        prefixes,
    };
    let mut resolved = resolve::resolve_module(path, &module, &mut ctx);

    // Qualified access is only via the module alias table (no bare enrichment).
    let _ = &mut resolved;

    let (interface, mut typed_builder) = infer::infer_module(
        path,
        &module,
        &mut resolved,
        &mut store,
        &mut sink,
        deps,
        opts.entry,
    );
    typed_builder.gensyms = gensyms.into_iter().collect();

    let work = store.work;
    let typed = typed_builder.build(path.to_string(), module.clone(), store);
    CheckResult {
        interface,
        typed: Some(typed),
        module,
        diagnostics: sink.into_diagnostics(),
        work,
    }
}

fn count_ast_nodes(module: &Module) -> usize {
    // Approximate: items + a DFS over expressions/patterns would be ideal;
    // use a cheap estimate that still catches pathological modules.
    let mut n = module.items.len();
    for item in &module.items {
        n = n.saturating_add(1);
        match item {
            lush_syntax::ast::ModuleItem::Fn(f) => {
                n = n.saturating_add(count_block_nodes(&f.body));
                n = n.saturating_add(f.params.len());
            }
            lush_syntax::ast::ModuleItem::Const(c) => {
                n = n.saturating_add(count_expr_nodes(&c.value));
            }
            lush_syntax::ast::ModuleItem::Type(td) => {
                n = n.saturating_add(1 + td.tvars.len());
                if let lush_syntax::ast::TypeDefBody::Adt(vs) = &td.body {
                    n = n.saturating_add(vs.len());
                }
            }
            lush_syntax::ast::ModuleItem::Import(_) => {}
        }
        if n > limits::MAX_NODES_PER_MODULE {
            return n;
        }
    }
    n
}

fn count_block_nodes(b: &lush_syntax::ast::Block) -> usize {
    let mut n = b.statements.len();
    for s in &b.statements {
        match s {
            lush_syntax::ast::Statement::Expr(e) => {
                n = n.saturating_add(count_expr_nodes(e));
            }
            lush_syntax::ast::Statement::Let(l) => {
                n = n.saturating_add(count_expr_nodes(&l.value));
            }
            lush_syntax::ast::Statement::Fn(f) => {
                n = n.saturating_add(count_block_nodes(&f.body));
            }
            lush_syntax::ast::Statement::Use(u) => {
                n = n.saturating_add(count_expr_nodes(&u.value));
            }
        }
    }
    n
}

fn count_expr_nodes(e: &lush_syntax::ast::Expr) -> usize {
    use lush_syntax::ast::ExprKind;
    // Iterative so a left-nested 4096-term chain does not blow the native stack.
    let mut n = 0usize;
    let mut stack: Vec<&lush_syntax::ast::Expr> = vec![e];
    while let Some(e) = stack.pop() {
        n = n.saturating_add(1);
        if n > limits::MAX_NODES_PER_MODULE {
            return n;
        }
        match &e.kind {
            ExprKind::Tuple(xs) => {
                for x in xs {
                    stack.push(x);
                }
            }
            ExprKind::List { items, spread } => {
                for x in items {
                    stack.push(x);
                }
                if let Some(s) = spread {
                    stack.push(s);
                }
            }
            ExprKind::Call { callee, args } => {
                stack.push(callee);
                for a in args {
                    if let lush_syntax::ast::ArgValue::Expr(inner) = &a.value {
                        stack.push(inner);
                    }
                }
            }
            ExprKind::Binary { left, right, .. } | ExprKind::Pipe { left, right } => {
                stack.push(right);
                stack.push(left);
            }
            ExprKind::Unary { expr, .. }
            | ExprKind::Paren(expr)
            | ExprKind::Echo(expr)
            | ExprKind::Field { base: expr, .. }
            | ExprKind::Assert { expr, .. } => {
                stack.push(expr);
            }
            ExprKind::Fn { body, .. } | ExprKind::Block(body) => {
                n = n.saturating_add(count_block_nodes(body));
            }
            ExprKind::Case { subjects, clauses } => {
                for s in subjects {
                    stack.push(s);
                }
                for c in clauses {
                    stack.push(&c.body);
                    if let Some(g) = &c.guard {
                        stack.push(g);
                    }
                    n = n.saturating_add(c.patterns.len());
                }
            }
            ExprKind::BitArray(segs) => {
                for s in segs {
                    stack.push(&s.value);
                }
            }
            ExprKind::RecordUpdate { base, fields, .. } => {
                stack.push(base);
                for (_, v) in fields {
                    stack.push(v);
                }
            }
            _ => {}
        }
    }
    n
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
    let budget = diag::DiagBudget::new(
        prior_diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Error)
            .count(),
        prior_diagnostics
            .iter()
            .filter(|d| d.severity == Severity::Warning)
            .count(),
    );

    // Seed graph-level diagnostics (lush shadow, module limit, cycles) into a
    // separate list — not cloned into every module.
    let mut graph_diags = prior_diagnostics;

    for (path, module) in modules {
        if path.starts_with("lush/") && !crate::stubs::is_stub_path(path) {
            graph_diags.push(Diagnostic::error(
                codes::E1007_RESERVED_LUSH,
                format!("module path `{path}` shadows the reserved `lush/` namespace"),
                module.span,
                Some("choose a path outside `lush/`".into()),
                lush_syntax::diagnostic::DiagnosticKind::Type,
            ));
        }
    }

    if modules.len() > limits::MAX_MODULES {
        graph_diags.push(Diagnostic::error(
            codes::E1307_MODULE_LIMIT,
            format!(
                "too many modules in graph ({} > {})",
                modules.len(),
                limits::MAX_MODULES
            ),
            Span::default(),
            None,
            lush_syntax::diagnostic::DiagnosticKind::Type,
        ));
        // Do not type-check an over-limit graph — return the limit diagnostic only.
        return (BTreeMap::new(), graph_diags);
    }

    let mut store = TypeStore::new();
    let prelude = stubs::install_prelude_builtins(&mut store);
    let stubs_map = stubs::load_all_stubs(&mut store);

    let mut deps: BTreeMap<String, ModuleInterface> = BTreeMap::new();
    deps.insert("prelude".into(), prelude);
    for (k, v) in stubs_map {
        deps.insert(k, v);
    }

    // Build import graph among user modules.
    let module_map: BTreeMap<&str, &Module> =
        modules.iter().map(|(p, m)| (p.as_str(), m)).collect();
    let mut import_graph: HashMap<String, Vec<String>> = HashMap::new();
    let mut import_span: HashMap<(String, String), Span> = HashMap::new();
    for (path, module) in modules {
        if path.starts_with("lush/") {
            continue;
        }
        let mut imports = Vec::new();
        for item in &module.items {
            if let lush_syntax::ast::ModuleItem::Import(imp) = item {
                let joined = imp.path.segments.join("/");
                // Only edges to other modules in this graph matter for topo/cycles.
                if module_map.contains_key(joined.as_str()) {
                    import_span.insert((path.clone(), joined.clone()), imp.span);
                    imports.push(joined);
                }
            }
        }
        import_graph.insert(path.clone(), imports);
    }

    // Kahn topological sort; collect cycle members.
    let (order, cycle_members) = topo_sort(&import_graph);
    if !cycle_members.is_empty() {
        // One E1006 with span at an offending import.
        let mut cycle_list: Vec<_> = cycle_members.iter().cloned().collect();
        cycle_list.sort();
        let span = cycle_list
            .iter()
            .find_map(|m| {
                import_graph.get(m).and_then(|imps| {
                    imps.iter().find_map(|dep| {
                        if cycle_members.contains(dep) {
                            import_span.get(&(m.clone(), dep.clone())).copied()
                        } else {
                            None
                        }
                    })
                })
            })
            .unwrap_or_default();
        graph_diags.push(Diagnostic::error(
            codes::E1006_IMPORT_CYCLE,
            format!("import cycle: {}", cycle_list.join(" -> ")),
            span,
            None,
            lush_syntax::diagnostic::DiagnosticKind::Type,
        ));
    }

    let mut results: BTreeMap<String, CheckResult> = BTreeMap::new();
    let mut all_diags = graph_diags;

    for path in &order {
        if path.starts_with("lush/") {
            continue;
        }
        if cycle_members.contains(path) {
            // Skip cycle members — they already have E1006.
            let module = module_map
                .get(path.as_str())
                .cloned()
                .cloned()
                .unwrap_or(Module {
                    items: vec![],
                    span: Span::default(),
                });
            results.insert(
                path.clone(),
                CheckResult {
                    interface: ModuleInterface::empty(path),
                    typed: None,
                    module,
                    diagnostics: vec![],
                    work: 0,
                },
            );
            continue;
        }
        let Some(module) = module_map.get(path.as_str()) else {
            continue;
        };

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
        if def_count > limits::MAX_DEFS_PER_MODULE {
            let diag = Diagnostic::error(
                codes::E1308_DEF_LIMIT,
                format!(
                    "too many definitions in module ({} > {})",
                    def_count,
                    limits::MAX_DEFS_PER_MODULE
                ),
                module.span,
                None,
                lush_syntax::diagnostic::DiagnosticKind::Type,
            );
            all_diags.push(diag.clone());
            let module = (*module).clone();
            results.insert(
                path.clone(),
                CheckResult {
                    interface: ModuleInterface::empty(path),
                    typed: None,
                    module,
                    diagnostics: vec![diag],
                    work: 0,
                },
            );
            continue;
        }

        let opts = CheckOptions {
            entry: entry_paths.iter().any(|p| p == path),
        };
        let result = check_module_with_budget(
            path,
            module,
            &deps,
            prefixes,
            vec![],
            opts,
            Some(budget.clone()),
        );
        deps.insert(path.clone(), result.interface.clone());
        all_diags.extend(result.diagnostics.iter().cloned());
        results.insert(path.clone(), result);
    }

    // Modules listed but not in order (shouldn't happen) — still check orphans.
    for (path, module) in modules {
        if path.starts_with("lush/") || results.contains_key(path) {
            continue;
        }
        let opts = CheckOptions {
            entry: entry_paths.iter().any(|p| p == path),
        };
        let result = check_module_with_budget(
            path,
            module,
            &deps,
            prefixes,
            vec![],
            opts,
            Some(budget.clone()),
        );
        deps.insert(path.clone(), result.interface.clone());
        all_diags.extend(result.diagnostics.iter().cloned());
        results.insert(path.clone(), result);
    }

    (results, all_diags)
}

/// Kahn topological sort. Returns (order, nodes_in_cycles).
fn topo_sort(graph: &HashMap<String, Vec<String>>) -> (Vec<String>, HashSet<String>) {
    let mut indeg: HashMap<String, usize> = HashMap::new();
    for (n, deps) in graph {
        indeg.entry(n.clone()).or_insert(0);
        for d in deps {
            *indeg.entry(d.clone()).or_insert(0) += 1;
            indeg.entry(n.clone()).or_insert(0);
        }
    }
    // Ensure all nodes appear
    for n in graph.keys() {
        indeg.entry(n.clone()).or_insert(0);
    }
    let mut q: VecDeque<String> = indeg
        .iter()
        .filter(|(_, &d)| d == 0)
        .map(|(k, _)| k.clone())
        .collect();
    // Deterministic
    let mut q_vec: Vec<_> = q.drain(..).collect();
    q_vec.sort();
    q.extend(q_vec);

    let mut order = Vec::new();
    let mut remaining = indeg.clone();
    while let Some(n) = q.pop_front() {
        order.push(n.clone());
        if let Some(deps) = graph.get(&n) {
            let mut unlocked = Vec::new();
            for d in deps {
                if let Some(e) = remaining.get_mut(d) {
                    *e = e.saturating_sub(1);
                    if *e == 0 {
                        unlocked.push(d.clone());
                    }
                }
            }
            unlocked.sort();
            q.extend(unlocked);
        }
        remaining.remove(&n);
    }

    // Kahn on a reversed edge sense: our edges are importer → importee (dependency).
    // We want dependencies before importers, so reverse the order.
    // Actually: edge A→B means A imports B, so B must be checked first.
    // Kahn with indegree counting incoming "I am imported by" edges:
    // Wait - we incremented indeg[dep] for each import, so indeg[B] = number of
    // modules that import B. That's wrong for "deps first".
    //
    // Correct: edge importer→dependency, indegree(dependency) counts importers.
    // Nodes with indegree 0 are never imported = leaves = should be LAST.
    // Nodes that nothing depends on...
    //
    // We want: process dependencies first. So edges should be dependency → importer
    // (dependency before importer), OR we reverse at the end.
    // Current: A imports B => edge A→B, indeg[B]++. B has higher indegree.
    // Kahn emits A (indeg 0) first, then B — WRONG (importer first).
    // Fix: reverse order at end so dependencies come first...
    // If A→B (A imports B), order after Kahn: A then B. Reverse: B then A. Correct!
    order.reverse();

    let cycle: HashSet<String> = remaining.keys().cloned().collect();
    (order, cycle)
}

/// Convenience: parse source and type-check a single module with stubs.
pub fn check_source(path: &str, source: &str, entry: bool) -> CheckResult {
    let parsed = lush_syntax::parse_module(source);
    let prior = parsed.diagnostics;
    // Skip type-checking when parse has errors.
    if prior.iter().any(|d| d.severity == Severity::Error) {
        let module = parsed.module.unwrap_or(Module {
            items: vec![],
            span: Span::default(),
        });
        return CheckResult {
            interface: ModuleInterface::empty(path),
            typed: None,
            module,
            diagnostics: prior,
            work: 0,
        };
    }
    let module = match parsed.module {
        Some(m) => m,
        None => {
            let module = Module {
                items: vec![],
                span: Span::default(),
            };
            return CheckResult {
                interface: ModuleInterface::empty(path),
                typed: None,
                module,
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
