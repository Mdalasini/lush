# Lush

Lush is a language with a Go-like toolchain and BEAM-inspired concurrency. The
normative language definition lives in [`spec.md`](spec.md).

## Status

Implementation is underway, following the staged build order in `spec.md` §15.3.
Current crates: `lush-syntax` (lexer, parser, AST, formatter) and `lush-types`
(HM inference with value restriction, sealed `Eq`/`Neg`, exhaustiveness, and
golden type-error fixtures for §15.3 step 2).

## Development

```bash
cargo test --workspace
cargo fmt --all
```

See [`AGENTS.md`](AGENTS.md) for repository guidelines.
