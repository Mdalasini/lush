# Repository Guidelines

Never commit directly to `main`. Make changes on a separate branch and open a pull request targeting `main`. Follow the repository's branch naming convention: a type prefix plus a short, descriptive topic in lowercase, hyphen-separated words (for example, `feat/<topic>`, `docs/<topic>`, `refactor/<topic>`, `fix/<topic>`, `chore/<topic>`, or `test/<topic>`).

Write commit subjects in the concise, imperative style of the existing history: capitalize the first word, describe one change, and omit the trailing period (for example, "Add the Lush language specification").

Pull requests should summarize the change, explain its rationale, link related discussion when available, and call out affected normative sections. Include test or manual review evidence; screenshots aren't needed for text-only changes.

Tag PRs that don't require a second look as `skip review` to stop the automatic macroscope review.
