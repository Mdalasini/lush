---
name: review-pr
description: Review a Lush pull request strictly for performance, security, maintainability and functionality, post inline findings, then loop. Watch for fixes, re-verify each push, resolve threads only after verification, and merge once everything passes. Use when asked to "review #N", "be a stickler", or "loop until it's ready and merge".
---

# Review a PR, then loop to merge

Reading the diff isn't enough. On #19, every blocking finding came from *running* the code
against adversarial input: stack overflows, millions of diagnostics, terminal escape
injection, misplaced labels. Build it, probe it, and only believe what you've reproduced.

## 1. Understand the change

1. Read the PR description, the linked issue, and every spec section it cites. The issue's
   checkboxes are the functional checklist.
2. Check the PR out locally in detached mode:
   `git fetch origin <branch> && git checkout --detach origin/<branch>`
3. Run the repo's gates, and note every failure:
   ```bash
   cargo fmt --all -- --check
   cargo clippy --workspace --all-targets --locked -- -D warnings
   cargo test --workspace --locked
   ```
4. Read every changed file, not just the summary diff. Look for:
   - dead code and working-note comments
   - duplicated logic, and hand-written code the compiler could derive
   - scattered error codes, and stringly-typed data where an enum belongs

## 2. Probe it

Build a probe binary against the crate, then feed it inputs from each category below.
`probe.sh` in this folder builds one in a scratch directory and runs the standard cases:
`.claude/skills/review-pr/probe.sh` (optionally with a `<path-to-scratch-dir>` argument).
Add PR-specific cases to it.

| Category | What to try |
|---|---|
| Functionality | Every issue checkbox, and every §12 conformance row (accept *and* reject). Try spec edge cases: statement-position forms, trailing commas, mixed-precedence chains, placeholders in illegal positions, arity rules, domain-prefixed paths. |
| Round trip | Run `format_round_trip` on each input. The output must be idempotent, reparse to an equivalent AST, and keep every comment *in place*. Check trailing comments, comments before `}`, and comments between variants. |
| Output quality | Lines over 80 columns, trailing whitespace (`cat -A`), redundant parentheses (e.g. `(a \|> g) \|> h`). |
| Security | 10k nested parentheses, 50k nested blocks, 100k prefix operators, 200k-term `+` and `\|>` chains, and 20k-link call chains. None may abort with a stack overflow. Also try raw ESC or BEL bytes in the source (`cat -v` the diagnostics), and non-ASCII text before an error (the label must underline the right text). |
| Performance | 4 MB of junk (the number of diagnostics must stay capped), and the time to format a 50k-element list, 80-level nesting, or a 300 KB file. Watch for super-linear growth. |

Record each result as a concrete repro: the input, the observed output, and what it should be.

## 3. Post the review

- Use one pending review with inline comments on the exact lines, then submit it as a comment.
  (GitHub won't let a PR's author request changes on their own PR.)
- Put a severity marker at the start of every comment:
  - 🔴 must fix before merge (crashes, security, broken or incorrect behaviour, red CI)
  - 🟠 should fix (a spec or issue requirement, or a test gap)
  - 🟡 cleanup (maintainability, naming, dead code)
- Each comment states the defect, the reproduction (input, then output), the spec clause or
  issue box it violates, and a concrete fix. Include a code sketch when that's quicker than
  prose.
- The review summary gives the legend, a numbered 🔴 list, and one line each for 🟠 and 🟡.
- End every GitHub post with the Claude Code attribution footer.

## 4. The loop

1. Subscribe to the PR's activity, and schedule a safety-net check-in (about 50 minutes, then
   about every 4 hours) for events that might be missed.
2. **On CI red:** reproduce it locally and post the root cause on the offending line
   (e.g. "`formatter.rs:75` reads the removed `Trivia::text`"). Include the exact command that
   shows it. Don't fix it yourself unless you were asked to drive the PR; the author owns the
   branch.
3. **On a new push:** fetch it, re-run the §1 gates, and re-run *all* the probes, not just the
   ones for the threads it claims to fix. Fixes regress other things and introduce new bugs
   (e.g. sanitising that shifted byte offsets).
4. **Resolve a thread only after you've re-run its repro and seen the correct behaviour.**
   If a fix is partial, leave the thread open and say exactly what's still missing.
5. **Post a round summary** as a PR comment each time:
   - what you verified and resolved, with evidence (numbers, timings)
   - what's new
   - what's still open, grouped by severity
6. **Skip echoes.** Your own reviews and comments come back as events; don't act on them.

## 5. Merge

Merge only when all of these hold:

- every review thread is resolved
- CI is green on the current head
- the gates pass locally, the probes are clean, and there's no merge conflict

Then:

1. Rewrite the PR description so it reads well:
   - summary with `Closes #N`
   - a module table
   - robustness and security limits, with numbers
   - tests
   - affected normative sections and what's out of scope
   - a one-line review history

   Remove tool-generated templates and badges.
2. Merge with `expectedHeadSha` set to the head you verified, using the repo's method (merge
   commit). If a permission check blocks the merge, stop and tell the user; don't work around
   it.
3. Unsubscribe from the PR, cancel any pending check-in, delete local build output
   (`target/`), and report the outcome: what was fixed, and any non-blocking follow-ups.
