# lush-types

Name resolution, desugaring (`use` / pipes / captures), value-restricted HM
inference with sealed `Eq`/`Neg` constraints, exhaustiveness, and compile-time
constant checks for Lush (build step 2, `spec.md` §15.3).

## Commands

```bash
cargo test -p lush-types --locked
cargo test --workspace --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Snapshot updates:

```bash
INSTA_UPDATE=always cargo test -p lush-types --locked
```

## Public API

- `check_module(path, module, deps, prefixes, prior_diagnostics, opts)` — check one
  module against dependency interfaces.
- `check_graph(modules, prefixes, prior_diagnostics, entry_paths)` — check a
  module graph (loads `lush/*` stubs, detects import cycles, caps diagnostics
  graph-wide).
- `check_source(path, source, entry)` — parse + check convenience helper.

## Standard library stubs

`stubs/` holds **declaration-only** stand-ins for `lush/*` until step 10. They are
**not** Lush source (the parser requires function bodies). Format:

```text
# comment
module lush/process

opaque Pid
opaque Subject(msg)

type Crash {
  Crash(message: String, trace: List(String))
}

pub fn spawn(fn() -> Nil) -> Pid
@eq(k)
pub fn insert(Dict(k, v), k, v) -> Dict(k, v)
```

- `@eq(a)` / `@neg(a)` on the line before a `pub fn` attach sealed constraints to
  that scheme (stub-loader only; not user-visible syntax).
- Stub modules load only under the reserved `lush/` namespace.

## Fixture layout

- `tests/fixtures/positive/` — must type-check with zero errors
- `tests/fixtures/negative/` — `// expect: E1xxx` headers
- `tests/fixtures/warnings/` — `// expect: W1xxx` headers
