# Lush Language Specification (v0.1 draft)

> Lush is a statically typed, functional language with actor-based concurrency. It is written in Rust, ships as a single self-contained toolchain binary, and runs on its own runtime. It has no dependency on Erlang, the BEAM, JavaScript, or any host language, and **no foreign function interface**.

This document is the source of truth for implementation. Where it says **MUST**, the implementation has no discretion. Where it says **MAY**, the implementer can decide. Items marked **[v2]** are explicitly deferred. Language behavior, public APIs, and explicitly stated MUST requirements are normative. Named algorithms, data structures, allocation sizes, crates, and workspace organization are implementation guidance unless explicitly required; alternatives MUST preserve observable behavior and resource/fairness guarantees.

---

## 1. Goals and non-goals

### Goals
1. **Functional expressiveness:** immutable data, algebraic data types, exhaustive pattern matching, first-class functions, pipes, Hindley-Milner inference.
2. **Reliable concurrency:** isolated lightweight processes, message passing, links, monitors, supervision ("let it crash").
3. **Familiar, modern syntax:** Gleam/Rust-flavoured, braces, no significant whitespace, no operator overloading.
4. **Scale:** millions of concurrent processes, multi-core scheduling, fast persistent data structures, per-process garbage collection that never stops the world.
5. **Go-like toolchain:** one binary, zero config, `lush build` produces one self-contained executable (platform contract in §11.3).
6. **Fully standalone:** the language, runtime, stdlib, and toolchain are one Rust codebase. Users never write or link Rust, C, Erlang, or JS.

### Non-goals (v1)
- FFI of any kind (see §14).
- Multi-node distribution **[v2]**.
- Hot code reloading **[v2]**.
- Traits/typeclasses, macros, and higher-kinded types **[v2]**.
- Shared mutable state, threads, locks, or `unsafe` in user code. Ever.

---

## 2. Design summary

| Area | Decision |
|---|---|
| File extension | `.lush` |
| Toolchain binary | `lush` |
| Typing | Static, value-restricted HM inference with sealed built-in constraints, no implicit conversions |
| Paradigm | Functional, expression-oriented, no loops (recursion + tail calls) |
| Null / exceptions | None. `Result`, `Option`, and `panic` (process crash) |
| Execution | Register-based bytecode VM in Rust (Cranelift JIT/AOT **[v2]**) |
| Concurrency | Actor processes, private heaps, typed message passing |
| Scheduling | M:N, one OS thread per core, work stealing, reduction-based preemption |
| GC | Per-process generational copying; no global pause |
| Message passing | Copy into receiver-owned fragments; large binaries shared by refcount |
| Packages | Go-style: git-based import paths, `lush.mod`, `lush.sum`, minimal version selection |
| Distribution | Single self-contained executable containing runtime + bytecode (§11.3) |

---

## 3. Lexical structure

- Source is UTF-8. Files use `\n` or `\r\n` line endings (normalised to `\n`).
- **Comments:** `// line`, `/// doc comment` (attaches to next item), `//// module doc comment` (top of file). No block comments.
- **Identifiers:**
  - values, functions, variables, modules, labels: `snake_case` (`[a-z_][a-z0-9_]*`)
  - types and constructors: `PascalCase` (`[A-Z][A-Za-z0-9]*`)
  - Lexically, lowercase/underscore-start names may contain ASCII uppercase letters; the patterns above specify conventional casing, not token rejection. The formatter and compiler MUST warn on wrong casing without silently renaming identifiers. `_` is a discard/placeholder, not a readable binding; `_name` is a normal binding with unused warnings suppressed.
- **Keywords:** `as assert case const else fn if import let opaque panic pub todo type use echo`
  (`echo` prints a value with its source location to stderr; debugging aid.)
- **Integer literals:** `123`, `1_000_000`, `0xFF`, `0o17`, `0b1010`. Type `Int`.
- **Float literals:** `1.0`, `1.5e10`, `2_000.5`. A `.` is required. Type `Float`. Negative floats are prefix negation of a positive Float literal (`-1.5`), not a signed literal (§5.7).
- **String literals:** `"hello\n"`, multi-line allowed. Escapes: `\n \r \t \\ \" \u{1F600}`.
- **Bit array literals:** `<<1, 2, 3>>`, `<<"text":utf8>>`, `<<n:size(16)>>`.
- **Operators:** see §5.7.
- Every statement in a block and every module-level `import` or `const` ends with `;`, including the final expression in a block. A block returns the value of its final expression statement; an empty block returns `Nil`. `fn` and `type` definitions do not end with `;`; every `case` arm ends with `;`, including the final arm (a branch block is followed by `;` after `}`). Commas separate list/argument items; trailing commas allowed.

---

## 4. Types

### 4.1 Built-in types
| Type | Description |
|---|---|
| `Int` | 64-bit signed. Overflow **panics** (checked arithmetic). `lush/int` provides `wrapping_*` and `try_*` variants. Big integers in stdlib `lush/bigint` **[v2]**. |
| `Float` | IEEE-754 binary64. Operations that would produce NaN/Infinity from finite inputs **panic**; NaN/Inf cannot be constructed in normal code. |
| `String` | Immutable UTF-8. Indexing is by grapheme via `lush/string`, never by byte offset. |
| `Bool` | `True` / `False` |
| `Nil` | Unit type with single value `Nil` |
| `List(a)` | Immutable singly linked list |
| `#(a, b, ...)` | Tuples, arity 2+ (`#(1, "a")`) |
| `BitArray` | Immutable byte/bit sequence |
| `fn(a, b) -> c` | Function type |
| `Result(ok, err)` | `Ok(ok)` / `Error(err)` |
| `Option(a)` | `Some(a)` / `None` (defined in stdlib, but built into the prelude) |
| `Dict(k, v)` | Persistent hash map (HAMT). Keys require sealed `Eq` capability (§4.3) |
| `Set(a)` | Persistent set built on `Dict` |
| `Vector(a)` | Persistent indexed vector (RRB tree), O(log32 n) index/update/append |
| `Pid` | Opaque process identifier |
| `Subject(msg)` | Typed mailbox address (see §9) |

### 4.2 Custom types
```lush
// Sum type (algebraic data type)
pub type Shape {
  Circle(radius: Float)
  Rectangle(width: Float, height: Float)
  Point
}

// Single-variant record
pub type User {
  User(name: String, age: Int)
}

// Generic
pub type Tree(a) {
  Leaf
  Node(left: Tree(a), value: a, right: Tree(a))
}

// Opaque: constructors hidden outside this module
pub opaque type Email {
  Email(String)
}

// Alias
pub type Name = String
```

- Record fields are accessed with `.`: `user.name`. Field access is only allowed when every variant of the type has that field with the same type.
- Record update: `User(..user, age: 31)`.
- Constructors with fields are functions and can be passed around; nullary constructors are values. Constructor names are unique within a module's value namespace.
- ADTs are nominal; aliases are transparent and cannot be cyclic. Recursive ADTs are allowed. Opaque constructors, field access, and record updates are inaccessible outside the defining module; equality capability follows §4.3.

### 4.3 Type inference
- Hindley-Milner inference with a **syntactic value restriction** and sealed built-in constraints. Top-level functions MAY omit annotations; the formatter does not add them. Public functions SHOULD be annotated (the compiler warns for unannotated `pub fn`).
- Generalize only type variables not free in the environment, and only for non-expansive right-hand sides: literals, identifiers, function expressions, and tuple/list/ADT construction whose components are all non-expansive. Ordinary calls, record updates, field access, blocks, `case`, and all effectful expressions are expansive. Constructor applications are recognized after name resolution. Desugar pipes, captures, and `use` before this check. Expansive bindings retain shared, nongeneralized unification variables; annotations cannot force their generalization. This conservative rule also applies to pattern bindings and values containing subjects, tasks, or selectors. Unresolved nongeneralized variables may not escape a module interface.
- For example, `let s = process.new_subject();` creates one monomorphic subject: sending an `Int` fixes its message type to `Int`, and subsequently sending a `String` is a compile error. In contrast, `fn() { process.new_subject(); }` may be generalized: each invocation creates a fresh subject. Recursive function groups are inferred monomorphically within the group and generalized afterward; polymorphic recursion is not supported.
- No subtyping, implicit coercion, user-defined overloading, or user-defined traits in v1. The compiler alone implements the sealed constraints `Eq(a)` and `Neg(a)`. These are carried in inferred type schemes and module interfaces; diagnostics display them as `where Eq(a)` or `where Neg(a)`. They need no source declaration syntax: annotations constrain ordinary types but do not erase inferred constraints. Instantiation propagates constraints and rejects unsupported concrete types; unresolved constraints on generalized variables remain polymorphic, with no numeric defaulting.
- `==` and `!=` require identical operand types and `Eq(a)`. Scalar data, `String`, `BitArray`, `Pid`, `Subject(a)`, `Monitor`, and `Timer` support equality; runtime identities compare by unique identity, independent of payload type. Functions, selectors, and tasks do not. Tuples, lists, vectors, sets, dictionaries, and ADTs support equality only when all stored component types do; dictionary keys and set elements require `Eq` at construction/lookup. Every `Eq` type has compiler-provided consistent hashing; user implementations are forbidden. Recursive ADT capability checking uses the least requirements on its type parameters. Opaque types export compiler-computed equality requirements without revealing representation; changing these requirements is an interface change.
- Equality is structural, with nominal constructor identity; maps and sets compare independent of insertion order, strings compare UTF-8 bytes without Unicode normalization, and bit arrays compare bit length and contents. Float equality is numeric, so `-0.0 == 0.0`, and their hashes MUST agree. Hash values and iteration order are not stable across executions and are not a serialization format.
- `Neg(a)` is satisfied only by `Int` and `Float`; generic negation dispatches on their runtime tags. Binary arithmetic operators remain split by type (§5.7). Type-specific helpers live in modules (`int.to_string`, `float.to_string`, `string.length`).
- Unused values, variables, imports, and unreachable patterns MUST produce warnings. Non-exhaustive `case` MUST be a compile error.

---

## 5. Expressions and syntax

Expressions produce values; declarations and bindings do not. After `use` desugaring (§5.5), a block `{ ... }` evaluates to its last statement if that statement is an expression (which ends in `;`); otherwise it evaluates to `Nil`. Earlier expression statements discard their values. Local function definitions do not supply a block result.

