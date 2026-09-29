# lush-vm

Single-process bytecode interpreter for build step 3. The current foundation
executes the verified integer/control-flow bytecode subset with resumable
instruction quanta and checked arithmetic. Aggregate values, calls, builtins,
full panic reports, and the remaining VM semantics are not yet implemented.

## Commands

```bash
cargo test -p lush-vm --locked
cargo fmt --all -- --check
cargo clippy -p lush-vm --all-targets --locked -- -D warnings
```
