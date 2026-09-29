# lush-ir

Core IR and bytecode infrastructure for build step 3. This initial implementation
provides the register instruction model and a verifier; source lowering,
optimisation, serialization, and disassembly are still in progress.

## Commands

```bash
cargo test -p lush-ir --locked
cargo fmt --all -- --check
cargo clippy -p lush-ir --all-targets --locked -- -D warnings
```
