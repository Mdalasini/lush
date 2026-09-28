//! Documentation example inventory helpers (`spec.md` §11.4).

use crate::error::SyntaxError;
use crate::parser::parse_module;

/// How a fenced example is treated by documentation CI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FenceClass {
    /// Complete module; parse unchanged.
    Module,
    /// Expression/statement snippet; wrap before parse.
    WrappedSnippet,
    /// Intentionally invalid; must fail to parse.
    ExpectedError,
    /// Signature-only API pseudocode; excluded from source compilation.
    ApiPseudocode,
    /// Illustrative placeholder (e.g. `...`); excluded, not silently repaired.
    IllustrativePlaceholder,
    /// Non-Lush fence (shell, ebnf, layout, manifest).
    NonLush,
}

/// One extracted fenced code block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fence {
    /// Zero-based index in document order.
    pub index: usize,
    /// 1-based start line of the opening fence.
    pub line: usize,
    /// Info string after ``` (may be empty).
    pub lang: String,
    /// Fence body without trailing fence.
    pub body: String,
}

/// Extract fenced blocks from markdown source.
pub fn extract_fences(markdown: &str) -> Vec<Fence> {
    let mut fences = Vec::new();
    let mut lines = markdown.lines().peekable();
    let mut line_no = 0usize;
    let mut index = 0usize;

    while let Some(line) = lines.next() {
        line_no += 1;
        let trimmed = line.trim_start();
        if let Some(rest) = trimmed.strip_prefix("```") {
            let lang = rest.trim().to_string();
            let start_line = line_no;
            let mut body_lines = Vec::new();
            for body in lines.by_ref() {
                line_no += 1;
                if body.trim_start().starts_with("```") {
                    break;
                }
                body_lines.push(body);
            }
            fences.push(Fence {
                index,
                line: start_line,
                lang,
                body: body_lines.join("\n"),
            });
            index += 1;
        }
    }
    fences
}

/// Wrap a snippet as a complete module for parse checking.
///
/// Top-level `import` / `const` / `type` / `fn` / `pub` lines stay at module scope.
/// Remaining statement/expression lines are placed in `pub fn main() -> Nil { ... }`.
/// Wrappers MUST NOT rewrite example tokens (§11.4).
pub fn wrap_snippet(body: &str) -> String {
    let trimmed = body.trim();
    let mut preamble = Vec::new();
    let mut stmts = Vec::new();

    for line in trimmed.lines() {
        let t = line.trim_start();
        if t.starts_with("import ")
            || t.starts_with("const ")
            || t.starts_with("pub const ")
            || t.starts_with("type ")
            || t.starts_with("pub type ")
            || t.starts_with("opaque ")
            || t.starts_with("pub opaque ")
            || t.starts_with("fn ")
            || t.starts_with("pub fn ")
        {
            preamble.push(line);
        } else {
            stmts.push(line);
        }
    }

    let mut out = String::new();
    if !preamble.is_empty() {
        out.push_str(&preamble.join("\n"));
        out.push('\n');
        if !stmts.iter().any(|l| !l.trim().is_empty()) {
            return out;
        }
        out.push('\n');
    }
    // Omit an explicit return type so expression snippets keep their inferred type (§11.4).
    out.push_str("pub fn main() {\n");
    for line in stmts {
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("}\n");
    out
}

/// Parse a documentation fence according to its class.
pub fn check_fence(class: FenceClass, body: &str) -> Result<(), Vec<SyntaxError>> {
    match class {
        FenceClass::Module => {
            parse_module(body)?;
            Ok(())
        }
        FenceClass::WrappedSnippet => {
            let wrapped = wrap_snippet(body);
            parse_module(&wrapped)?;
            Ok(())
        }
        FenceClass::ExpectedError => {
            if parse_module(body).is_ok() {
                Err(vec![SyntaxError::Parse {
                    span: crate::span::Span::point(0),
                    message: "expected parse error for ExpectedError fence".into(),
                }])
            } else {
                Ok(())
            }
        }
        FenceClass::ApiPseudocode | FenceClass::IllustrativePlaceholder | FenceClass::NonLush => {
            Ok(())
        }
    }
}

/// One inventory entry: stable key, document order index, and classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InventoryEntry {
    /// Stable key (`section/topic`) — prefer this over bare index when editing `spec.md`.
    pub key: &'static str,
    /// Zero-based fence index in document order (must stay aligned with [`extract_fences`]).
    pub index: usize,
    /// How CI treats the fence.
    pub class: FenceClass,
}

/// Spec inventory for `spec.md` (§11.4).
///
/// Keys are stable section anchors; indices must match fence order. When adding
/// or removing fences, update both the key list and indices together.
pub fn spec_inventory() -> Vec<InventoryEntry> {
    use FenceClass::*;
    [
        ("§4.2/custom-types", 0, Module),
        ("§5.1/bindings", 1, WrappedSnippet),
        ("§5.2/placeholder", 2, IllustrativePlaceholder),
        ("§5.3/pipes", 3, WrappedSnippet),
        ("§5.4/case", 4, WrappedSnippet),
        ("§5.5/use", 5, WrappedSnippet),
        ("§5.10/imports", 6, WrappedSnippet),
        ("§8/pipeline-ascii", 7, NonLush),
        ("§9.1/subject", 8, Module),
        ("§9.1/selector", 9, WrappedSnippet),
        ("§9.2/process-api", 10, ApiPseudocode),
        ("§9.3/actor-signatures", 11, ApiPseudocode),
        ("§9.3/actor-example", 12, Module),
        ("§9.4/supervisor-api", 13, ApiPseudocode),
        ("§9.5/task-snippet", 14, WrappedSnippet),
        ("§9.5/task-api", 15, ApiPseudocode),
        ("§11/toolchain", 16, NonLush),
        ("§11/project-layout", 17, NonLush),
        ("§11/lush.mod", 18, NonLush),
        ("§12/ebnf", 19, NonLush),
        ("§13/fib", 20, Module),
        ("§13/million-messages", 21, Module),
        ("§13/supervised-worker", 22, Module),
        ("§15/crates-layout", 23, NonLush),
    ]
    .into_iter()
    .map(|(key, index, class)| InventoryEntry { key, index, class })
    .collect()
}
