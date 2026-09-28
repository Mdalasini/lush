//! Documentation fence inventory and parse checks (`spec.md` §11.4).

use lush_syntax::doc::{check_fence, extract_fences, spec_inventory, FenceClass};
use std::path::PathBuf;

fn spec_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../spec.md")
}

#[test]
fn every_fence_is_classified() {
    let markdown = std::fs::read_to_string(spec_path()).expect("read spec.md");
    let fences = extract_fences(&markdown);
    let inventory = spec_inventory();
    assert_eq!(
        fences.len(),
        inventory.len(),
        "inventory size must match fence count; update spec_inventory()"
    );
    for (i, fence) in fences.iter().enumerate() {
        assert_eq!(inventory[i].0, i, "inventory index mismatch at {i}");
        if fence.lang == "lush" {
            assert!(
                !matches!(inventory[i].1, FenceClass::NonLush),
                "lush fence {i} classified NonLush"
            );
        }
        if fence.lang != "lush" && !fence.lang.is_empty() {
            // ebnf etc.
            assert!(
                matches!(inventory[i].1, FenceClass::NonLush),
                "non-lush lang {:?} fence {i} should be NonLush",
                fence.lang
            );
        }
    }
}

#[test]
fn no_unclassified_or_silent_skips() {
    let markdown = std::fs::read_to_string(spec_path()).expect("read spec.md");
    let fences = extract_fences(&markdown);
    let inventory = spec_inventory();
    let mut parse_checked = 0usize;
    let mut excluded = 0usize;

    for (fence, (idx, class)) in fences.iter().zip(inventory.iter()) {
        assert_eq!(fence.index, *idx);
        match class {
            FenceClass::Module | FenceClass::WrappedSnippet | FenceClass::ExpectedError => {
                check_fence(*class, &fence.body).unwrap_or_else(|e| {
                    panic!(
                        "fence {} (line {}, {:?}) failed: {e:?}\n{}",
                        fence.index, fence.line, class, fence.body
                    )
                });
                parse_checked += 1;
            }
            FenceClass::ApiPseudocode
            | FenceClass::IllustrativePlaceholder
            | FenceClass::NonLush => {
                excluded += 1;
            }
        }
    }

    assert!(parse_checked > 0, "expected some parse-checked fences");
    assert!(excluded > 0, "expected some explicitly excluded fences");
}