### 5.1 Bindings
```lush
let x = 5;
let #(a, b) = pair;            // irrefutable pattern
let assert Ok(v) = parse(s);   // refutable: panics on mismatch
let assert Ok(v) = parse(s) as "config must parse";   // custom panic message
const max_size = 1024;         // module-level constant, compile-time evaluable
```
Shadowing is allowed. There is no mutation.

### 5.2 Functions
```lush
pub fn add(a: Int, b: Int) -> Int {
  a + b;
}

// Labelled arguments (external label, internal name)
pub fn replace(in string: String, each pattern: String, with replacement: String) -> String {
  ...;
}
replace(in: "a,b", each: ",", with: " ");

// Anonymous functions and captures
let inc = fn(x) { x + 1; };
let add_five = add(5, _);     // function capture

// Higher-order
list.map([1, 2, 3], fn(x) { x * 2; });
```
- Functions are first-class closures. Closures capture by value (everything is immutable).
- Recursion works for top-level and local named functions. Local definitions use `fn name(...) { ... }` without a trailing semicolon. A maximal consecutive group of local function definitions is mutually recursive; its names are in scope in that group and the subsequent block remainder, but not earlier statements. Captures resolve against bindings preceding the group. Top-level functions in a module form mutually visible recursive groups by dependency. Anonymous functions cannot self-reference.
- **Proper tail calls are MANDATORY.** Any call in tail position MUST NOT grow the stack, including mutual recursion and calls through closures.
- Function bodies are blocks. Return is the final expression statement (terminated by `;`); there is no `return` keyword.
- A single-name parameter has that name as its optional external label; two names specify external label then internal binding. Calls may pass parameters positionally or by their declared labels, each exactly once; positional arguments precede labelled arguments and fill the first unfilled positions. Labels belong to statically resolved named functions/constructors, not structural function types; calls through ordinary function values are positional only. An argument's source order, not parameter order, determines evaluation order.

### 5.3 Pipes
```lush
"  Hello "
|> string.trim
|> string.uppercase
|> string.append(_, "!");
```
`a |> f` is `f(a)`; `a |> f(b)` is `f(a, b)` (first-argument insertion), unless `_` appears, then `a` goes there.

### 5.4 Case and pattern matching
```lush
case shape {
  Circle(r) -> 3.14 *. r *. r;
  Rectangle(w, h) -> w *. h;
  Point -> 0.0;
};
```
Supported patterns:
- Literals (`1`, `2.5`, `"hi"`), variables, discard `_`, `_name`
- Constructors, with positional or labelled fields: `User(name: n, ..)`
- Tuples `#(a, b)`; lists `[]`, `[x]`, `[first, ..rest]`
- String prefix: `"hello " <> name`
- Bit array segments: `<<0x1, n:size(8), rest:bytes>>`
- Alternatives: `1 | 2 | 3 -> ...` (each alternative binds the same variables)
- Alias: `Ok(x) as result`
- Guards: `n if n > 0 -> ...` (guards are limited to pure comparison/boolean expressions on bound variables)
- Multiple subjects: `case a, b { 1, 2 -> ...; _, _ -> ...; }`

Whitespace does not separate arms: each branch expression is terminated by `;`, including the last arm. For a branch block, write `pattern -> { ... };`; when the whole `case` is a statement, its closing `}` takes its own statement-ending `;`.

The compiler MUST check exhaustiveness and redundancy. A Maranget usefulness algorithm and decision-tree compilation are recommended, not required representations. Guarded arms do not establish exhaustiveness. Literal domains other than `Bool` and `Nil` require a covering wildcard/variable or another provably total pattern. String-prefix and bit-array patterns may conservatively require a fallback; the compiler MUST NOT claim an incomplete match is exhaustive.

### 5.5 `use` expressions (callback flattening)
```lush
use file <- result.try(fs.read("a.txt"));
use n <- result.try(int.parse(file));
Ok(n * 2);
```
Desugars to `result.try(fs.read("a.txt"), fn(file) { ... rest of block ... })`. Before type inference and block-result analysis, replace `use p1, p2 <- f(args); rest` with the final expression statement `f(args, fn(p1, p2) { rest });`. This is a semantic rewrite, not reparsed source: the implicit callback fills the callee's final parameter during argument resolution, even when explicit arguments are labelled. That parameter must not already be supplied. Evaluate the callee and explicit arguments in source order, then create the callback; ordinary source calls still require positional arguments before labelled arguments. The enclosing block returns the outer call's result, not the callback's result independently. Multiple patterns become callback parameters in order and MUST be irrefutable; an empty pattern list creates a zero-argument callback. The right-hand side MUST be a call or a function reference (treated as a call with no explicit arguments). A final `use` is valid and supplies an empty callback body returning `Nil`, subject to type checking. Desugaring is recursive, consumes only the current block, and preserves tail position for the outer call and for the callback's final expression.

### 5.6 Blocks, `if`, and control
- There is **no `if`/`else` in v1 beyond `case`**; use `case condition { True -> ...; False -> ...; }`. **[Decision:** keeping the core to one branching construct simplifies the compiler; `if` sugar may be added later.**]**
- `todo`, `todo as "msg"`: type-checks as any type, panics when reached. Compiler warns.
- `panic`, `panic as "msg"`: crashes the current process.
- `assert expr`, `assert expr as "msg"`: panics if `False`.
- `echo expr`: debug print, returns the value.

### 5.7 Operators (highest to lowest precedence)
| Prec | Operator | Meaning |
|---|---|---|
| 12 | `.` | field access |
| 11 | `f(x)` | call |
| 10 | `-` `!` (prefix) | negate Int or Float / logical not |
| 9 | `*` `/` `%` | Int multiply, divide, remainder |
| 9 | `*.` `/.` | Float multiply, divide |
| 8 | `+` `-` | Int add, subtract |
| 8 | `+.` `-.` | Float add, subtract |
| 8 | `<>` | String concatenation |
| 7 | `<` `<=` `>` `>=` | Int comparison |
| 7 | `<.` `<=.` `>.` `>=.` | Float comparison |
| 6 | `==` `!=` | structural equality, requires `Eq` (§4.3) |
| 5 | `&&` | short-circuit and |
| 4 | `\|\|` | short-circuit or |
| 1 | `\|>` | pipe (left-assoc) |

- Prefix `-` is type-directed: `Int -> Int` or `Float -> Float` (no other operand type); `-1.5` is valid. Negating the minimum `Int` panics on overflow.
- `/` and `%` on Int **panic on division by zero** (use `int.divide` for a `Result`). Float `/.` by zero also panics.
- Equality on functions or data types storing functions is a compile error (§4.3).
- No user-defined operators.
- List cons/prepend is `[x, ..xs]`.
- Binary operators are left-associative except comparisons and equality, which are non-associative; chained comparisons/equality require parentheses. Prefix operators associate right. Calls and field access form a left-to-right postfix chain (`f(x).field(y)`).

### 5.8 Evaluation and primitive semantics
- Evaluate the callee, then arguments left-to-right in source order, including labelled arguments; evaluate tuple/list/constructor fields and case subjects left-to-right. Evaluate a record-update base before its replacement fields. A pipe evaluates its left operand exactly once before the right-hand callee and explicit arguments. No optimizer may change observable effects, panic ordering, or short-circuit behavior. `&&` and `||` evaluate the right operand only when required.
- A capture such as `add(5, _)` desugars to `fn(x) { add(5, x); }`; callee and non-placeholder expressions are evaluated on invocation, not capture creation. Exactly one direct argument placeholder is allowed in a capture or a piped call; nested placeholders and multiple placeholders are errors. A piped placeholder is replaced by the already-evaluated left operand, not by a callback.
- `Int` division truncates toward zero; remainder has the dividend's sign and satisfies `a = (a / b) * b + a % b` mathematically. Both division and remainder panic on zero divisors and on `MIN_INT / -1` or `MIN_INT % -1`. Literal range errors are compile errors. A directly negated integer literal may have magnitude `2^63` and denotes `MIN_INT`; the same positive literal is invalid. Negating a computed `MIN_INT` panics.
- Float operations use binary64 round-to-nearest, ties-to-even; underflow to subnormal or signed zero is allowed. Overflow and invalid operations panic; parsers/decoders return `Error` for nonfinite inputs and literals outside the finite range are compile errors. No intrinsic may introduce NaN or infinity. Fused arithmetic may not replace separately rounded source operations.
- Constants permit literals, constant references, tuples/lists/ADT construction, and primitive operators on constant operands only. No function calls, effects, closures, subjects, or runtime handles are allowed. Constant-reference cycles and evaluation failures are compile errors. Constant evaluation uses the same numeric semantics as execution.
- `assert` requires `Bool` and returns `Nil`; panic messages require `String`. `panic` and `todo` can inhabit any expected result type because they do not return.

### 5.9 Bit arrays
- Bits are ordered most-significant-bit first within each byte. An unqualified segment is an unsigned 8-bit `Int`. Integer segments support `size(n)`, `signed`/`unsigned`, and `big`/`little`, joined by `-`, for example `<<n:size(16)-signed-little>>`. Default byte order is `big`; little-endian widths MUST be multiples of eight. Widths are 1–64 bits. Values must fit the specified signed/unsigned range; construction panics otherwise. Unsigned 64-bit construction accepts only nonnegative representable `Int` values; a pattern whose unsigned decoded value exceeds `MAX_INT` fails to match rather than producing an invalid `Int`.
- `utf8` segments take `String` and append its UTF-8 bytes; in patterns only literal strings are allowed with `utf8`. `bytes` takes byte-aligned `BitArray`; `bits` takes any `BitArray`. `size(n)` on `bytes` counts bytes, and on `bits` counts bits; construction requires exact source length. Unsized `bytes`/`bits` append the entire value; in patterns they are allowed only as the final remainder segment. Float segments are deferred.
- Pattern sizes may use nonnegative integer literals or variables bound before that segment, without calls or arithmetic. Invalid widths, insufficient input, alignment failures, and out-of-range decoded integers fail the match; statically invalid specifications are compile errors. Construction size/alignment errors panic. Signed decoding uses two's complement. A complete pattern must consume all input; bit arrays track exact bit length and ignore unused storage padding.

