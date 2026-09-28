#!/usr/bin/env bash
# Adversarial probes for lush-syntax, used by the review-pr skill.
#
# Usage: .claude/skills/review-pr/probe.sh [scratch-dir]
#
# Builds a small release-mode probe binary against the checked-out crate in a
# scratch directory (outside the workspace), then runs functionality, formatter,
# security and performance cases. Every case prints its input and result so you
# can paste it into a review comment as a repro. Add PR-specific cases at the end.
set -uo pipefail

REPO="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
SCRATCH="${1:-${TMPDIR:-/tmp}/lush-probe}"
mkdir -p "$SCRATCH/src"

cat > "$SCRATCH/Cargo.toml" <<EOF
[package]
name = "lush-probe"
version = "0.0.0"
edition = "2021"
[dependencies]
lush-syntax = { path = "$REPO/crates/lush-syntax" }
[workspace]
EOF

cat > "$SCRATCH/src/main.rs" <<'EOF'
use std::io::Read;
fn main() {
    let mode = std::env::args().nth(1).expect("mode: parse | fmt | render");
    let mut s = String::new();
    std::io::stdin().read_to_string(&mut s).unwrap();
    match mode.as_str() {
        "parse" => {
            let o = lush_syntax::parse_module(&s);
            if o.ok() {
                println!("OK");
            } else {
                for d in o.diagnostics.iter().take(3) {
                    println!("ERR {d} @{}", d.span);
                }
                println!("({} diagnostics)", o.diagnostics.len());
            }
        }
        "fmt" => match lush_syntax::format_round_trip(&s) {
            Ok(out) => print!("{out}--round trip ok\n"),
            Err(e) => println!("ROUND TRIP ERR {e}"),
        },
        "render" => {
            let o = lush_syntax::parse_module(&s);
            for d in &o.diagnostics {
                print!("{}", d.render("probe.lush", &o.source, false));
            }
        }
        other => panic!("unknown mode {other}"),
    }
}
EOF

(cd "$SCRATCH" && cargo build -q --release) || { echo "probe build failed"; exit 1; }
P="$SCRATCH/target/release/lush-probe"

# Make raw ESC/BEL bytes visible (they must never appear) without mangling UTF-8.
visible() { sed -e $'s/\x1b/<ESC>/g' -e $'s/\x07/<BEL>/g'; }
# case MODE SOURCE: print the input, then the probe's output.
case_() { echo "== $1 :: $2"; printf '%s' "$2" | timeout 30 "$P" "$1" 2>&1 | visible | head -15; }
# gen MODE PYTHON-EXPR: generate a large input, report the result's tail and the elapsed time.
gen() {
  local start end
  start=$(date +%s.%N)
  python3 -c "import sys; sys.stdout.write($2)" | timeout 120 "$P" "$1" 2>&1 | tail -2
  end=$(date +%s.%N)
  echo "   ^ $1 :: $2  ($(echo "$end - $start" | bc)s)"
}

echo "### Functionality: each should give the result in the comment"
case_ parse 'fn f() { a == b && c == d; }'          # OK
case_ parse 'fn f() { a < b < c; }'                 # one E0110
case_ parse 'fn f() { a < b == c; }'                # one E0110
case_ parse 'fn f() { fn(x) { x; }; }'              # OK
case_ parse 'fn f() { list.map(xs, fn(_) { 1; }); }' # OK
case_ parse 'fn f() { let x = _; }'                 # E0180
case_ parse 'fn f() { f(_, _); }'                   # E0181
case_ parse 'fn f() { [..xs,]; }'                   # OK
case_ parse 'fn f() { [..xs, x]; }'                 # E0130
case_ parse 'import github.com/user/repo;'          # OK
case_ parse 'fn f() { let x = 123abc; }'            # E0002
case_ parse 'fn f() -> #(Int) { todo; }'            # E0132

echo "### Formatter: idempotent, comments kept in place, <= 80 columns, no trailing spaces"
case_ fmt 'fn f() { a |> g |> h; }'
case_ fmt 'fn f() {
  let x = 1; // trailing
  case x {
    _ -> 3; // after arm
  };
  // before close
}
pub type T {
  A
  // between variants
  B
}
'
case_ fmt 'fn f() { some_function_name(argument_number_one, argument_number_two, argument_three_is_long, and_four); }'
case_ fmt 'fn f() { let total = first_value_here + second_value_here + third_value_here + fourth_value + fifth; }'

echo "### Security: none may abort; control bytes must not reach the terminal raw"
gen parse "'fn f() { ' + '('*10000 + '1' + ')'*10000 + '; }'"
gen parse "'fn f() {' + '{'*50000 + '}'*50000 + '}'"
gen parse "'fn f() { ' + '-'*100000 + '1; }'"
gen fmt   "'fn f() { ' + 'a + '*200000 + 'b; }'"
gen fmt   "'fn f() { ' + 'a |> '*200000 + 'b; }'"
gen fmt   "'fn f() { x' + '.f(1)'*20000 + '; }'"
printf 'fn f() { \x1b]0;pwned\x07; }' | "$P" parse | visible | head -3
# The label must underline `123abc` even after multi-byte and control characters.
printf 'fn f() {\n  let s = "\xc3\xa9\xf0\x9f\x98\x80\r\x1b"; let x = 123abc;\n}\n' | "$P" render | visible

echo "### Performance: capped diagnostics, roughly linear formatting"
gen parse "'\$ '*2000000"
gen fmt   "'fn f() { [' + ', '.join('item_%d'%i for i in range(50000)) + ']; }'"
gen fmt   "'\n'.join('fn f%d(a, b) { let x = call_something(a, b, [1, 2, 3]) |> other(_, a); case x { 1 -> a; _ -> b; }; }'%i for i in range(3000))"

# --- PR-specific cases go below ---
