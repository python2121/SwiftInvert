//! `AppModel.buildTree` / `flatten` rules on a synthetic directory tree.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::field_reassign_with_default
)]

mod common;

use std::cmp::Ordering;
use std::path::Path;

use negsession::library::{MAX_SCAN_DEPTH, natural_cmp};
use negsession::{build_tree, flatten};

fn touch(p: &Path) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, b"").unwrap();
}

#[test]
fn natural_order_is_digit_aware_and_case_insensitive() {
    assert_eq!(natural_cmp("IMG_2.CR3", "IMG_10.CR3"), Ordering::Less);
    assert_eq!(natural_cmp("img_10.cr3", "IMG_9.CR3"), Ordering::Greater);
    assert_eq!(natural_cmp("a", "B"), Ordering::Less);
    assert_eq!(natural_cmp("B", "a"), Ordering::Greater);
    assert_eq!(
        natural_cmp("IMG_0001.CR3", "IMG_1.CR3"),
        Ordering::Less,
        "more zeros first"
    );
    assert_eq!(natural_cmp("same", "same"), Ordering::Equal);
    assert_eq!(natural_cmp("roll", "roll 2"), Ordering::Less);
    let mut v = vec!["Roll 10", "roll 2", "Roll 1", "album", "Album 3"];
    v.sort_by(|a, b| natural_cmp(a, b));
    assert_eq!(v, ["album", "Album 3", "Roll 1", "roll 2", "Roll 10"]);
}

#[test]
fn tree_rules_files_before_subfolders_hidden_skipped_rawless_pruned() {
    let root = common::scratch_dir("library");
    touch(&root.join("IMG_10.CR3"));
    touch(&root.join("IMG_2.cr3"));
    touch(&root.join("notes.txt"));
    touch(&root.join(".hidden.CR3"));
    touch(&root.join(".cache/IMG_1.CR3"));
    touch(&root.join("empty/readme.md"));
    touch(&root.join("b roll/DSC_1.NEF"));
    touch(&root.join("a roll/sub/X.ARW"));
    touch(&root.join("a roll/IMG_3.DNG"));

    let tree = build_tree(&root).expect("RAW files present");
    assert_eq!(tree.path, root);
    let names: Vec<String> = tree
        .files
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        ["IMG_2.cr3", "IMG_10.CR3"],
        "natural-sorted, non-RAW + hidden skipped"
    );
    let subs: Vec<&str> = tree.subfolders.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        subs,
        ["a roll", "b roll"],
        "RAW-less `empty` and hidden `.cache` pruned"
    );
    assert_eq!(tree.total_count, 5);
    assert_eq!(tree.subfolders[0].total_count, 2);

    // Depth-first: a folder's own files, then its subfolders.
    let flat = flatten(&tree);
    let rel: Vec<String> = flat
        .iter()
        .map(|p| {
            p.strip_prefix(&root)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(
        rel,
        [
            "IMG_2.cr3",
            "IMG_10.CR3",
            "a roll/IMG_3.DNG",
            "a roll/sub/X.ARW",
            "b roll/DSC_1.NEF"
        ]
    );
}

#[test]
fn rawless_root_is_none_and_depth_is_capped() {
    let root = common::scratch_dir("library");
    touch(&root.join("only/text.txt"));
    assert!(build_tree(&root).is_none());
    assert!(build_tree(&root.join("does-not-exist")).is_none());

    // A RAW at depth MAX+1 is beyond the cap; one at MAX is in.
    let mut deep = root.clone();
    for i in 0..=MAX_SCAN_DEPTH {
        deep = deep.join(format!("d{i}"));
    }
    touch(&deep.join("too-deep.CR3"));
    assert!(
        build_tree(&root).is_none(),
        "depth {} pruned",
        MAX_SCAN_DEPTH + 1
    );
    let mut ok = root.clone();
    for i in 0..MAX_SCAN_DEPTH {
        ok = ok.join(format!("d{i}"));
    }
    touch(&ok.join("in-reach.CR3"));
    let tree = build_tree(&root).unwrap();
    assert_eq!(tree.total_count, 1);
    assert_eq!(flatten(&tree).len(), 1);
}