### 5.10 Modules and imports
- **One file = one module.** The module path is the file path relative to `src/` (`src/app/user.lush` → `app/user`).
- Everything is private unless marked `pub`.
```lush
import app/user;
import lush/list;
import lush/dict.{type Dict};
import lush/result.{try, map as result_map};
import some/long/module as m;

user.new("ann");            // qualified access
```
- Import cycles are a compile error. A module may be imported both qualified and with a selective type import; each binding must actually be used.
- The **prelude** (always in scope): `Int Float String Bool Nil List Result Option BitArray` types, `Ok Error Some None True False Nil` constructors.

---

## 6. Compilation pipeline

```
source → lexer → parser → AST → name resolution → type inference
       → exhaustiveness check → typed AST → Core IR (ANF, monomorphisation-free)
       → optimisation → bytecode → (bundle with runtime) → executable
```

1. **Lexer** (`logos`), **parser** (hand-written recursive descent + Pratt for expressions). MUST recover from errors and report multiple.
2. **Type checker:** Value-restricted HM with unification, level-based generalisation, and sealed `Eq`/`Neg` constraints (§4.3). Errors MUST be precise, with source spans and suggestions (Elm/Rust quality). Use `ariadne` or `miette`.
3. **Core IR:** A-normal-form, explicit closures, pattern matches compiled to decision trees.
4. **Optimisations (v1, keep small):** inlining of small functions, constant folding, dead code elimination, known-call resolution, tail-call marking. Case-of-known-constructor. No monomorphisation: values are uniformly represented; sealed equality/hash and negation operations dispatch through runtime tags/type descriptors. Generic execution still incurs this runtime dispatch cost.
5. **Bytecode generation:** register-based, per-function register frames.
6. **Incremental compilation:** per-module cache keyed by source content, dependency interface hashes (including sealed constraints), compiler/stdlib versions, target, and compiler flags, stored in `.lush/cache/`.

---

## 7. Runtime

### 7.1 Value representation
All values are 64-bit tagged words.
- Low bits tag: immediate small `Int` (62-bit fast path, promoted to boxed 64-bit when needed), `Bool`/`Nil`/nullary constructors, `Pid`, pointer-to-heap-object.
- Heap objects have a one-word header (kind, arity/length, GC bits).
- Kinds: tuple/constructor (tag + fields), cons cell, boxed `Int`/`Float`, closure (function index + captured values), string/binary (small: inline on heap; large: shared refcounted, see §7.4), Dict/Vector nodes, `Subject`.
- Strings and bit arrays are byte buffers; strings are validated UTF-8 on creation.

### 7.2 Processes
Each process owns:
- A **private heap** (initial ~233 words, grows by Fibonacci-like steps, inspired by BEAM).
- A **growable stack** (segmented or copy-on-grow; starts tiny, ~1 KB).
- A **typed subject mailbox** (MPSC queue of `(subject_ref, value)` messages) and a separate **system signal inbox** (§7.5); safe to push from any scheduler thread.
- Link set, monitor set, trap-exit flag, reduction counter, status (runnable/waiting/exited).

The heap (~1.9 KB) and stack (~1 KB) alone exceed 1 KB. Lush's target is **≤ 4 KB per newly spawned idle process**, including initial heap, stack, mailbox/inbox and bookkeeping (excluding shared runtime tables and the caller's references). A test MUST measure 1,000,000 idle processes with this accounting; resource-constrained machines may use a smaller count.

Processes never share mutable memory. No process can read another's heap.

### 7.3 Scheduler
- `N` scheduler threads (default: number of logical cores; env `LUSH_SCHEDULERS` to override).
- Each has a local run queue; idle schedulers **steal** from others (`crossbeam-deque`).
- **Preemption by reduction counting:** the interpreter charges at least one reduction for each bytecode instruction (including jumps, comparisons and tail calls), not just allocations or calls. The default quantum is **4000** reductions; when exhausted, the process yields and is re-queued at the back. Long native/builtin operations and selective mailbox scans charge proportional work and yield/resume at safe points (§7.6, §9.1). This is Lush's explicit fairness contract, inspired by BEAM reductions, not an assertion that absence of source loops guarantees fairness. Each scheduler thread must reach a safe point within a bounded amount of interpreter/builtin work; no time-based hard preemption is promised.
- `receive`/`receive_signal` with no matching item parks the process (not runnable) until a match arrives or a timeout fires; unmatched items do not spuriously complete the wait.
- Timers use a hierarchical timing wheel owned by the runtime.
- The program exits when the **main process** returns (exit code 0) or crashes (exit code 1, error printed). Other processes are terminated.

### 7.4 Memory and garbage collection
- **Each process is garbage collected independently** with a generational copying collector (Cheney semi-space, young + old generation, like BEAM). GC runs on the process's own scheduler thread when its heap fills. **No other process is ever paused. There is no global stop-the-world phase by construction.**
- Per-process limit: `max_heap_size`, measured in bytes despite its name, defaults to 64 MiB and is configurable VM-wide with `LUSH_MAX_HEAP_SIZE`. Charge live heap objects, stack, receiver-owned queued/in-progress message fragments, subject/signal queue entries, links/monitors, and creator-owned timers (including retained payloads). Charge each distinct shared buffer's full backing allocation once per retaining process, including retention through queued messages, timers, or sub-slices. This is a conservative ownership budget, not physical RSS. Reclaim unreachable objects before declaring live-heap exhaustion; collector reserve/to-space is separately bounded by the collector's maximum copying overhead. Exceeding the budget terminates the charged process with `HeapLimit`, not the VM. Signal/queue admission must reserve budget before allocating; overflow terminates the receiver without needing to enqueue another signal there. Runtime teardown releases all charges.
- **Messages are deep-copied** on the sender's thread into isolated, receiver-owned off-heap fragments; the sender never mutates the receiver's active heap. Reserve receiver budget incrementally while copying, discard partial fragments if either endpoint exits, and publish only complete messages. On receive, adopt or copy the fragment into the receiver's heap without double charging its logical contents. Receiver limit exhaustion kills the receiver; asynchronous `send` still returns `Nil` if the sender survives.
- **Large binaries/strings (> 64 bytes)** live in a shared, atomically refcounted, immutable off-heap region and are sent by reference. Release references when heap references are collected, fragments/timers are discarded, or the retaining process exits. Sub-slices are views onto the parent buffer and retain its full allocation.
- Closures sent as messages copy their captured environment. Function code is global and immutable, so it is never copied.
- The runtime's own global structures (process table, run queues, timers) use lock-free or fine-grained locking. No global lock on the hot path.

### 7.5 Fault tolerance primitives
- **Panic:** terminates only the current process with `Abnormal(Crash(message, trace))`. Public `process.ExitReason` is `Normal | Killed | NoProcess | HeapLimit | Abnormal(Crash)`; `Crash` contains a message and stack trace. A returned process exits `Normal`. `NoProcess` reports an absent target and is not a user-requested exit reason.
- **Link:** bidirectional. Linking an absent/dead pid produces a `NoProcess` exit signal for the caller. On linked exit, `Normal` is ignored by a non-trapping peer; `Abnormal`, `HeapLimit`, or `NoProcess` kills a non-trapping peer with that reason. A trapping peer instead receives `Exit(from: Pid, reason: ExitReason)` in its system signal inbox, including `Normal`. `process.kill` unconditionally terminates its target with `Killed`, regardless of the target's trap-exit flag. That exit propagates to linked peers like any other abnormal exit: non-trapping peers terminate with `Killed`, while trapping peers remain alive and receive `Exit(from: target_pid, reason: Killed)` in the system signal inbox. The kill request itself is never delivered as a message; this distinction lets a trapping supervisor restart a killed child.
- **Atomic linking:** `link` either establishes both directions while the target is live or delivers `NoProcess`; a concurrent exit cannot fall between these outcomes without notification. Repeated links are idempotent; self-link/unlink are no-ops. `spawn_link` installs both directions before the child can execute, so even an immediate child crash propagates. `unlink` removes both directions atomically but does not retract an exit already committed to delivery. `HeapLimit` propagates like `Killed`.
- **Monitor:** unidirectional. On target exit, the monitoring process receives exactly one `Down(monitor: Monitor, pid: Pid, reason: ExitReason)` in its **system signal inbox**, including for `Normal`. Monitoring an already-dead or nonexistent pid queues `Down(ref, pid, NoProcess)` immediately; the returned ref matches the event. Monitors do not propagate failure. `demonitor(ref, flush: True)` atomically disables the caller's monitor and removes its queued `Down`; after return no `Down` for that ref can arrive, even if target exit raced with cleanup. With `flush: False`, an already-committed `Down` may still arrive. Repeated demonitor is harmless; using another process's monitor panics. Monitor handles remain owner-bound even after target exit.
- **Typed delivery:** system signals have type `process.Signal` (`Down | Exit`) and are runtime-generated, not `msg` values. They never enter a user `Subject(msg)` mailbox. `process.receive_signal` and `process.receive_signal_forever` read the separate signal inbox, whether or not exit trapping is enabled; only trapped exit signals become `Exit`. This preserves typed-subject safety while making lifecycle events observable. `process.trap_exits(True)` takes effect before it returns.
- **Exit API:** `process.exit(pid, reason)` sends a link-like exit signal without creating a link: `Normal` is ignored by a non-trapping target but delivered as `Exit(sender, Normal)` to a trapping target; `Abnormal` terminates a non-trapping target, or is delivered as `Exit` if trapping. `exit` accepts only `ExitNormal` or `ExitAbnormal(message)` (the runtime captures the trace in `Abnormal(Crash(...))` on delivery); `Killed` is reserved for `process.kill`, and `NoProcess` cannot be sent. `process.kill(pid)` terminates the target even when trapping; killing/exiting an already-dead pid is a no-op. Self-directed abnormal exit terminates the caller; self-directed normal exit terminates the caller normally.
- **Ordering:** messages and signals from one sender to one receiver are enqueued in send order (including exit/monitor notifications after earlier sends by that sender). Different senders have no total order. Signal inbox and subject mailbox remain separately selectable; observing one does not consume the other. These are Lush decisions inspired by BEAM links/monitors, not claims of BEAM API or storage equivalence.

### 7.6 I/O model
- User code sees **synchronous-looking** I/O. Under the hood:
  - **Network and timers:** non-blocking, via a central reactor (`mio`). A process performing a socket read is parked and woken by the reactor.
  - **Filesystem and subprocess:** run on a separate **blocking thread pool** (like BEAM dirty schedulers). The calling process parks; scheduler threads are never blocked.
