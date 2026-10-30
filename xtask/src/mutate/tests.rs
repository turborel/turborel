use super::*;

/// A mutation of `file` from `original` to `replacement`, named "M", that
/// two integration tests and a unit test must catch.
fn mutation(file: &'static str, original: &'static str, replacement: &'static str) -> Mutation {
    Mutation { name: "M", breaks: "nothing", file, original, replacement, integration: &["a::x", "a::y"], unit: &["b::tests::z"] }
}

/// A directory of its own under the workspace's `target`, empty.
fn scratch(name: &str) -> PathBuf {
    let dir = command::workspace_root().join("target").join("xtask-mutate-tests").join(name);
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Verifies: [TOOL] R-TEST-9 every mutation of the list applies to the
/// source as it is, its original found there exactly once, changes it, and
/// names tests; no two share a name. So `verify`, which runs these tests,
/// keeps the list in step with the code.
#[test]
fn every_mutation_applies_to_the_source() {
    let root = command::workspace_root();
    let mut names: Vec<&str> = MUTATIONS.iter().map(|m| m.name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), MUTATIONS.len(), "names are unique");
    for m in &MUTATIONS {
        let source = fs::read_to_string(root.join(m.file)).unwrap();
        let mutated = apply(&source, m).unwrap_or_else(|e| panic!("{e}"));
        assert_ne!(mutated, source, "{}", m.name);
        assert!(!m.integration.is_empty() || !m.unit.is_empty(), "{} names no test", m.name);
    }
}

/// Verifies: [TOOL] R-TEST-9 a mutation applies where its original is
/// found exactly once; not found, or found twice, it is refused.
#[test]
fn a_mutation_applies_only_where_its_original_is_once() {
    let m = mutation("f.rs", "a < b", "a <= b");
    assert_eq!(apply("if a < b {}", &m).unwrap(), "if a <= b {}");
    assert!(apply("if a > b {}", &m).is_err(), "not found");
    assert!(apply("a < b; a < b", &m).is_err(), "found twice");
}

/// Verifies: [TOOL] R-TEST-9 libtest's lines are read as each test's
/// outcome; other lines are not.
#[test]
fn test_outcomes_are_read_from_libtest() {
    let stdout = "running 3 tests\ntest a::x ... ok\ntest a::y ... FAILED\ntest a::w ... ignored\n\ntest result: FAILED. 1 passed";
    let found = outcomes(stdout);
    assert_eq!(found.get("a::x"), Some(&true));
    assert_eq!(found.get("a::y"), Some(&false));
    assert_eq!(found.len(), 2);
}

/// Verifies: [TOOL] R-TEST-9 a mutation is caught when every named test
/// failed; it survived when none did; a named test that passed, or did not
/// run, is reported.
#[test]
fn a_mutation_is_caught_only_when_every_named_test_fails() {
    let m = mutation("f.rs", "a", "b");
    let seen = |list: &[(&str, bool)]| list.iter().map(|(n, ok)| ((*n).to_owned(), *ok)).collect::<HashMap<_, _>>();
    assert_eq!(verdict(&m, &seen(&[("a::x", false), ("a::y", false), ("b::tests::z", false)])), None);
    assert_eq!(verdict(&m, &seen(&[("a::x", true), ("a::y", true), ("b::tests::z", true)])).unwrap(), "survived: no named test failed");
    let partly = verdict(&m, &seen(&[("a::x", false), ("a::y", true)])).unwrap();
    assert_eq!(partly, "these named tests did not fail: a::y, b::tests::z");
}

/// Verifies: [TOOL] R-TEST-9 the command takes every mutation, or one by
/// its name; an unknown name, or more than one, is refused.
#[test]
fn mutations_are_chosen_by_name() {
    assert_eq!(chosen(&MUTATIONS, &[]).unwrap().len(), MUTATIONS.len());
    let first = MUTATIONS.first().unwrap().name;
    assert_eq!(chosen(&MUTATIONS, &[first]).unwrap().iter().map(|m| m.name).collect::<Vec<_>>(), [first]);
    assert!(chosen(&MUTATIONS, &["no such mutation"]).is_err());
    assert!(chosen(&MUTATIONS, &[first, first]).is_err());
}

/// Verifies: [TOOL] R-TEST-9 a mutation in place is written back by
/// `restore`, and by `Drop` when the run fails before it; the copy kept
/// while it is in place is removed once it is written back.
#[test]
fn a_mutated_file_is_written_back() {
    let root = scratch("written-back");
    fs::write(root.join("f.rs"), "if a < b {}").unwrap();
    let m = mutation("f.rs", "a < b", "a <= b");
    let applied = Applied::new(&root, &m).unwrap();
    assert_eq!(fs::read_to_string(root.join("f.rs")).unwrap(), "if a <= b {}");
    assert_eq!(fs::read_to_string(root.join(BACKUP_DIR).join(BACKUP_PATH)).unwrap(), "f.rs");
    applied.restore().unwrap();
    assert_eq!(fs::read_to_string(root.join("f.rs")).unwrap(), "if a < b {}");
    assert!(!root.join(BACKUP_DIR).join(BACKUP_PATH).exists());
    let dropped = Applied::new(&root, &m).unwrap();
    drop(dropped);
    assert_eq!(fs::read_to_string(root.join("f.rs")).unwrap(), "if a < b {}", "by Drop");
    assert!(Applied::new(&root, &mutation("f.rs", "x", "y")).is_err(), "not applied");
    assert!(!root.join(BACKUP_DIR).join(BACKUP_PATH).exists(), "nothing kept for a mutation not applied");
}

/// Verifies: [TOOL] R-TEST-9 a file a stopped run left mutated is written
/// back from its copy at the start of the next run; with no copy, nothing
/// is written.
#[test]
fn a_file_left_mutated_is_written_back_by_the_next_run() {
    let root = scratch("left-mutated");
    // What `Applied::new` leaves when the process is stopped from outside,
    // so neither `restore` nor `Drop` runs.
    fs::write(root.join("f.rs"), "if a <= b {}").unwrap();
    fs::create_dir_all(root.join(BACKUP_DIR)).unwrap();
    fs::write(root.join(BACKUP_DIR).join(BACKUP_CONTENTS), "if a < b {}").unwrap();
    fs::write(root.join(BACKUP_DIR).join(BACKUP_PATH), "f.rs").unwrap();
    restore_leftover(&root).unwrap();
    assert_eq!(fs::read_to_string(root.join("f.rs")).unwrap(), "if a < b {}");
    fs::write(root.join("f.rs"), "changed since").unwrap();
    restore_leftover(&root).unwrap();
    assert_eq!(fs::read_to_string(root.join("f.rs")).unwrap(), "changed since", "no copy, nothing written");
}

/// Verifies: [TOOL] R-TEST-9 a failed build is shown by its last lines.
#[test]
fn a_failed_build_shows_its_last_lines() {
    assert_eq!(tail("a\nb\nc", 2_usize), "b\nc");
    assert_eq!(tail("a", 5_usize), "a");
}
