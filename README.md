# Lush

Lush is a language with a Go-like toolchain and BEAM-inspired concurrency. The
normative language definition lives in [`spec.md`](spec.md).

## Status

Implementation is underway, following the staged build order in `spec.md` §15.3.
The first crate is `lush-syntax` (lexer, parser, AST, formatter).

## Development

```bash
cargo test --workspace
cargo fmt --all
```

See [`AGENTS.md`](AGENTS.md) for repository guidelines.
