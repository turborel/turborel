//! Unit tests: a package installed from its file, into a directory that
//! cannot take it, and one installed whose digests file was changed
//! after, each line of it refused (design document §13.3, §13.4).

use super::super::install::install_file;
use super::*;
use crate::error::Code;
use crate::plugin::errors::UNAVAILABLE;

/// A scratch directory `name`, a plug-in project DEMO 2.0.0 in its `src`,
/// and its package, unsigned.
fn project(name: &str) -> (PathBuf, Package) {
    let root = scratch(name);
    plugin(&root.join("src"), "DEMO", "2.0.0", "");
    let package = pack(&root.join("src"), &Toy).unwrap();
    (root, package)
}

/// Verifies: [EXTENSION] design document §13.3: a package is installed
/// from its file; a file missing is an I/O error, one that is no package
/// is refused (38T07), directories naming none refuse it (38T05), and a
/// first directory that is a file cannot be written, nothing left of
/// what was begun.
#[test]
fn a_package_is_installed_from_its_file() {
    let (root, package) = project("install-file");
    let file = root.join("demo.package");
    std::fs::write(&file, package.write()).unwrap();
    let dirs = Directories::parse(&root.join("plugins").display().to_string());
    let open = trust(&[], false);
    let dir = install_file(&file, &dirs, &Toy, &open).unwrap();
    assert_eq!(dir, root.join("plugins").join("DEMO").join("2.0.0"));
    assert_eq!(verify_installed(&dir, &Toy, &open).unwrap(), None);
    assert_eq!(install_file(&root.join("none.package"), &dirs, &Toy, &open).unwrap_err().code, Code::Io);
    std::fs::write(root.join("junk.package"), b"not a package").unwrap();
    assert_eq!(install_file(&root.join("junk.package"), &dirs, &Toy, &open).unwrap_err().sqlstate(), errors::CHANGED);
    let none = install_package(&package, &Directories::parse(""), (&Toy, &open), Installed::Explicit).unwrap_err();
    assert_eq!(none.sqlstate(), UNAVAILABLE);
    std::fs::write(root.join("blocked"), b"a file").unwrap();
    let blocked = Directories::parse(&root.join("blocked").display().to_string());
    let unwritten = install_package(&package, &blocked, (&Toy, &open), Installed::Explicit).unwrap_err();
    assert_eq!(unwritten.code, Code::Io);
    std::fs::remove_dir_all(&root).unwrap();
}

/// Verifies: [EXTENSION] design document §13.4: an installed package's
/// digests file of another header, a line of no field, a signature of
/// one field or not hexadecimal, a digest not hexadecimal, a path that
/// leaves the package, a file it names that is gone, or none naming the
/// manifest, is refused at load, each 38T07 but the path's.
#[test]
fn a_changed_digests_file_is_refused() {
    let (root, package) = project("install-digests");
    let dirs = Directories::parse(&root.join("plugins").display().to_string());
    let open = trust(&[], false);
    let dir = install_package(&package, &dirs, (&Toy, &open), Installed::Explicit).unwrap();
    let file = dir.join(DIGESTS_FILE);
    let good = std::fs::read_to_string(&file).unwrap();
    let mut lines = good.lines();
    let header = lines.next().unwrap().to_owned();
    let entries: Vec<&str> = lines.collect();
    let run = entries.iter().find(|l| l.ends_with(" run")).unwrap();
    let digest = run.split_once(' ').unwrap().0;
    let changed = [
        "turborel-package 2\n".to_owned(),
        format!("{header}\nnospace\n"),
        format!("{header}\nsignature {digest}\n"),
        format!("{header}\nsignature zz zz\n"),
        format!("{header}\nzz run\n"),
        format!("{header}\n{digest} gone.bin\n"),
        format!("{header}\n{run}\n"),
    ];
    for text in &changed {
        std::fs::write(&file, text).unwrap();
        assert_eq!(verify_installed(&dir, &Toy, &open).unwrap_err().sqlstate(), errors::CHANGED, "{text}");
    }
    std::fs::write(&file, format!("{header}\n{digest} ../run\n")).unwrap();
    assert!(verify_installed(&dir, &Toy, &open).is_err(), "a path out of the package");
    std::fs::write(&file, &good).unwrap();
    assert_eq!(verify_installed(&dir, &Toy, &open).unwrap(), None);
    std::fs::remove_dir_all(&root).unwrap();
}
