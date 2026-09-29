# lush-ir

Core IR (ANF), decision-tree pattern matching, optimiser, bytecode compiler,
verifier, and deterministic `.lushc` serialisation for Lush (build step 3).

```bash
cargo test -p lush-ir --locked
```

Depends on `lush-syntax` and `lush-types`. Does not use `unsafe`.
