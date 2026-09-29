//! Shared helpers for step-3 e2e programs under `tests/e2e/`.

use std::fs;
use std::path::{Path, PathBuf};

use lush_ir::OptLevel;
use lush_vm::{compile_sources_with_opt, RunResult, Vm, VmConfig};

pub fn e2e_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/e2e")
}

pub fn read_case(name: &str) -> (String, String, String, i32) {
    let root = e2e_root().join(name);
    // Support both `tests/e2e/foo.lush` and `tests/e2e/foo/…` layouts.
    let (src_path, base) = if root.is_dir() {
        (root.join("src").join("main.lush"), root)
    } else {
        let base = e2e_root();
        (base.join(format!("{name}.lush")), base)
    };
    let src = fs::read_to_string(&src_path).unwrap_or_else(|e| panic!("read {src_path:?}: {e}"));
    let stdout = fs::read_to_string(base.join(format!("{name}.stdout")))
        .or_else(|_| fs::read_to_string(base.join("stdout")))
        .unwrap_or_default();
    let stderr = fs::read_to_string(base.join(format!("{name}.stderr")))
        .or_else(|_| fs::read_to_string(base.join("stderr")))
        .unwrap_or_default();
    let status: i32 = fs::read_to_string(base.join(format!("{name}.status")))
        .or_else(|_| fs::read_to_string(base.join("status")))
        .unwrap_or_else(|_| "0".into())
        .trim()
        .parse()
        .expect("status");
    (src, stdout, stderr, status)
}

pub fn run_source(path: &str, src: &str, level: OptLevel) -> RunResult {
    let program = compile_sources_with_opt(&[(path.into(), src.into())], path, level)
        .unwrap_or_else(|d| {
            panic!(
                "compile failed: {:?}",
                d.iter()
                    .map(|x| format!("{}:{}", x.code, x.message))
                    .collect::<Vec<_>>()
            )
        });
    let mut vm = Vm::new(program, VmConfig::default());
    vm.run_to_completion()
}

pub fn assert_case(name: &str) {
    let (src, expect_out, expect_err, expect_status) = read_case(name);
    let exit = run_source("main", &src, OptLevel::default());
    assert_eq!(exit.status, expect_status, "{name} status");
    assert_eq!(
        String::from_utf8_lossy(&exit.stdout),
        expect_out,
        "{name} stdout"
    );
    assert_eq!(
        String::from_utf8_lossy(&exit.stderr),
        expect_err,
        "{name} stderr"
    );
}

pub fn assert_differential(name: &str) {
    let (src, expect_out, expect_err, expect_status) = read_case(name);
    let a = run_source("main", &src, OptLevel::O0);
    let b = run_source("main", &src, OptLevel::O1);
    assert_eq!(a.status, b.status, "{name} status O0 vs O1");
    assert_eq!(
        String::from_utf8_lossy(&a.stdout),
        String::from_utf8_lossy(&b.stdout),
        "{name} stdout O0 vs O1"
    );
    assert_eq!(
        String::from_utf8_lossy(&a.stderr),
        String::from_utf8_lossy(&b.stderr),
        "{name} stderr O0 vs O1"
    );
    assert_eq!(a.status, expect_status, "{name} status vs golden");
    assert_eq!(
        String::from_utf8_lossy(&a.stdout),
        expect_out,
        "{name} stdout vs golden"
    );
    assert_eq!(
        String::from_utf8_lossy(&a.stderr),
        expect_err,
        "{name} stderr vs golden"
    );
}

pub fn assert_quantum_invariant(name: &str) {
    let (src, _, _, _) = read_case(name);
    let program = compile_sources_with_opt(&[("main".into(), src)], "main", OptLevel::default())
        .expect("compile");
    let mut results = Vec::new();
    for q in [1u64, 7, 4000, u64::MAX] {
        let mut vm = Vm::new(program.clone(), VmConfig::default());
        let exit = loop {
            match vm.run_slice(q) {
                lush_vm::SliceResult::Yielded => continue,
                lush_vm::SliceResult::Done(r) => break r,
            }
        };
        results.push(exit);
    }
    let first = &results[0];
    for (i, r) in results.iter().enumerate().skip(1) {
        assert_eq!(r.status, first.status, "{name} status quantum[{i}]");
        assert_eq!(r.stdout, first.stdout, "{name} stdout quantum[{i}]");
        assert_eq!(r.stderr, first.stderr, "{name} stderr quantum[{i}]");
        assert_eq!(
            r.reductions, first.reductions,
            "{name} reductions quantum[{i}]"
        );
    }
}

/// Flat e2e case names (stem of `tests/e2e/<name>.lush`), excluding known
/// compile-error-only fixtures that have no runnable expectations.
pub fn list_runnable_cases() -> Vec<String> {
    let root = e2e_root();
    let mut names = Vec::new();
    if let Ok(rd) = fs::read_dir(&root) {
        for ent in rd.flatten() {
            let p = ent.path();
            if p.extension().and_then(|s| s.to_str()) == Some("lush") {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    // `unavailable` is a negative compile fixture (E2000).
                    if stem == "unavailable" {
                        continue;
                    }
                    names.push(stem.to_string());
                }
            }
        }
    }
    names.sort();
    names
}

#[allow(dead_code)]
pub fn list_flat_cases() -> Vec<String> {
    list_runnable_cases()
}

#[allow(dead_code)]
pub fn expect_files_present(name: &str) {
    let root = e2e_root();
    for ext in ["lush", "stdout", "stderr", "status"] {
        let p = root.join(format!("{name}.{ext}"));
        assert!(
            p.exists() || Path::new(&root).join(name).join(ext).exists(),
            "missing {p:?}"
        );
    }
}
