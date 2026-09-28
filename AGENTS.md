# Repository Guidelines

## Project Structure & Module Organization

`spec.md` is the normative source of truth and `README.md` is the project entry point. The Rust workspace is rooted at `Cargo.toml` with crates under `crates/` (currently `lush-syntax`, `lush-types`). Planned directories from `spec.md` §15.1 (`stdlib/`, `tests/`, and additional crates) are added as those stages land. Keep specification changes in `spec.md`; avoid duplicating normative language behavior in separate documents.

## Build, Test, and Development Commands

```bash
cargo test --workspace
cargo fmt --all -- --check
```

Follow the staged build order in `spec.md` §15.3. For documentation-only edits, inspect the affected section and its cross-references before committing.

## Coding Style & Naming Conventions

Write concise Markdown with descriptive headings, fenced examples, and tables for compact reference material. Keep normative requirements (`MUST`, `MAY`, `SHOULD`) precise and consistent with the rest of `spec.md`. Lush examples use `.lush`; value/function/module names use `snake_case`, while types and constructors use `PascalCase`. The planned Rust layout and crate names are listed in §15.1; Rust-specific formatter and lint settings have not yet been established.

## Testing Guidelines

Run `cargo test --workspace` before opening a PR. Specification edits should be checked for consistent terminology, valid examples, and accurate section references. Syntax work uses unit and snapshot tests in `crates/lush-syntax`; later stages add type-checking fixtures, end-to-end `.lush` programs under `tests/`, and the acceptance criteria in §15.4. Add regression fixtures alongside the feature they cover.

## Commit & Pull Request Guidelines

Never commit directly to `main`. Make changes on a separate branch and open a pull request targeting `main`. Follow the repository's branch naming convention: a type prefix plus a short, descriptive topic in lowercase, hyphen-separated words (for example, `feat/<topic>`, `doc/contributor-guide`, `refactor/<topic>`, `fix/<topic>`). These examples are illustrative, not exhaustive.

Write commit subjects in the concise, imperative style of the existing history: capitalize the first word, describe one change, and omit the trailing period (for example, "Add the Lush language specification").

Pull requests should summarize the change, explain its rationale, link related discussion when available, and call out affected normative sections. Include test or manual review evidence; screenshots aren't needed for text-only changes.

## Specification and Security

Treat explicit normative statements and security requirements in `spec.md` as implementation constraints. If a proposal changes observable behavior, update the relevant specification text and acceptance criteria together. Do not imply that planned features, dependencies, or guarantees are already implemented.