- A scheduler thread MUST NEVER block on I/O.
- No builtin may monopolize a scheduler: sorting, hashing, copying messages, GC, and selective scans MUST use bounded chunks and charge reductions or park/resume. If an operation cannot safely yield mid-step, its maximum step size must be bounded; operations that require blocking run in the blocking pool. A process's GC pauses that process only; large collections must not monopolize its scheduler thread or delay other processes for an unbounded interval. Copy/scan work, including a single large object, is resumable in chunks of at most 1024 words or queue entries, charged at least one reduction per word/entry; yield at a chunk boundary when the quantum expires. GC may keep its process unavailable while its collection continuation is re-queued, allowing other processes to run. These are work bounds, not wall-clock latency guarantees.

---

## 8. Bytecode VM (implementation guidance)

MAY be changed freely by the implementer, since it is not part of the language contract. Suggested design:
- Register machine, per-frame fixed register count (max 256).
- Instruction sizes: fixed 32-bit or 64-bit words, opcode + operands.
- Core ops: `Move, LoadConst, LoadInt, Add/Sub/Mul… (checked), Cmp*, Jump, JumpIfFalse, SwitchTag (from decision trees), MakeTuple, MakeCons, MakeClosure, GetField, Call, TailCall, CallClosure, TailCallClosure, Return, Spawn, Send, Receive, Panic`. Every executed instruction charges reductions; a `Reduce` opcode alone is insufficient for fairness.
- `TailCall` reuses the current frame.
- Module bytecode is serialised into a `.lushc` file per module and bundled at build.
- A `--dump-bytecode` flag on `lush build` MUST exist for debugging.

---

## 9. Concurrency API and message passing

### 9.1 Typed mailboxes
Lush uses **typed** message passing. A `Subject(msg)` is a typed address: only values of type `msg` can be sent to it.

```lush
import lush/process.{type Subject};

pub type Msg {
  Increment
  Get(reply: Subject(Int))
}
```

