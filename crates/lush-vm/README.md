# lush-vm

Single-process bytecode interpreter, tagged values, bump-arena allocator,
builtins (`lush/io`, `lush/int`), and panic reports for Lush (build step 3).

```bash
cargo test -p lush-vm --locked
```

Depends on `lush-ir`. Does not use `unsafe`. Heap reclamation arrives in step 4.
