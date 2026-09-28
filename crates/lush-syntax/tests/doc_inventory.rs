//! Documentation fence inventory and parse checks (`spec.md` §11.4).

use lush_syntax::doc::{check_fence, extract_fences, spec_inventory, FenceClass};
use std::collections::HashSet;
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

    let mut keys = HashSet::new();
    for (i, entry) in inventory.iter().enumerate() {
        assert_eq!(entry.index, i, "inventory index mismatch at {i}");
        assert!(
            keys.insert(entry.key),
            "duplicate inventory key {}",
            entry.key
        );
        let fence = &fences[i];
        if fence.lang == "lush" {
            assert!(
                !matches!(entry.class, FenceClass::NonLush),
                "lush fence {} ({}) classified NonLush",
                i,
                entry.key
            );
        }
        if fence.lang != "lush" && !fence.lang.is_empty() {
            assert!(
                matches!(entry.class, FenceClass::NonLush),
                "non-lush lang {:?} fence {} ({}) should be NonLush",
                fence.lang,
                i,
                entry.key
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

    for (fence, entry) in fences.iter().zip(inventory.iter()) {
        assert_eq!(fence.index, entry.index);
        match entry.class {
            FenceClass::Module | FenceClass::WrappedSnippet | FenceClass::ExpectedError => {
                check_fence(entry.class, &fence.body).unwrap_or_else(|e| {
                    panic!(
                        "fence {} ({}, line {}, {:?}) failed: {e:?}\n{}",
                        fence.index, entry.key, fence.line, entry.class, fence.body
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