- `process.new_subject() -> Subject(msg)` creates a subject **owned by the calling process**. Any process holding it can send or query its immutable owner pid via `subject_owner`, including after owner exit or subject closure. Receiving, closing, adding a subject to a selector, or waiting with that selector from a non-owner process panics at runtime; ownership is not encoded in the type. Selectors are bound to their creating process. Receiving/selecting a closed subject also panics. `close_subject` is owner-only and idempotent: it atomically discards queued messages and rejects all later sends, including racing timer deliveries.
- Subjects are first-class, copyable values, and safe to embed in messages (that's how request/reply works).
- A process has one physical **user mailbox**. Each message is tagged with its target subject's unique reference. `receive` on a subject does a **selective receive**: it scans the mailbox for the first message with that tag, leaving others in place. The scan MUST be resumable and charge reductions even when no message matches; arrival during a parked scan wakes it. System signals use a separate inbox (§7.5).
- `Selector(a)` lets a process wait on several subjects at once, mapping each to a common type:
```lush
let sel =
  process.new_selector()
  |> process.select_subject(work_subject)
  |> process.select_map(timer_subject, TimerFired);
process.select(sel, within: 5000);   // Result(a, Nil)
```
- Selector builders return a new immutable selector. `select_subject` adds a subject of the selector's result type; `select_map` adds a subject with a mapping callback (ordinary user code, allowed to have effects or panic). Duplicate subject references in one selector panic; an empty selector is valid and waits until timeout. Selection consumes the earliest mailbox message among all registered subjects, independent of builder order, then runs only its mapper in the receiving process. A mapper panic consumes that message and terminates the process. All ownership/closed-subject checks occur before consuming anything. Mapping is not a predicate and cannot reject a message.
- Subjects closed explicitly or owned by an exited process reject subsequent sends silently (asynchronous send returns `Nil`); this does not acknowledge delivery. Use a monitor or `actor.call` when failure must be observed. Because types are checked statically, a process can never receive a message it doesn't understand: lifecycle notifications are a separate, statically typed `Signal` channel.

### 9.2 Core `lush/process` API
```lush
pub type Crash { Crash(message: String, trace: List(String)) }
pub type ExitReason { Normal Killed NoProcess HeapLimit Abnormal(Crash) }
pub type Signal { Down(monitor: Monitor, pid: Pid, reason: ExitReason) Exit(from: Pid, reason: ExitReason) }
pub type ExitRequest { ExitNormal ExitAbnormal(String) }
pub type SpawnError { SpawnFailed }
// API pseudocode: signatures only, not valid Lush definitions (fn needs a body).
pub fn spawn(fn() -> Nil) -> Pid;
pub fn try_spawn(fn() -> Nil) -> Result(Pid, SpawnError);
pub fn spawn_link(fn() -> Nil) -> Pid;
pub fn self() -> Pid;
pub fn new_subject() -> Subject(msg);
pub fn subject_owner(Subject(msg)) -> Pid;
pub fn close_subject(Subject(msg)) -> Nil;
pub fn new_selector() -> Selector(a);
pub fn select_subject(Selector(a), Subject(a)) -> Selector(a);
pub fn select_map(Selector(a), Subject(msg), fn(msg) -> a) -> Selector(a);
pub fn select(Selector(a), within: Int) -> Result(a, Nil);
pub fn send(Subject(msg), msg) -> Nil;
pub fn receive(Subject(msg), within: Int) -> Result(msg, Nil); // ms; Error(Nil) on timeout
pub fn receive_forever(Subject(msg)) -> msg;
pub fn receive_signal(within: Int) -> Result(Signal, Nil);
pub fn receive_signal_forever() -> Signal;
pub fn sleep(Int) -> Nil;
pub fn send_after(Subject(msg), after: Int, msg) -> Timer;
pub fn cancel_timer(Timer) -> Nil;
pub fn monitor(Pid) -> Monitor;
pub fn demonitor(Monitor, flush: Bool) -> Nil;
pub fn link(Pid) -> Nil;
pub fn unlink(Pid) -> Nil;
pub fn trap_exits(Bool) -> Nil;
pub fn exit(Pid, ExitRequest) -> Nil;
pub fn kill(Pid) -> Nil;
pub fn is_alive(Pid) -> Bool;
```
All durations (`within`, `timeout`, `after`, sleep, and shutdown grace) are nonnegative integer milliseconds on a monotonic clock; negative values panic before side effects. Timed APIs require an explicit duration, with no implicit default; `receive_forever` and `receive_signal_forever` have no deadline. A wait uses one absolute deadline, never extended by wakeups or scanning. Runtime admission order decides races: consume the earliest eligible item admitted before the deadline, even if scanning finishes later; otherwise timeout wins. Zero duration polls items already admitted at entry. Runtime admission and timeout commitment are serialized per receiver, with no lost wakeups; separate queues preserve enough admission-order metadata for multi-channel waits. Timer firing is not earlier than its deadline but may be delayed by scheduling.

`send_after` retains its payload under the creating process's budget until cancellation/firing, then transfers delivery accounting to the receiver. Timers are canceled when their creator exits. `cancel_timer` is idempotent and may be called by any holder: if it wins before delivery commitment no message is sent; otherwise the queued message remains. It returns no delivery guarantee and never retracts a message. Wait-internal timers are canceled/released on every completion path.

`spawn` and `spawn_link` panic in the caller on process-capacity exhaustion, creating no child/link. `try_spawn` is the unlinked fallible equivalent and returns `Error(SpawnFailed)` without creating a child. Actor startup and allocation of the supervisor process return their respective `SpawnFailed` errors; capacity failure inside a supervisor child factory must be returned as `ChildStartError`'s `ChildSpawnFailed`, producing `ChildStartFailed`. `is_alive` is only a snapshot, not synchronization. Graceful shutdown uses `exit(pid, ExitAbnormal("shutdown"))`: a trapping target receives `Exit` and must choose to stop; after a bounded grace deadline the controller uses `kill` and observes `Down`. A non-trapping target terminates immediately. `ExitNormal` is not a reliable shutdown request because non-trapping targets ignore it.

Process names / registry (`process.register`) **[v2]**; use supervisors and subjects for discovery in v1.

### 9.3 `lush/actor` (OTP-lite)
The `pub fn` signatures below are API pseudocode, not source definitions; Lush function definitions require bodies (§12).
```lush
pub type Next(state) { Continue(state) Stop(process.ExitRequest) }
pub type StartError { SpawnFailed InitFailed(process.ExitReason) InitTimeout }
pub type CallError { Timeout ServerDown(process.ExitReason) }

pub fn start(
  init: fn() -> state,
  within: Int,
  handler: fn(msg, state) -> Next(state),
) -> Result(Subject(msg), StartError);

pub fn call(Subject(msg), make_msg: fn(Subject(reply)) -> msg, timeout: Int) -> Result(reply, CallError);
```
`actor.start` creates an **unlinked**, temporarily monitored child. The child creates its own message subject and runs `init()` in the child, so initialization may set exit trapping and allocate child-owned resources. Only after initialization succeeds does startup return its subject. `SpawnFailed` means capacity was unavailable and no child/subject escaped; initialization exit gives `InitFailed(reason)`. The explicit `within` deadline covers initialization: timeout kills the child, waits for termination, and returns `InitTimeout`. Startup closes its private handshake subject and flush-demonitors on every path; if the starter exits before handshake completion the runtime kills the provisional child. No linked-start actor API is required in v1: supervisors bridge a successful subject to its pid with `subject_owner` and establish the link. Successful startup is not a guarantee that the child is still alive when the caller next runs.

The actor loops receiving messages, applies `handler`, and `Stop(ExitNormal | ExitAbnormal(message))` terminates with the corresponding exit reason. A handler can explicitly read the signal inbox; trapped signals are never passed as `msg`. Trapping alone does not make the default message loop cooperate with shutdown, so supervisors still enforce grace deadlines.

`actor.call` creates a private reply subject, monitors `subject_owner(server)`, evaluates `make_msg` exactly once, sends exactly one request, and waits for reply, matching `Down`, or timeout. The deadline starts at call entry; callback execution remains normal preemptible code, not forcibly interrupted at the deadline. Reply/Down/timeout use the admission/deadline rule in §9.2: the earliest eligible reply or matching `Down` wins, otherwise `Error(Timeout)`. `Down` gives `Error(ServerDown(reason))`, including `NoProcess` for an already-dead owner. A closed server subject whose owner is alive simply times out. On **every** path, including success, timeout, Down, or callback panic/caller exit, close the private reply subject, discard queued/late extra replies, cancel the wait timer, and disable/flush the monitor race-safely. Process teardown performs this cleanup if the caller dies. Dropping an address alone never closes a subject. Closure does not cancel the server request. The wait and cleanup filter by the private subject and monitor ref, leaving unrelated signals/messages untouched; runtime/stdlib primitives implement them without polling or lost wakeups.

Example:
```lush
import lush/actor;
import lush/process.{type Subject};
import lush/process;

pub type Msg {
  Increment
  Get(Subject(Int))
}

fn handle(msg: Msg, count: Int) -> actor.Next(Int) {
  case msg {
    Increment -> actor.Continue(count + 1);
    Get(reply) -> {
      process.send(reply, count);
      actor.Continue(count);
    };
  };
}

pub fn main() -> Nil {
  let assert Ok(counter) = actor.start(fn() { 0; }, within: 1000, handler: handle);
  process.send(counter, Increment);
  process.send(counter, Increment);
  let assert Ok(n) = actor.call(counter, Get, 1000);
  let _ = echo n;   // 2
}
```

### 9.4 `lush/supervisor`
```lush
// API pseudocode; ChildSpec is a configurable record.
pub type Strategy { OneForOne OneForAll RestForOne }
pub type Restart { Permanent Transient Temporary }
pub type ChildStartError { ChildSpawnFailed Failed(String) }
pub type ChildSpec {
  ChildSpec(
    start: fn() -> Result(Pid, ChildStartError),
    restart: Restart,
    shutdown_ms: Int,
  )
}
pub type StartError { SpawnFailed ChildStartFailed(index: Int, error: ChildStartError) }
pub fn worker(start: fn() -> Result(Pid, ChildStartError)) -> ChildSpec;
pub fn start(
  strategy: Strategy,
  children: List(ChildSpec),
  max_restarts: Int,
  within: Int,
) -> Result(Pid, StartError);
```
- `worker` defaults to `Permanent` and 5000 ms shutdown grace. Direct `ChildSpec` construction configures both. `max_restarts` is nonnegative and `within` is a positive rolling-window duration in milliseconds; both are explicit (no hidden defaults). Invalid configuration panics before spawning. Child identity is its zero-based list index; an empty list is valid.
- `start` creates a trapping supervisor and atomically links it to the caller before it runs. Startup errors roll back and make this provisional supervisor exit `Normal` after reporting `Error`; unrelated caller failures still propagate normally. Start callbacks run sequentially in list order in the supervisor and **must** return promptly with a fresh, unlinked child or an error; they must clean up any provisional child before returning an error. Use bounded `actor.start` for child initialization, map its errors to `ChildStartError`, and return `process.subject_owner(subject)`. The supervisor monitors and atomically links each returned pid before accepting it; absence or an already-observed exit is `ChildStartFailed(index, Failed(...))`. Later exits follow restart rules. An initial returned error stops all accepted children in reverse order before returning `Error`. Child factories MUST use fallible startup (`try_spawn` or `actor.start`) for expected capacity errors. A panic in a factory is a programmer error, not a returned startup error: it crashes the supervisor and propagates through its links; it does not promise orderly rollback. Previously linked non-trapping children terminate, while the forced-death caveat below applies to trapping children. Startup waits/cleanup use private subjects and monitor refs, not an unfiltered signal receive that could discard unrelated events.
- `Permanent` restarts after every exit; `Transient` only after a reason other than `Normal`; `Temporary` never restarts and is removed after exit. A non-restarting exit does not trigger a strategy wave. On a restart-eligible exit, `OneForOne` replaces only that child, `OneForAll` replaces all remaining children, and `RestForOne` replaces that child and later children. Stop affected survivors in reverse list order, then start replacements in original order. `Temporary` children stopped as collateral are removed, not restarted; other collateral children restart regardless of their intentional stop reason. Consume matching lifecycle notifications without treating deliberate shutdowns or stale pids as new failures.
- Count one restart attempt per strategy wave (including a failed wave), not per child. Before attempting a wave, count attempts in the preceding `within` milliseconds using monotonic time; if admitting this attempt would exceed `max_restarts`, shut down all remaining children and exit `Abnormal(Crash("restart intensity exceeded", ...))`. A restart callback failure tears down any replacements already started in that wave and all remaining children, then exits abnormally rather than spinning retries. These exits propagate to the parent. Initial startup does not consume restart intensity.
- All rollback, strategy stops, and supervisor shutdowns are bounded per child: monitor it, send `ExitAbnormal("shutdown")`, wait up to its `shutdown_ms` for matching `Down`, then `kill` if necessary and wait for termination acknowledgment. A zero grace kills immediately. Stop children sequentially in reverse order; flush-demonitor and unlink on completion. The grace bounds cooperative waiting, not total wall-clock scheduler/teardown latency. The supervisor handles a shutdown `Exit` from its parent by stopping children and exiting normally; another parent exit stops children and exits with that reason. During shutdown no children restart. Forcing the supervisor itself with `kill` bypasses this cleanup; linked non-trapping children die, but trapping children require an external controller if orphan prevention is required.
- Supervisors can be nested: a `ChildSpec.start` callback may invoke `supervisor.start` (already linked to the calling supervisor; this is the sole exception to the unlinked-child rule). Parent shutdown grace should allow the nested supervisor to stop its subtree.
- Discovery is explicit in v1. Each actor child-start callback, after successful `actor.start`, sends the new `Subject(msg)` to a designated discovery subject and returns its owner pid; repeat this publication on every restart. Consumers replace their old address and monitor the new owner. Publication is provisional (the child can die before the supervisor links it), not an atomic availability guarantee; stale subjects are never redirected. Callers handle `ServerDown`/timeout and wait for the next publication. A callback that fails after publication must terminate its provisional child.

### 9.5 `lush/task`
```lush
let t = task.async(fn() { expensive(); });
let result = task.await(t, 5000);   // Result(a, TaskError)
```
```lush
// API pseudocode; Task(a) is opaque and bound to its creating process.
pub type TaskError { Timeout Down(process.ExitReason) }
pub fn async(fn() -> a) -> Task(a);
pub fn await(Task(a), within: Int) -> Result(a, TaskError);
pub fn parallel_map(List(a), fn(a) -> b) -> Result(List(b), TaskError);
```
- `async` atomically links and monitors a child before it executes, and creates an owner-only private result subject. Capacity exhaustion panics like `spawn_link`. The child sends one result and exits normally. A task crash **kills a non-trapping caller** through the link, even before `await`; `Result` does not catch linked failure. A caller that enabled exit trapping before `async` can instead observe `Error(Down(reason))`. A normal exit without a result also yields `Down(Normal)`. Awaiting from another process or awaiting the same task twice panics.
- `await` uses the same private-subject/monitor/deadline arbitration and cleanup as `actor.call`; a result admitted before the matching Down and deadline wins. Unlike calls, timeout **cancels the task**: atomically unlink first, kill the child, wait for its termination, then return `Error(Timeout)`. A linked failure already committed before unlink still applies, so non-trapping callers may die in that race. On any completion close/discard the result subject, cancel its wait timer, unlink, and flush-demonitor. Do not consume trapped `Exit` notifications, which remain available through `receive_signal`; they are distinct from the monitor's `Down`. If the task owner exits, the runtime cancels any unfinished tasks and releases their bookkeeping even when the child traps exits. An unawaited task retains its result/bookkeeping until await or owner exit and is charged to the owner.
- `parallel_map` starts linked tasks in input order and collects results in input order with no timeout. Empty input returns `Ok([])`. On the first observed task error it cleans up all uncollected sibling tasks before returning that error: cancel unfinished children and release completed-but-uncollected results, private subjects, monitors, links, timers, and bookkeeping. Capacity panic or caller exit also cleans up every task from partial startup. Linked crashes still kill non-trapping callers. Concurrency is the input length in v1; callers must batch inputs when bounded concurrency is needed.

---

## 10. Standard library (v1 scope)

Written in Lush wherever possible; hot paths as internal VM intrinsics (see §14). All modules under `lush/`. The `lush` package identity and `lush/` import namespace are reserved for the standard library bundled with the toolchain. Local modules, dependencies, and manifest declarations MUST NOT shadow or replace them; naming a module `lush/*` never grants intrinsic access.

| Module | Contents |
|---|---|
| `bool`, `int`, `float`, `string`, `bit_array`, `order`, `nil` | Basics, parsing, conversions, math |
| `list`, `dict`, `set`, `vector`, `queue`, `pair` | Collections |
| `option`, `result`, `function` | Combinators |
| `io` | `print`, `println`, `eprintln`, stdin reading |
| `fs` | Files, directories, metadata, temp files |
| `path` | Path manipulation |
| `os` | `args`, env vars, `exec` (subprocess), exit |
| `time` | Monotonic clock, wall clock, durations, timezone-free timestamps |
| `random` | Random numbers (seedable PRNG + OS entropy) |
| `net/tcp`, `net/udp` | Sockets |
| `net/http` | HTTP/1.1 client and server (HTTP/2 **[v2]**), TLS via built-in `rustls` in the runtime |
| `json`, `toml` | Encode/decode with decoders (`decode.field(...)` style, type-safe) |
| `crypto` | Hashes (SHA-2, BLAKE3), HMAC, secure random |
| `base64`, `hex`, `url`, `uri` | Encoding |
| `regexp` | Regular expressions (linear-time engine, no backrefs) |
| `process`, `actor`, `supervisor`, `task` | Concurrency (§9) |
| `log` | Structured logging |
| `testing` | Assertions and test runner support |

Stdlib rule: **no function may exist that the language cannot express or the VM cannot provide natively.** If users need a capability, it goes into the stdlib, not behind FFI.

---

## 11. Toolchain

One binary: `lush`. Every workflow is a subcommand.

```
lush new <name>        create a project
lush build             compile to a single executable at ./build/<name>
lush run [args]        build (cached) and run
lush check             typecheck only, fast
lush test [pattern]    run tests
lush fmt [--check]     format (no configuration)
lush add <path>[@ver]  add a dependency
lush remove <path>     remove a dependency
lush update [path]     update dependencies
lush tidy              remove unused deps, verify dependency checksums
lush doc               generate HTML docs from /// comments
lush lsp               language server (stdio)
lush observe           live process inspector for a running program **[v1.1]**
lush version
```

### 11.1 Project layout
```
myapp/
  lush.mod            manifest
  lush.sum            checksums (generated, committed)
  src/
    main.lush         entry: pub fn main() -> Nil
    app/user.lush
    app/user_test.lush
  .lush/              cache (gitignored)
  build/              output (gitignored)
```

### 11.2 `lush.mod`
```
module github.com/alice/myapp

lush 0.1

require (
  github.com/bob/httpkit v1.4.2
  github.com/carol/jsonx v0.9.0
)
```
- Package identity = import path = repo URL, as in Go, except for the reserved bundled `lush` package (§10). No central registry in v1. Git resolution is implemented in-process (`gix`/`git2`-equivalent), without requiring an external `git` executable. HTTPS source archives MAY be another transport, but MUST yield the same canonical source tree and checksum.
- Versions are immutable git tags in semver form (`v1.4.2`); the same module path must have the same content at a given version or verification fails. Major version ≥ 2 changes the import path (`.../httpkit/v2`), as in Go. A module's declared path must match its required path; two distinct versions of one module path cannot coexist in one build.
- **Minimal Version Selection (Go-style MVS):** each `require` names one minimum version of a module path, **not a version range or arbitrary constraint**. Starting at the root, traverse the full requirement graph (including requirements of older reachable versions), selecting the **highest required version per module path** from all reachable versions. Re-traverse when new requirements are discovered until the build list is stable; do not discard requirements merely because a higher version of the same path was selected. Versions compare by SemVer (including prerelease ordering); `v0` and `v1` are each distinct versions of their module path, while `v2+` uses the major-suffixed path above. A dependency can raise, never lower, another's selected version. No SAT solving or backtracking. For example, root requires `a v1.2.0`, `b v1.0.0`; if `b v1.0.0` requires `a v1.4.0`, select `a v1.4.0`. `lush add`/`lush update` explicitly raise minima; `lush tidy` prunes unused root requirements and recomputes the build list. `lush.sum` verifies fetched content and is not a version-selection lockfile.
- `lush.sum` is a checksum ledger, **not a lockfile**. Each record binds a module path and version to a versioned SHA-256 canonical-tree digest. Verify every fetched or cached tree used for compilation or requirement-graph traversal, including manifests of older reachable versions. A mismatch MUST fail without silently replacing the recorded hash.
- Canonical tree hash format v1: SHA-256 over the bytes `lush-source-tree-v1\n`, followed by one record per regular file sorted by unsigned UTF-8 path bytes. A record is a little-endian u64 path-byte length, the relative path bytes, a little-endian u64 content-byte length, and the exact content bytes. Include `lush.mod` and all distributed files, not merely `.lush` files; do not normalize contents or line endings. Ignore directory entries, timestamps, owners, permissions, compression, and archive ordering. Git transport exports the selected commit tree without repository metadata; archives MUST represent that same tree (a transport-declared single root directory may be stripped before validation).
- Before extraction or hashing, reject symlinks, hard links, submodules, devices and other non-regular entries; absolute paths, drive prefixes, backslashes, empty/`.`/`..` path components, invalid UTF-8, duplicate paths, file/directory conflicts, and paths that collide under a supported host filesystem's case/Unicode rules. Never follow filesystem links or write outside the extraction root. Reject `.git` metadata paths. Enforce documented limits on archive bytes, expanded bytes, file count, and path length before allocating/extracting; apply equivalent validation to Git trees and cache entries.
- First acquisition via explicit dependency-management commands (`add`, `update`, `tidy`) may record missing sums: this is **trust on first use (TOFU)**, not proof of publisher authenticity. Tags can move upstream; recorded sums detect subsequent content changes, not a malicious first download. Build/check/run/test and strict CI MUST fail on missing sums or mismatches and MUST NOT modify `lush.mod` or `lush.sum`. Dependency commands MUST report newly trusted records for review. Strict CI uses committed manifests/sums; offline mode for dependency-consuming commands MUST perform no network access and fail clearly if any verified graph input is absent.
- Downloaded dependencies are stored in a global read-only cache (`~/.lush/pkg/`); read-only status alone is not verification. `lush vendor` copies into `./vendor/` **[v1.1]**. `lush tidy` prunes unused root requirements, recomputes the graph, verifies existing checksums, and records explicitly acquired missing checksums; it does not create a version-selection lockfile.
- Dependencies supply Lush source and inert package data only: no native modules or build/install scripts are executed. This narrows the audit surface; it does not make dependencies trustworthy, automatically audited, or builds automatically reproducible. Runtime Lush code can still use filesystem, network, and subprocess APIs.
- Reproducible builds require identical root source/data, complete resolved dependency graph and verified trees, toolchain/stdlib/runtime-stub versions and digests, target, compiler flags, and all other declared build inputs. The build MUST avoid undeclared environment, timestamps, random identifiers, absolute build paths, and host-dependent file ordering in output. Two clean offline builds with those inputs MUST produce byte-identical executables, including deterministic ad-hoc signing (§11.3); caches must not affect output. This is build reproducibility, not deterministic program execution or independent verification of the toolchain's provenance.

### 11.3 Building a self-contained executable
**Self-contained does not mean universally static.** It means one executable containing the runtime, stdlib, and application bytecode, with no separately installed Lush runtime or language toolchain. The following platform contract governs packaging (including earlier shorthand references to a “static executable”):

| v1 platform | Architecture | System dependencies |
|---|---|---|
| Linux | x86_64, aarch64 | musl-linked static runtime stub; supported Linux kernel, no dynamic libc requirement |
| macOS | arm64, x86_64 | System-provided libraries/frameworks and loader; fully static macOS executables are not promised |

Releases MUST publish and test minimum kernel/macOS versions and CPU baselines. v1 builds only for the host OS/architecture using a bundled matching stub; other targets are rejected. Windows x86_64 is not supported in v1. OS services, certificates/configuration, application data, and programs invoked through `os.exec` may still be deployment requirements.

1. Compile all modules (including dependencies) to bytecode. Select the runtime stub embedded in the toolchain release, with an exact target, stub digest, runtime ABI version, bytecode format version, and stdlib/intrinsic ABI version. Reject mismatches; do not silently download or substitute a stub during a build.
2. Package the bytecode and metadata into a versioned bundle. Its header/footer MUST identify format, target, required ABI versions, section offsets/lengths, and a SHA-256 digest covering the bundle metadata and payload (excluding the digest field itself). The loader MUST check magic, supported versions, compatibility, integrity, non-overlapping sections, file bounds, integer-overflow-safe offset arithmetic, and documented size/allocation limits before loading. Missing, truncated, corrupt, or incompatible bundles fail with a diagnostic before application execution. Checksums detect corruption, not authenticity against an attacker who can replace the executable.
3. Use the platform-specific bundler to embed the bundle in a copy of the stub. An appended bundle/footer is permitted where the executable format supports it; arbitrary appending is not a portable signing strategy. On macOS the bundler MUST finalize Mach-O layout, embedded bundle location, and signature metadata, then apply a deterministic **ad-hoc signature with the signer built into the toolchain**, covering the final bundle. Signature data must not invalidate bundle location/bounds. No external `codesign`, linker, C compiler, SDK, or developer certificate is required for ordinary `lush build`. Ad-hoc signing is not publisher authentication or notarization; Developer ID signing/notarization for distribution is a separate optional workflow with its own credentials/tools.
4. On startup, locate and validate the bundle, then validate bytecode before executing `main`: legal opcodes/operand encodings, instruction-aligned control-flow targets, register/constant/function indices, arities, closure layouts, and type/representation invariants required for safe execution. Intrinsic references MUST match the bundled stdlib's authorized interface (§14). Malformed input must fail safely, not panic the loader or bypass memory checks; a checksum is not a bytecode verifier. Verification plus runtime checks MUST preserve VM safety even for adversarial bytecode; this is not a sandbox for its filesystem/network effects.

Cross-target builds are **[v1.1]**, not “trivial cross-compilation”: release-provided target stubs, target-specific packaging/signing, compatibility checks, and tests on the destination OS/architecture are required. The v1 host-only bundler establishes the format and validation contract first. Native AOT bundles remain **[v2]**.

### 11.4 Testing
- Files named `*_test.lush` are test modules. Functions named `test_*` with type `fn() -> Nil` are tests. A panic is a failure.
- Each test runs in its own process, in parallel, with a timeout (default 10 s).
- `lush/testing` provides `assert_equal(got, expected)`, `assert_ok`, `assert_error`, with diffed output.
- Test files are excluded from `lush build`.
- Documentation CI MUST extract and syntax-check/type-check every complete Lush example in this specification and generated API docs against the bundled stdlib. Complete modules compile unchanged; expression/statement snippets get explicit fixture wrappers supplying imports, bindings, types, and an enclosing function. Wrappers MUST NOT silently repair example tokens. Maintain an inventory mapping each fence to a module fixture, wrapped snippet, expected-error fixture, or explicitly labeled API pseudocode/illustrative placeholder; no unclassified fences or silent skips. Signature-only API pseudocode is excluded from source compilation, but its declared API must be checked against the implementation separately. Non-Lush grammar, shell, manifest, and directory-layout fences are not Lush source.
- Run executable examples with expected outputs and bounded harness timeouts; syntax/type checking alone does not run intentionally long-lived programs. The supervised worker example is terminated by its harness. Documentation fixture failures are release blockers, including syntax/API drift in earlier sections.

### 11.5 Formatter
`lush fmt` is opinionated, has zero options, is idempotent, and is the only accepted style. 2-space indent, 80-column soft limit; it emits required statement/import/const semicolons (§3). The formatter MUST preserve comments.

### 11.6 Diagnostics
Errors MUST include a code, a source snippet with a label, and a hint where feasible. Runtime panics MUST print: message, source location, and a stack trace (including the process's spawn location). Crash reports MUST include linked-process context.

---

## 12. Formal grammar (abridged EBNF)

```ebnf
module        = { import | definition } ;
import        = "import" path [ "." "{" import_item { "," import_item } [ "," ] "}" ] [ "as" ident ] ";" ;
import_item   = [ "type" ] ( ident | UIdent ) [ "as" ( ident | UIdent ) ] ;
definition    = [ "pub" ] ( fn_def | type_def | const_def ) ;
fn_def        = "fn" ident "(" [ params ] ")" [ "->" type ] block ;
params        = param { "," param } [ "," ] ;
param         = [ ident ] ident [ ":" type ] ;
type_def      = [ "opaque" ] "type" UIdent [ "(" tvars ")" ] ( "{" variant { variant } "}" | "=" type ) ;
tvars         = ident { "," ident } [ "," ] ;
variant       = UIdent [ "(" field { "," field } [ "," ] ")" ] ;
field         = [ ident ":" ] type ;
const_def     = "const" ident [ ":" type ] "=" expr ";" ;

block         = "{" { statement } "}" ;
statement     = fn_def | ( let_stmt | use_stmt | expr ) ";" ;
let_stmt      = "let" pattern [ ":" type ] "=" expr
              | "let" "assert" pattern [ ":" type ] "=" expr [ "as" string ] ;
use_stmt      = "use" [ pattern { "," pattern } ] "<-" expr ;

expr          = literal | ident | constructor | "(" expr ")"
              | expr binop expr | unop expr
              | expr "|>" expr | expr "(" [ args ] ")" | expr "." ( ident | UIdent )
              | "fn" "(" [ params ] ")" [ "->" type ] block
              | "case" expr { "," expr } "{" clause { clause } "}"
              | "todo" [ "as" string ] | "panic" [ "as" string ]
              | "assert" expr [ "as" string ] | "echo" expr
              | block | "[" [ list_items ] "]"
              | "#(" expr "," expr { "," expr } [ "," ] ")"
              | "<<" [ segments ] ">>"
              | constructor "(" ".." expr { "," ident ":" expr } [ "," ] ")" ;
args          = arg { "," arg } [ "," ] ;
arg           = [ ident ":" ] ( expr | "_" ) ;
list_items    = ".." expr [ "," ]
              | expr { "," expr } [ "," ".." expr ] [ "," ] ;
clause        = pattern_row { "|" pattern_row } [ "if" guard ] "->" expr ";" ;
pattern_row   = pattern { "," pattern } ;
type          = type_name [ "(" type { "," type } [ "," ] ")" ] | ident
              | "fn" "(" [ type { "," type } [ "," ] ] ")" "->" type
              | "#(" type "," type { "," type } [ "," ] ")" ;
type_name     = UIdent | ident "." UIdent ;
```
This grammar abbreviates lexical tokens, import `path`, constructors, patterns, guards, and bit segments, whose normative rules are in §§3–5. A path is slash-separated module components; dependency imports may begin with a repository domain such as `github.com`, and resolution uses the longest matching declared dependency prefix. Qualified expression/type names use the imported module binding, not slash paths. Constructor references may be qualified. Precedence and associativity are fixed by §5.7; this expression grammar is not an ambiguity-resolution rule. Each pattern row must match the number of case subjects; alternatives bind the same variables with identical types. Trailing commas are accepted in comma-delimited lists, not as extra case subjects or pattern rows.

After `use` rewriting, the final `expr ";"` in a block supplies its value; otherwise the block yields `Nil`. Every `case` clause has a mandatory terminating `;`, including a block branch (`pattern -> { ... };`). Whitespace cannot delimit clauses. Implementations MAY refine grammar presentation but MUST NOT change accepted syntax, precedence, or delimiters specified here or in normative examples.

| Conformance feature | Accept | Reject |
|---|---|---|
| Multiple subjects | `case a, b { 1, 2 -> True; _, _ -> False; }` | An arm with only one pattern |
| List spread | `[x, ..xs]`, `[..xs]` | `[x ..xs]`, `[ ..xs, x ]` |
| Tuple arity | `#(a, b,)` | `#()`, `#(a)` |
| Local function | `{ fn id(x) { x; } id(1); }` | A bodyless local signature |
| Qualified type | `actor.Next(Int)` | Slash-qualified type expressions |
| Comparison | `(a < b) == True` | `a < b < c` |
| Trailing commas | `f(a,)`, `fn(a,) { a; }` | A comma in place of a case-arm semicolon |
| Record update | `User(..user, age: 31)` | Updating an inaccessible opaque constructor |

Every row MUST have parser/typechecker fixtures, in addition to the example inventory in §11.4.

---

## 13. Example programs

**Fibonacci with tail recursion**
```lush
import lush/io;
import lush/int;

fn fib(n: Int, a: Int, b: Int) -> Int {
  case n {
    0 -> a;
    _ -> fib(n - 1, b, a + b);
  };
}

pub fn main() -> Nil {
  io.println("fib(50) = " <> int.to_string(fib(50, 0, 1)));
}
```

**A million processes**

This is a spawn/message aggregation workload, not a measurement of simultaneously idle processes. Run on a suitably provisioned host; the benchmark harness MUST set and report `LUSH_MAX_HEAP_SIZE` high enough for the parent's accumulated mailbox (§7.4), otherwise `HeapLimit` is a valid outcome under the default 64 MiB budget.

```lush
import lush/process;
import lush/process.{type Subject};
import lush/list;

pub fn main() -> Nil {
  let parent = process.new_subject();
  list.range(1, 1_000_000)
  |> list.each(fn(i) {
    let _ = process.spawn(fn() { process.send(parent, i); });
    Nil;
  });
  let _ = echo count(parent, 1_000_000, 0);
  Nil;
}

fn count(s: Subject(Int), remaining: Int, sum: Int) -> Int {
  case remaining {
    0 -> sum;
    _ -> count(s, remaining - 1, sum + process.receive_forever(s));
  };
}
```

**Supervised worker**
```lush
import lush/supervisor;
import lush/process;
import lush/process.{type Subject};

fn worker() -> Nil {
  process.sleep(1000);
  worker();
}

pub fn main() -> Nil {
  let assert Ok(_) = supervisor.start(supervisor.OneForOne, [
    supervisor.worker(fn() {
      case process.try_spawn(worker) {
        Ok(pid) -> Ok(pid);
        Error(process.SpawnFailed) -> Error(supervisor.ChildSpawnFailed);
      };
    }),
  ], max_restarts: 3, within: 5000);
  // Keep main alive: returning terminates all processes (§7.3).
  let idle: Subject(Nil) = process.new_subject();
  process.receive_forever(idle);
}
```

---

## 14. No FFI: rules and rationale

- Lush has **no** `@external`, no `unsafe`, no native modules, no linking to C/Rust/Erlang/JS, and no plugin system. This is a permanent language principle, not a v1 limitation.
- The runtime exposes **internal intrinsics** (e.g. `intrinsic.tcp_read`) usable **only** by authorized modules of the standard library bundled with that exact toolchain. Trust derives from compiler-established bundled package identity and verified stdlib/stub metadata, never an import-path prefix, user-supplied manifest flag, filename, or bytecode claim. The compiler and bundle validator MUST reject unauthorized intrinsic references and namespace impersonation (§10). Intrinsics are an implementation detail and may change with a versioned ABI.
- Extending Lush with new capabilities (a new hash algorithm, a new protocol) means contributing it to the runtime and stdlib (Rust) or writing it in Lush.
- Interop with other software happens over **process boundaries**: `os.exec` subprocesses, stdin/stdout pipes, TCP/Unix sockets, HTTP, files.
- No FFI removes user-supplied native extensions; it does **not** by itself prove memory safety, audit dependencies, sandbox effects, or ensure reproducible builds. Memory safety relies on a sound type system, validated bytecode, correct VM/GC/intrinsics, and the native runtime's dependencies. Those components form the trusted computing base and require testing and security review. Self-contained deployment and reproducibility have the explicit limits and input requirements in §11.

---

## 15. Rust implementation plan

### 15.1 Workspace layout
```
lush/
  Cargo.toml                (workspace)
  crates/
    lush-syntax/     lexer (logos), parser, AST, formatter
    lush-types/      name resolution, HM inference, exhaustiveness
    lush-ir/         Core IR, optimiser, bytecode compiler
    lush-vm/         values, heap, GC, interpreter, scheduler, mailboxes, signals
    lush-runtime/    reactor (mio), blocking pool, timers, intrinsics (fs, net, crypto, tls)
    lush-pkg/        lush.mod parsing, MVS, git fetch, cache, checksums
    lush-lsp/        language server (tower-lsp)
    lush-cli/        `lush` binary
  stdlib/            Lush source for lush/*
  tests/             end-to-end .lush programs + expected output
```

### 15.2 Suggested crates
`logos`, `ariadne`/`miette`, `crossbeam-deque`, `crossbeam-channel`, `parking_lot`, `mio`, `rustls`, `sha2`/`blake3`, `serde`/`toml`, `tower-lsp`, `clap`, `camino`, `gix`, `regex-automata`. Cranelift **[v2]**.

### 15.3 Build order (each step must have passing tests before the next)
1. **Syntax:** lexer, parser, AST, pretty-printer/formatter for the full §12 grammar and the lexical forms it references (§3), including every normative syntax example in §§3–5. Parser fixtures for every §12 conformance-table row (rows that need type information, such as opaque record updates, are completed in step 2). Snapshot tests and formatter round-trip tests.
2. **Types:** name resolution (modules, imports, prelude, qualified names, visibility including opaque types, import cycles); desugaring of `use`, pipes and captures (§§5.3, 5.5, 5.8) before inference; value-restricted HM inference with level-based generalisation and the sealed `Eq`/`Neg` constraints carried in type schemes and module interfaces (§4.3); ADTs, records, aliases and generics; numeric literal range and constant-expression checks (§5.8); exhaustiveness (errors) and redundancy (warnings); the compile-time warnings required by §§3–4 (unused, unreachable, casing, `todo`, unannotated `pub fn`); and the type-dependent §12 conformance rows, such as opaque record updates. Declaration-only interface stubs for the `lush/*` modules that the spec examples import stand in for the standard library until step 10. Golden error-message tests, plus regression tests for the §15.4 Type soundness and Equality constraints items.
3. **Compile + VM (single process):** Core IR, bytecode, interpreter, tail calls. Run `fib`, list ops, `case`.
4. **Heap and GC:** per-process heap, Cheney copying, generational, stress test with tiny heap sizes to shake out GC bugs (`LUSH_GC_STRESS=1` collects on every allocation).
5. **Processes:** spawn, send, receive, subjects, selectors, reduction counting, single scheduler thread. Target: 100k processes ping-pong.
6. **Multi-core scheduler:** work stealing, cross-thread mailboxes, message copying, shared large binaries.
7. **Fault tolerance:** links, monitors, exit signals, trap-exit, panics with stack traces. `actor`, `supervisor`, `task`.
8. **Toolchain:** `new/build/run/test/fmt/check`, executable bundling, `lush.mod`/MVS/git fetch.
9. **Runtime I/O:** reactor, blocking pool, `fs`, `net/tcp`, `time`, `os`.
10. **Stdlib breadth:** dict (HAMT), vector, set, json, http, regexp, crypto.
11. **LSP and doc:** include documentation syntax/type-check fixtures and packaging/security regression tests. `observe` and cross-target builds are v1.1 work (§16), not v1 gates.

### 15.4 Acceptance criteria for v1

These are semantic, correctness, and security gates. Hardware-dependent performance objectives are reported separately in §15.5, not treated as language-level latency guarantees.

- [ ] **Syntax and documentation:** all complete examples across the spec and docs parse and type-check via §11.4 fixtures; snippets have explicit wrappers and API pseudocode is explicitly excluded. Enforce semicolons on every block statement, import/constant, and `case` arm, including final arms and branch blocks; reject missing delimiters even across newlines. Positive/negative grammar conformance fixtures cover precedence, associativity, patterns, labels, captures, qualified types, record updates, and the lexical forms referenced by the abridged §12 grammar. `main` returns `Nil`; its return/crash terminates remaining processes with the specified exit status. Formatter round trips preserve meaning/comments and are idempotent across the stdlib and fixtures.
- [ ] **Type soundness:** inference/generalisation uses a value restriction: generalise eligible non-expansive values, not effectful application results or allocated subject identities; keep weak type variables shared until constrained. Regression tests MUST reject using one `new_subject()` result (including through aliases, returned aggregates, or closures capturing it) at both `Subject(Int)` and `Subject(String)`, while permitting a polymorphic function that creates a fresh subject per call. Abstract types, ADTs, higher-order functions, and captured environments retain their type invariants through compilation and message copying. Non-exhaustive matches are errors and redundant patterns warn.
- [ ] **Equality constraints:** structural equality and collection-key eligibility use compiler-internal constraints propagated through inference, generalisation, module interfaces, opaque representations, recursive ADTs, and containers. Reject direct or nested function equality and function-containing keys, including indirect instantiations of generic helpers; accept eligible structural values. These are built-in constraints, not user-definable traits or ad-hoc runtime comparisons of closures.
- [ ] **Numeric semantics:** test the full signed 64-bit Int range across immediate/boxed representations, checked arithmetic overflow, division/remainder by zero, minimum-Int negation and division overflow, and wrapping/try helpers. Test Float literal syntax, type-directed negation, finite-result policy, division by zero, parsing/conversion rejection of non-finite values, and constant-folded versus runtime behavior. Optimisation MUST preserve values and panic behavior.
- [ ] **Desugaring and execution:** `use` consumes the rest of its block as a callback with correct scoping, argument order, exactly-once evaluation, and final value; test nested/empty `use`, labelled arguments with an implicit final callback, pipes, and callback failures. Direct, mutual, and closure tail calls do not grow the stack. Run GC-stress tests over all value kinds, closures, messages, and shared buffers.
- [ ] **Scheduling:** instrument deterministic work counters to verify every instruction charges reductions, including nonallocating tail recursion and jump loops. Hashing/sorting, copying, mailbox scans, and GC obey §7.6 bounded steps (including large individual objects), yield/resume without lost work, and cannot monopolise a scheduler. Blocking I/O stays off scheduler threads. Independent processes continue while another is collecting; neither fairness nor independent GC promises a fixed wall-clock response time.
- [ ] **Subject/selector ownership:** non-owner receive/close/register/select operations panic before consuming messages. Closed-subject checks, idempotent close, immutable selector builders, duplicate rejection, empty selectors, earliest-eligible-message selection independent of builder order, and exactly one mapper invocation obey §9.1. Mapper panic consumes only the selected message; unrelated queues remain intact. Sends to dead/closed subjects are no-ops. Typed lifecycle events never enter `Subject(msg)`.
- [ ] **Deadlines and timers:** negative durations fail before effects; zero polls, absolute deadlines, admission-order arbitration, wakeup races, and pre-deadline admission followed by late scanning follow §9.2. Timer cancellation/firing races never duplicate delivery; creator exit cancels timers, retained payload charges are released, and internal wait timers are cleaned up on every path.
- [ ] **Links and monitors:** exercise atomic link/spawn-link versus immediate exit, unlink races, normal/abnormal/heap-limit/killed propagation, trapping behavior, and absent targets. Monitoring before/after death yields exactly one typed `Down` with the correct reference/reason. Flush-demonitor prevents subsequent matching `Down` even under races; foreign-owner monitor cleanup panics. Killing a trapping child still terminates it, but its trapping supervisor receives `Exit(child, Killed)` rather than being killed by that propagated signal.
- [ ] **Actor cleanup:** startup success, capacity failure, init crash/timeout, and starter death release provisional children, handshake subjects, timers, and monitors as specified. Calls test reply/Down/deadline ordering, `ServerDown(NoProcess)`, closed-live-owner timeout, exactly-once `make_msg`, and callback panic/caller exit. Every completion closes/discards private replies, flushes the matching monitor, and releases the timer without consuming unrelated signals/messages. Repeated calls and late/duplicate replies do not accumulate resources.
- [ ] **Supervisor/task lifecycle:** cover all restart strategies/policies, reverse-order shutdown, restart-wave intensity, failed-start rollback, nested supervisors, discovery publication/stale addresses, grace expiration and kill acknowledgment. Forced supervisor death follows §9.4 rather than promising cleanup it cannot perform. Tasks test owner-only/single await, linked-crash behavior with/without trapping, result/Down/timeout races, unlink-before-cancel, owner-exit cancellation even for trapping children, retained unawaited results, ordered `parallel_map`, and partial-start/sibling cleanup.
- [ ] **Resource accounting:** test §7.4 ownership budgets for heaps, stacks, queue entries, partial message copies, links/monitors, timers, and shared-buffer backing allocations (including sub-slices). Receiver exhaustion produces `HeapLimit` without killing the VM or requiring an allocation to report the failure. Endpoint exit discards partial copies; adoption does not double-charge. Teardown, cancellation, collection, and resource-close paths release charges/references and runtime-owned I/O registrations/handles; repeated lifecycle stress tests show no unbounded retention. Collector reserve remains separately bounded.
- [ ] **Dependency security and reproducibility:** MVS retains requirements from older reachable versions, selects the highest required version per path, and respects major-path rules. Git/archive equivalents hash identically; byte/path changes differ. Reject checksum mismatch, strict-CI missing sums, malicious paths/links/collisions, oversized archives, tampered cache trees, and reserved-namespace impersonation. Verify all graph inputs, not just selected source files. Offline misses fail without networking. Two clean builds from §11.2's declared inputs produce identical output independent of cache state and checkout location.
- [ ] **Packaging and trust:** test each v1 host platform/architecture on a clean machine at its supported OS baseline without Lush, a linker, SDK, or external signer. macOS output has a valid built-in ad-hoc signature after bundling. Reject cross targets in v1, incompatible stubs/ABIs, corrupt/truncated bundles, malicious offsets/lengths, invalid bytecode and forged intrinsic authorization. Fuzz archive, bundle, and bytecode validators with bounded resources. No local/dependency package can acquire intrinsic privileges through names, manifests, or serialized claims; only the toolchain-bundled stdlib identity is authorized.

### 15.5 Performance targets and reproducible benchmark protocol

The initial engineering targets are ≤ 4 KB of initial per-process allocations for newly spawned idle processes under §7.2 accounting, and completion of §13's million-message workload in < 10 s with < 5 GB peak RSS on a reference 8-core host. They are separate workloads and **not semantic acceptance gates or hard latency promises**. Report misses and regressions rather than weakening correctness tests to meet them.

- Establish a versioned reference hardware baseline before publishing comparisons: a dedicated 8-physical-core host with at least 32 GiB RAM and local SSD, fixed to 8 scheduler threads. Record exact CPU model/architecture, physical/logical cores, RAM, storage, OS/kernel, power mode, and background load. Publish distinct Linux/macOS baselines; results on different machines are not interchangeable. Record toolchain/source revision, runtime/stub digests, build profile and flags, allocator, all `LUSH_*` settings, dependency sums, benchmark revision, and seeds/input sizes.
- Provide checked-in harnesses and literal run commands. Use release builds, build outside timed execution, specify warm/cold cache state, perform at least 3 warmups and 10 measured independent runs, and retain raw samples. Use monotonic clocks; report median, p95, p99, maximum, throughput, peak RSS, and allocation/ownership-budget measurements with sample counts and percentile method. For latency percentiles gather at least 10,000 events per measured run; do not infer tail guarantees from ten total-duration samples. Report variance and failures, including resource exhaustion; do not silently discard outliers.
- Idle-process measurements use a barrier proving all processes are simultaneously alive and parked; separately report §7.2 per-process allocations, shared-runtime baseline, caller references, and total RSS. Measure 1,000,000 processes on the reference host; smaller resource-constrained runs disclose their count and are not presented as million-process results.
- For the million-message aggregation workload, validate the received count/sum and report peak mailbox size and configured parent ownership budget as well as wall time/RSS. Queue buildup can legitimately exhaust the default budget; benchmark overrides are explicit, not changes to default semantics.
- Measure ping-pong throughput/latency, spawn rate, unmatched selective scans, large builtin operations/message copies, and latency of a small active process while a separate process collects a 1 GiB live heap. Configure and report enough heap budget and copying reserve for that GC workload. Run with one scheduler and multiple schedulers; report GC pause/chunk distributions, other-process p99/max latency, and work-counter compliance. These characterize hardware-dependent performance; §7's bounded-work contract, not “a few ms,” is the correctness criterion.

---

## 16. Explicitly deferred

### v1.1
- Cross-target builds for supported Linux/macOS architectures, with versioned release stubs, platform-specific bundling/signing, and destination-platform validation (§11.3). Windows x86_64 requires a separate support decision and packaging/test plan; it is not implicitly promised by cross-target support.
- `lush observe`: live process inspector/UI, with its own access-control and overhead design; not a v1 toolchain acceptance requirement.
- `lush vendor`: verified vendored source trees using the same canonical checksums as fetched dependencies.

### v2+
Traits/typeclasses (for user-facing `to_string`/`compare` abstractions, not needed for v1's internal equality constraints), macros, `if` sugar, JIT/AOT via Cranelift, bigint literals, process registry/names, distribution across nodes (future wire identity/serialization must be designed; v1 does not promise portable serialized `Pid`/subject values), hot code loading, HTTP/2 and QUIC, a package registry/proxy, user-facing property-based testing, WASM target. Implementation property/fuzz tests are not deferred.
