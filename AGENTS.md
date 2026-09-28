# Repository Guidelines

## Project Structure & Module Organization

This repository currently contains the Lush language specification, not an implementation: `spec.md` is the normative source of truth and `README.md` is the project entry point. The Rust workspace, `crates/`, `stdlib/`, and `tests/` described in `spec.md` §15.1 are planned structure, not present directories. Keep specification changes in `spec.md`; add implementation files in the documented locations when those components are introduced. Avoid duplicating normative language behavior in separate documents.

## Build, Test, and Development Commands

There is no build system or executable in the current checkout. Consequently, commands such as `cargo test` and `lush test` are not yet available. When implementation begins, follow the staged build order in `spec.md` §15.3 and add the workspace manifest and test harness before documenting runnable commands here. For documentation-only edits, inspect the affected section and its cross-references before committing.

## Coding Style & Naming Conventions

Write concise Markdown with descriptive headings, fenced examples, and tables for compact reference material. Keep normative requirements (`MUST`, `MAY`, `SHOULD`) precise and consistent with the rest of `spec.md`. Lush examples use `.lush`; value/function/module names use `snake_case`, while types and constructors use `PascalCase`. The planned Rust layout and crate names are listed in §15.1; Rust-specific formatter and lint settings have not yet been established.

## Testing Guidelines

No automated tests currently exist. Specification edits should be checked for consistent terminology, valid examples, and accurate section references. The future implementation plan calls for syntax snapshots, type-checking and diagnostic fixtures, end-to-end `.lush` programs with expected output under `tests/`, and broader acceptance criteria in §15.4. Add regression fixtures alongside the feature they cover when that infrastructure exists.

## Commit & Pull Request Guidelines

Make all changes intended for `main` on a separate branch, then file a pull request targeting `main`; do not commit those changes directly to `main`. Use a descriptive branch name such as `docs/contributor-guide`.

The short Git history uses concise, imperative subjects (for example, “Add the Lush language specification”). Follow that pattern: capitalize the first word, describe one change, and omit a trailing period. Pull requests should summarize the specification or implementation change, explain its rationale, link related discussion when available, and call out affected normative sections. Include test or manual review evidence; screenshots are unnecessary for text-only changes.

## Specification and Security

Treat explicit normative statements and security requirements in `spec.md` as implementation constraints. If a proposal changes observable behavior, update the relevant specification text and acceptance criteria together. Do not imply that planned features, dependencies, or guarantees are already implemented.
