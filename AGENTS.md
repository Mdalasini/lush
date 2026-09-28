# Repository Guidelines

Never commit directly to `main`. Make changes on a separate branch and open a pull request targeting `main`. Follow the repository's branch naming convention: a type prefix plus a short, descriptive topic in lowercase, hyphen-separated words (for example, `feat/<topic>`, `docs/<topic>`, `refactor/<topic>`, `fix/<topic>`, `chore/<topic>`, or `test/<topic>`).

Write commit subjects in the concise, imperative style of the existing history: capitalize the first word, describe one change, and omit the trailing period (for example, "Add the Lush language specification").

Pull requests should summarize the change, explain its rationale, link related discussion when available, and call out affected normative sections. Include test or manual review evidence; screenshots aren't needed for text-only changes.

Tag PRs that don't require a second look as `skip review` to stop the automatic macroscope review.

## Build and test

This repository is a Rust workspace. Implemented crates:

- `lush-syntax` — lexer, parser, AST, formatter (step 1)
- `lush-types` — name resolution, desugaring, HM inference, exhaustiveness (step 2)
- `lush-test-support` — shared `spec.md` fence inventory and fixture helpers

```bash
cargo test --workspace --locked
cargo test -p lush-syntax --locked
cargo test -p lush-types --locked
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
```

Formatter / diagnostic snapshot updates (when intentionally changing output):

```bash
cargo install cargo-insta
cargo insta review
# or: INSTA_UPDATE=always cargo test -p lush-types --locked
```

## Agent skills

Repeatable workflows live in `.claude/skills/`:

- `create-issue`: write a spec-grounded issue for a build step, where every checkbox is required and testable.
- `implement-issue`: implement an issue, open the PR, and address review comments and CI until merge.
- `review-pr`: a strict review loop (performance, security, maintainability, functionality) using `review-pr/probe.sh`, re-verifying each push before resolving threads and merging.
