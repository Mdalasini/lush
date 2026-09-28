---
name: create-issue
description: Write a GitHub issue for a unit of Lush work (usually one build step or sub-step from spec.md §15.3) that an implementer can finish without guessing. Use when asked to "open an issue", "write up step N", or "file a ticket" for this repo.
---

# Create an issue

The issue is the implementer's contract and the reviewer's checklist. Every checkbox has to be
something a reviewer can verify from the diff or a test, and nothing important may be left
optional. In #17 the first draft let part of the §12 grammar be "done if practical". Those
parts were the ones that came back broken, and the scope had to be widened afterwards (#18).

## 1. Ground it in the spec

1. Read the §15.3 build step and every section it depends on. `spec.md` is normative: quote
   it and don't paraphrase it loosely.
2. If the build step's wording is narrower than the acceptance criteria in §15.4, or narrower
   than what the next step needs, fix the spec first. Open a `docs/<topic>` PR that changes
   the step text (as #18 did), then write the issue against the corrected text.
3. List what the step does **not** need, and which later step owns each of those things.

## 2. Structure

Use these sections, in this order:

```markdown
## Summary
Implement step N of the build order in `spec.md` §15.3 (as widened by #NN):

> <quote the step verbatim>

<One or two sentences on what else this sets up (workspace, harness) and the §15.3 gate:
tests must pass before step N+1 starts.>

## Scope
### 0. <Bootstrap, if any>
- [ ] ...
### 1. <Component> (§x.y)
- [ ] <one verifiable behaviour per box, with the spec section it comes from>
### 2. ...

## Tests
- [ ] <which fixtures and snapshots prove each scope item>

## Out of scope (later steps)
- <thing>: step M, because <reason>.

## Definition of done
- <objective conditions: commands pass, every box has a test, docs list only commands that run>

## References
`spec.md` §... (<topic>), §... (<topic>)
```

## 3. Rules for checkboxes

- **Everything is required.** Don't write "if practical" or "optionally". If something truly
  belongs later, move it to *Out of scope* and name the step that owns it.
- **One behaviour per box**, phrased so it can be tested: "`a == b && c == d` parses; `a < b < c`
  is E0110", not "correct precedence".
- **Name the edge cases the spec implies**, because an implementer will miss them otherwise.
  Things that were missed in step 1:
  - an anonymous `fn` at the start of a statement
  - trailing commas after a spread (`[..xs,]`)
  - domain-prefixed import paths (`github.com/user/repo`)
  - mixed-level comparison chains (`a < b == c`)
  - `_` outside call-argument position
  - more than one capture hole in a call
  - tuple-type arity
- **Cover the non-functional requirements every parsing or runtime step needs**, even when
  the spec only implies them:
  - bounded recursion depth
  - capped diagnostics
  - no terminal escape injection in rendered errors
  - resource limits on input size
  - performance of the obvious worst cases
- **Every §12 conformance row and every §15.4 acceptance item that the step touches** gets a
  positive and a negative fixture box.
- **Spec examples:** require that the tests *extract them from `spec.md`*, not hand-copied
  fixtures. Copies drift silently.
- **Docs:** only commands that actually run go into `AGENTS.md`.

## 4. Publish

- Use the repository's GitHub tools. Search existing issues first so you don't file a
  duplicate.
- The title is imperative and scoped, e.g. "Implement build step 1: syntax (lexer, parser,
  AST, formatter)".
- If the spec changed first, link that PR in the summary. When scope changes later, edit the
  issue body in place so it stays the single source of truth, rather than adding "also do X"
  comments.
