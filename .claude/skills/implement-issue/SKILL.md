---
name: implement-issue
description: Implement a Lush GitHub issue end to end. Branch, build every checkbox with tests, open a readable PR, then keep addressing review comments and CI failures until the reviewer is satisfied and CI is green. Use when asked to "implement #N", "pick up the issue", or "fix the review comments" on a PR you own.
---

# Implement an issue

Your job isn't finished when the code compiles. It's finished when every issue checkbox is
backed by a test, CI is green, and every review thread is resolved. Most wasted review rounds
on #19 came from pushes that never ran locally (the build broke, snapshots weren't committed,
`cargo fmt` failed), not from hard problems.

## 1. Before writing code

1. Read the issue and every spec section it cites. `spec.md` is normative, and if the issue
   and the spec disagree, the spec wins; raise the conflict on the issue.
2. Branch from the latest `main` using `AGENTS.md` naming (`feat/<topic>`, `fix/<topic>`, …).
   Never commit to `main`.
3. Turn each checkbox into a planned test before coding: a fixture, snapshot or unit test.
   A box with no test isn't done.

## 2. Building it (Rust workspace conventions)

- Crates live in `crates/` (spec §15.1). Workspace dependencies go in the root `Cargo.toml`.
- Diagnostics: every error has a code from the central `codes` module, a span, and a hint
  (§11.6). Messages name the actual token (`describe()`) and escape user text with
  `escape_debug`.
- Treat all input as untrusted from day one:
  - Bound recursion depth and chain length, and report a diagnostic when a limit is hit.
  - Cap the total number of diagnostics shared across the lexer and the parser.
  - Resynchronise after errors (skip to the next statement or item).
  - Reject oversized input.
  - Sanitise control characters before rendering snippets, using a replacement of the same
    byte width so spans stay valid.
  - Use byte indexing throughout.
- Keep hot paths allocation-light. Store spans, not copied text (e.g. trivia), and borrow with
  `Cow` when no normalisation is needed.
- Don't hand-write structural equality over the AST. Strip spans and use the derived
  `PartialEq`.
- Leave no working notes ("Actually…"), dead helpers, or "reserved for future" variants.

## 3. Tests

- **Positive fixtures:** `tests/fixtures/positive/*.lush`.
- **Negative fixtures:** `tests/fixtures/negative/*.lush`. The first line must be
  `// expect: E0xxx`, and the harness asserts that code fires, not just that parsing failed.
- **Spec examples:** extract them from `spec.md` in the test. Relocate module-level lines above
  a wrapper function rather than dropping them, and assert that every fence line survived.
- **Snapshots** (insta): run `cargo insta test --accept` (or `INSTA_UPDATE=always cargo test`),
  **inspect** the `.snap` files, and commit them together with the fixtures that produce them.
- **Stress tests:** deep nesting, long operator chains, and megabytes of junk input. Assert
  bounded output and no crash; run any stack-heavy case in a thread with an explicit stack
  size.
- **Formatter:** round-trip test (idempotent and AST-equivalent) over every fixture.

## 4. Before every push (non-negotiable)

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
git status   # no stray *.snap.new; new .snap files are staged
```

Also re-read your own diff and ask what CI would reject. In particular: a field removed in one
module but still used in another, fixtures added without their snapshots, and fmt drift in
files you only touched lightly.

## 5. Opening the PR

- The title matches the issue ("Implement build step N: …"). The body starts with `Closes #N`.
- Write the description for humans, following the `AGENTS.md` PR guidance:
  - **Summary:** what it does and why.
  - **What's in it:** a short table of modules.
  - **Robustness and security:** the limits you enforce, with numbers.
  - **Tests:** what proves each scope area.
  - **Affected normative sections**, plus what's deliberately out of scope.

  Don't use tool-generated templates, HTML badges, or "open in X" buttons.
- Commit subjects are imperative and capitalised, with no trailing period, one change each.

## 6. The review loop

Subscribe to the PR's activity and handle every event until it's merged.

1. **CI red:** reproduce it locally first, then fix the root cause. Never skip, ignore or
   quarantine a test, and never push an empty commit to re-trigger CI. A build break or fmt
   failure is always yours.
2. **Review comments:** read every open thread on the current head, not only the newest
   review.
   - 🔴 (blocking) and 🟠 (spec or issue requirement) must be fixed. 🟡 cleanups should be
     fixed in the same push unless you reply with a concrete reason not to.
   - Fix the root cause the comment describes, not just the example it quotes. For instance, a
     depth limit also has to cover left-deep chains, and an error cap has to cover the lexer
     as well as the parser.
   - Add a regression test for every behavioural fix, using the reviewer's repro as the test
     input.
3. **Batch the fixes.** Push once per round, only after the §4 checklist is clean. Use one
   commit per theme ("Cap lexer diagnostics…", "Fix formatter comment placement…").
4. **Reply only to threads you won't fix,** with a reason. Leave resolving to the reviewer,
   who resolves a thread only after verifying the fix.
5. **Repeat until** every thread is resolved and CI is green on the current head. Don't argue
   scope inside the PR. If a request is genuinely out of scope, say which step owns it and
   open an issue for it.
