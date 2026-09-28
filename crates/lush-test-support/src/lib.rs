//! Shared fixture helpers and `spec.md` fence inventory for Lush crate tests.

use std::fs;
use std::path::Path;

/// How a ` ```lush ` fence from `spec.md` is exercised.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpecFenceKind {
    /// Complete module; parse (and type-check) unchanged.
    Module,
    /// Mixed module items + statements: relocate module-level lines above a wrapper fn.
    WrapMixed,
    /// Expression fragment; wrap as a single expression statement.
    WrapExpr,
    /// API pseudocode / signature-only illustrations (§11.4).
    Pseudocode,
    /// Contains illustrative placeholders (`...`) that are not source tokens.
    Placeholder,
}

/// Inventory of every ` ```lush ` fence in `spec.md`, in document order.
/// Update this when adding fences — tests fail if the count drifts.
pub fn spec_fence_inventory() -> &'static [SpecFenceKind] {
    use SpecFenceKind::*;
    &[
        Module,      // 0 types
        WrapMixed,   // 1 bindings
        Placeholder, // 2 functions (`...;`)
        WrapExpr,    // 3 pipes
        WrapExpr,    // 4 case
        WrapMixed,   // 5 use
        WrapMixed,   // 6 imports
        Module,      // 7 Msg type
        WrapMixed,   // 8 selector
        Pseudocode,  // 9 process API
        Pseudocode,  // 10 actor API
        Module,      // 11 actor example
        Pseudocode,  // 12 supervisor API
        WrapMixed,   // 13 task await
        Pseudocode,  // 14 task API
        Module,      // 15 fib
        Module,      // 16 million processes
        Module,      // 17 supervised worker
    ]
}

/// Complete-module fences that must type-check against stubs (step 2).
pub fn complete_module_fence_indices() -> &'static [usize] {
    &[0, 7, 11, 15, 16, 17]
}

pub fn extract_lush_fences(spec: &str) -> Vec<String> {
    let mut fences = Vec::new();
    let mut rest = spec;
    while let Some(start) = rest.find("```lush\n") {
        rest = &rest[start + "```lush\n".len()..];
        let Some(end) = rest.find("```") else {
            break;
        };
        fences.push(rest[..end].to_string());
        rest = &rest[end + 3..];
    }
    fences
}

/// Relocate single-line module items above a wrapper function; keep the rest inside.
pub fn wrap_mixed(body: &str) -> String {
    let (module, stmts): (Vec<_>, Vec<_>) = body.lines().partition(|l| {
        let t = l.trim_start();
        [
            "import ",
            "const ",
            "pub const ",
            "type ",
            "pub type ",
            "pub opaque type ",
            "opaque type ",
        ]
        .iter()
        .any(|p| t.starts_with(p))
    });
    format!(
        "{}\npub fn __spec_snippet() {{\n{}\n}}\n",
        module.join("\n"),
        stmts.join("\n")
    )
}

pub fn wrap_as_expr(expr: &str) -> String {
    let body = expr.trim_end();
    let body = if body.ends_with(';') {
        body.to_string()
    } else {
        format!("{body};")
    };
    format!("pub fn __spec_snippet() {{\n{body}\n}}\n")
}

pub fn assert_fence_lines_preserved(fence: &str, src: &str, fence_index: usize) {
    for (line_no, line) in fence.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        assert!(
            src.contains(line),
            "spec.md lush fence #{fence_index} line {} was dropped by the wrapper:\n  {line:?}\n--- source ---\n{src}",
            line_no + 1
        );
    }
}

pub fn read_spec_md(manifest_dir: &Path) -> String {
    let path = manifest_dir.join("../../spec.md");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Leading `// expect: CODEx` lines.
pub fn expected_codes(src: &str) -> Vec<String> {
    src.lines()
        .take_while(|line| line.trim_start().starts_with("//"))
        .filter_map(|line| {
            let trimmed = line.trim_start().trim_start_matches("//").trim();
            trimmed
                .strip_prefix("expect:")
                .map(|rest| rest.trim().to_string())
        })
        .filter(|c| !c.is_empty())
        .collect()
}

pub fn read_lush_files(dir: &Path) -> Vec<(String, String)> {
    let mut files = Vec::new();
    if !dir.exists() {
        return files;
    }
    let mut entries: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "lush"))
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let name = path.file_stem().unwrap().to_string_lossy().into_owned();
        let src = fs::read_to_string(&path).unwrap();
        files.push((name, src));
    }
    files
}
