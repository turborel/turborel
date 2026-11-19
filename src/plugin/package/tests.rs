//! Unit tests of packages: the archive, packing a project, installing
//! and checking one, resolution, licenses and the lock file, over a stand-in
//! for the cryptography (the real one is the protocol crate's, tested
//! there and by the command-line client's tests).

use std::path::{Path, PathBuf};

use super::archive::{EntryKind, Package};
use super::crypto::{Digest, PackageCrypto, PublicKey, Signature, Signer};
use super::install::{Installed, installed_how};
use super::lock::{Lock, Locked};
use super::project::{Project, pack_project};
use super::resolve::{Candidate, Origin, Request, Source, resolve};
use super::semver::{Range, Version};
use super::*;
use crate::plugin::limits::Directories;

/// A stand-in digest: four FNV-1a hashes of differing seeds.
fn toy(bytes: &[u8]) -> Digest {
    let mut out = [0_u8; 32];
    for (lane, chunk) in out.chunks_mut(8).enumerate() {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ u64::try_from(lane).unwrap();
        for b in bytes {
            h = (h ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3);
        }
        chunk.copy_from_slice(&h.to_le_bytes());
    }
    out
}

/// The stand-in cryptography: anyone with a public key could forge its
/// signatures, which a test of the checks around them does not mind.
struct Toy;

fn toy_signature(key: &PublicKey, message: &[u8]) -> Signature {
    let mut all = key.to_vec();
    all.extend_from_slice(message);
    let first = toy(&all);
    let mut out = [0_u8; 64];
    out[..32].copy_from_slice(&first);
    out[32..].copy_from_slice(&toy(&first));
    out
}

impl PackageCrypto for Toy {
    fn sha256(&self, bytes: &[u8]) -> Digest {
        toy(bytes)
    }
    fn verify(&self, key: &PublicKey, message: &[u8], signature: &Signature) -> bool {
        toy_signature(key, message) == *signature
    }
}

struct ToyKey(u8);

impl Signer for ToyKey {
    fn public_key(&self) -> PublicKey {
        toy(&[self.0])
    }
    fn sign(&self, message: &[u8]) -> Signature {
        toy_signature(&self.public_key(), message)
    }
}

/// A fresh directory for one test.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("turborel-package-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Writes a plug-in's directory: its manifest of `name`, `version` and
/// `extra` items, an executable `run`, a doc, a data file, an upgrade
/// script and a source file.
fn plugin(at: &Path, name: &str, version: &str, extra: &str) {
    std::fs::create_dir_all(at.join("doc")).unwrap();
    std::fs::create_dir_all(at.join("data")).unwrap();
    std::fs::create_dir_all(at.join("up")).unwrap();
    let manifest =
        format!("NAME {name}; VERSION '{version}'; PROTOCOL 1; EXECUTABLE 'run'; FUNCTION TWICE(INTEGER) RETURNS INTEGER; {extra}");
    std::fs::write(at.join(crate::plugin::MANIFEST_FILE), manifest).unwrap();
    std::fs::write(at.join("run"), b"program").unwrap();
    std::fs::write(at.join("doc/readme.txt"), b"docs").unwrap();
    std::fs::write(at.join("data/data.bin"), b"data").unwrap();
    std::fs::write(at.join("up/1.sql"), b"SELECT 1").unwrap();
    std::fs::write(at.join("main.rs"), b"fn main() {}").unwrap();
}

/// The package of the project in `dir`, its build in `dir/target/release`.
fn pack(dir: &Path, crypto: &dyn PackageCrypto) -> crate::error::Result<Package> {
    pack_project(Project { dir, built: &dir.join("target").join("release") }, crypto)
}

fn trust(keys: &[&ToyKey], require: bool) -> Trust {
    Trust { keys: keys.iter().map(|k| k.public_key()).collect(), require_signed: require }
}

/// Verifies: [EXTENSION] design document §13.1, RULES.md D264:
/// hexadecimal round trips; a package packed from a project holds the
/// manifest first and each file it names, and those under `doc/` and
/// `data/`, of its kind, and no source file; it writes and reads back
/// the same, signed or not; an
/// entry changed is refused with SQLSTATE 38T07, as is another format, a
/// second manifest or a path that leaves the package.
#[test]
fn a_package_packs_writes_and_reads_back() {
    assert_eq!(unhex(&hex(&[0, 1, 0xab, 0xff])).unwrap(), [0, 1, 0xab, 0xff]);
    assert!(unhex("abc").is_none() && unhex("zz").is_none());
    let dir = scratch("pack");
    plugin(&dir, "DEMO", "1.0.0", "UPGRADE FROM '0.9.x' SCRIPT 'up/1.sql';");
    let mut package = pack(&dir, &Toy).unwrap();
    let kinds: Vec<(&str, EntryKind)> = package.entries.iter().map(|e| (e.path.as_str(), e.kind)).collect();
    assert_eq!(kinds[0], (crate::plugin::MANIFEST_FILE, EntryKind::Manifest));
    assert!(kinds.contains(&("run", EntryKind::Executable)) && kinds.contains(&("doc/readme.txt", EntryKind::Doc)));
    assert!(kinds.contains(&("up/1.sql", EntryKind::Upgrade)) && kinds.contains(&("data/data.bin", EntryKind::Data)));
    assert!(!kinds.iter().any(|(p, _)| *p == "main.rs"), "{kinds:?}");
    assert_eq!(Package::read(&package.write(), &Toy).unwrap(), package);
    package.sign(&ToyKey(1)).unwrap();
    let bytes = package.write();
    assert_eq!(Package::read(&bytes, &Toy).unwrap(), package);
    let mut tampered = bytes.clone();
    let at = tampered.windows(7).position(|w| w == b"program").unwrap();
    tampered[at] = b'P';
    assert_eq!(Package::read(&tampered, &Toy).unwrap_err().sqlstate(), errors::CHANGED);
    let mut other_format = bytes.clone();
    other_format[6] = 2;
    assert_eq!(Package::read(&other_format, &Toy).unwrap_err().sqlstate(), errors::CHANGED);
    for bad in ["../x", "/x", "a\\b", "a//b", "c:x", "./a"] {
        assert!(archive::check_path(bad).is_err(), "{bad}");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Verifies: [EXTENSION] design document §13.4: a package installs into
/// `<first directory>/<name>/<version>/` with its digests and state, and
/// checks clean; installing it again is kept, and other files of the same
/// version refused (38T07); a file changed after is refused at load
/// (38T07); unsigned, or signed by a key not trusted, is refused when a
/// signature is required (38T08), and its signer told when trusted.
#[test]
fn installing_checks_digests_and_signatures() {
    let root = scratch("install");
    let (source, dirs) = (root.join("src"), Directories::parse(&root.join("plugins").display().to_string()));
    std::fs::create_dir_all(root.join("plugins")).unwrap();
    plugin(&source, "DEMO", "1.2.0", "");
    let unsigned = pack(&source, &Toy).unwrap();
    let refused = install_package(&unsigned, &dirs, (&Toy, &trust(&[], true)), Installed::Explicit).unwrap_err();
    assert_eq!(refused.sqlstate(), errors::UNTRUSTED);
    let mut signed = unsigned.clone();
    signed.sign(&ToyKey(7)).unwrap();
    let untrusted = install_package(&signed, &dirs, (&Toy, &trust(&[&ToyKey(8)], true)), Installed::Explicit).unwrap_err();
    assert_eq!(untrusted.sqlstate(), errors::UNTRUSTED);
    let trusted = trust(&[&ToyKey(7)], true);
    let dir = install_package(&signed, &dirs, (&Toy, &trusted), Installed::Dependency).unwrap();
    assert_eq!(dir, root.join("plugins").join("DEMO").join("1.2.0"));
    assert_eq!(installed_how(&dir), Installed::Dependency);
    assert_eq!(verify_installed(&dir, &Toy, &trusted).unwrap(), Some(hex(&ToyKey(7).public_key())));
    assert_eq!(install_package(&signed, &dirs, (&Toy, &trusted), Installed::Explicit).unwrap(), dir);
    assert_eq!(installed_how(&dir), Installed::Explicit);
    std::fs::write(source.join("data/data.bin"), b"other").unwrap();
    let different = pack(&source, &Toy).unwrap();
    let again = install_package(&different, &dirs, (&Toy, &trust(&[], false)), Installed::Explicit).unwrap_err();
    assert_eq!(again.sqlstate(), errors::CHANGED);
    std::fs::write(dir.join("run"), b"changed").unwrap();
    assert_eq!(verify_installed(&dir, &Toy, &trusted).unwrap_err().sqlstate(), errors::CHANGED);
    assert_eq!(verify_installed(&source, &Toy, &trust(&[], false)).unwrap(), None);
    assert_eq!(verify_installed(&source, &Toy, &trusted).unwrap_err().sqlstate(), errors::UNTRUSTED);
    std::fs::remove_dir_all(&root).unwrap();
}

/// Verifies: [EXTENSION] design document §13.2: the upgrade scripts are
/// those whose range holds the old version, in the order of the lowest
/// version each range names.
#[test]
fn upgrade_scripts_run_in_version_order() {
    let text = "NAME DEMO; VERSION '2.0.0'; PROTOCOL 1; EXECUTABLE 'run'; \
                UPGRADE FROM '>=1.5.0 <2.0.0' SCRIPT 'up/c.sql'; UPGRADE FROM '>=1.0.0 <2.0.0' SCRIPT 'up/b.sql'; \
                UPGRADE FROM '<1.0.0' SCRIPT 'up/a.sql';";
    let manifest = crate::plugin::manifest::Manifest::parse(text).unwrap();
    assert_eq!(upgrade_scripts(&manifest, &Version::new(1, 6, 0)).unwrap(), ["up/b.sql", "up/c.sql"]);
    assert_eq!(upgrade_scripts(&manifest, &Version::new(1, 2, 0)).unwrap(), ["up/b.sql"]);
    assert_eq!(upgrade_scripts(&manifest, &Version::new(0, 9, 0)).unwrap(), ["up/a.sql"]);
}

/// Versions held in memory, for resolution's tests.
struct Fixed(Vec<Candidate>);

impl Source for Fixed {
    fn versions(&self, name: &str) -> crate::error::Result<Vec<Candidate>> {
        Ok(self.0.iter().filter(|c| c.name == name).cloned().collect())
    }
}

fn c(name: &str, version: &str, requires: &[(&str, &str)]) -> Candidate {
    Candidate {
        name: name.to_owned(),
        version: Version::parse(version).unwrap(),
        requires: requires.iter().map(|(n, r)| ((*n).to_owned(), Range::parse(r).unwrap())).collect(),
        turborel: None,
        origin: Origin::Installed(PathBuf::from(name)),
    }
}

fn solve(all: &[Candidate], roots: &[(&str, &str)], pinned: &[(&str, &str)]) -> crate::error::Result<Vec<String>> {
    let request = Request {
        roots: roots.iter().map(|(n, r)| ((*n).to_owned(), Range::parse(r).unwrap())).collect(),
        pinned: pinned.iter().map(|(n, v)| ((*n).to_owned(), Version::parse(v).unwrap())).collect(),
        turborel: Version::new(1, 0, 0),
    };
    Ok(resolve(&Fixed(all.to_vec()), &request)?.into_iter().map(|c| format!("{} {}", c.name, c.version)).collect())
}

/// Verifies: [EXTENSION] design document §13.2: resolution takes the
/// newest version in range, dependencies first; backtracks to an older
/// version when the newest conflicts; meets a diamond with one version;
/// keeps a version pinned; passes over a version whose TURBOREL range
/// leaves out this release and a prerelease not asked for; and refuses a
/// conflict, a missing package, naming the requirement and the file to
/// add, and a cycle with SQLSTATE 38T09.
#[test]
fn resolution_picks_the_newest_compatible_set() {
    let all = vec![
        c("APP", "1.0.0", &[("LIB", "^1.0.0"), ("UTIL", "*")]),
        c("APP", "2.0.0", &[("LIB", "^2.0.0"), ("UTIL", "<1.0.0")]),
        c("LIB", "1.4.0", &[]),
        c("LIB", "1.5.0", &[]),
        c("LIB", "2.0.0", &[("UTIL", ">=1.0.0")]),
        c("LIB", "1.6.0-beta.1", &[]),
        c("UTIL", "0.9.0", &[]),
        c("UTIL", "1.1.0", &[]),
    ];
    assert_eq!(solve(&all, &[("LIB", "^1.0.0")], &[]).unwrap(), ["LIB 1.5.0"]);
    // APP 2.0.0 needs LIB 2, which needs UTIL >= 1, which APP 2.0.0 refuses.
    assert_eq!(solve(&all, &[("APP", "*")], &[]).unwrap(), ["LIB 1.5.0", "UTIL 1.1.0", "APP 1.0.0"]);
    assert_eq!(solve(&all, &[("APP", "*")], &[("LIB", "1.4.0")]).unwrap(), ["LIB 1.4.0", "UTIL 1.1.0", "APP 1.0.0"]);
    let diamond = vec![
        c("TOP", "1.0.0", &[("L", "*"), ("R", "*")]),
        c("L", "1.0.0", &[("BASE", "^1.2")]),
        c("R", "1.0.0", &[("BASE", "<1.4")]),
        c("BASE", "1.3.0", &[]),
        c("BASE", "1.4.0", &[]),
    ];
    assert_eq!(solve(&diamond, &[("TOP", "*")], &[]).unwrap(), ["BASE 1.3.0", "L 1.0.0", "R 1.0.0", "TOP 1.0.0"]);
    let mut limited = all.clone();
    limited[3].turborel = Some(Range::parse(">=2.0.0").unwrap());
    assert_eq!(solve(&limited, &[("LIB", "^1.0.0")], &[]).unwrap(), ["LIB 1.4.0"]);
    let refusals = [
        solve(&all, &[("LIB", "^1.0.0"), ("APP", "=2.0.0")], &[]),
        solve(&all, &[("NOPE", "*")], &[]),
        solve(&all, &[("LIB", "^1.0.0")], &[("LIB", "2.0.0")]),
        solve(&[c("A", "1.0.0", &[("B", "*")]), c("B", "1.0.0", &[("A", "*")])], &[("A", "*")], &[]),
    ];
    for refused in refusals {
        assert_eq!(refused.unwrap_err().sqlstate(), errors::CONFLICT);
    }
    let missing = solve(&all, &[("APP", "*"), ("NOPE", "^1.2.0")], &[]).unwrap_err();
    assert!(missing.msg.contains("requires NOPE ^1.2.0") && missing.msg.contains("add a package file of NOPE"), "{}", missing.msg);
    let cycle = solve(&[c("A", "1.0.0", &[("B", "*")]), c("B", "1.0.0", &[("A", "*")])], &[("A", "*")], &[]).unwrap_err();
    assert!(cycle.msg.contains("A requires B requires A"), "{}", cycle.msg);
}

/// Verifies: [EXTENSION] design document §13.2: resolution over the
/// packages installed finds each `<directory>/<name>/<version>/`.
#[test]
fn installed_packages_resolve_by_name() {
    let root = scratch("resolve");
    plugin(&root.join("BASE").join("1.0.0"), "BASE", "1.0.0", "");
    plugin(&root.join("BASE").join("1.1.0"), "BASE", "1.1.0", "");
    plugin(&root.join("TOP").join("3.0.0"), "TOP", "3.0.0", "REQUIRES BASE '~1.0';");
    let dirs = Directories::parse(&root.display().to_string());
    let found = resolve_installed(&dirs, "TOP", None, &[]).unwrap();
    let names: Vec<String> = found.iter().map(|(n, v, _)| format!("{n} {v}")).collect();
    assert_eq!(names, ["BASE 1.0.0", "TOP 3.0.0"]);
    assert_eq!(found[1].2, root.join("TOP").join("3.0.0"));
    std::fs::remove_dir_all(&root).unwrap();
}

/// Verifies: [EXTENSION] design document §13.6, RULES.md D262, D267: a
/// lock writes and reads back, keeping the range of a package asked for
/// by name and each license, expression and file, of a package that
/// declares one; a lock of no license lines reads; one of another format,
/// or that begins with a license, is refused.
#[test]
fn a_lock_round_trips() {
    let mut lock = Lock::default();
    let wanted = Some(Range::parse("^1.0.0 || >=3.0.0").unwrap());
    let license = (Some("MIT OR Apache-2.0".to_owned()), Some("LICENSE".to_owned()));
    let top = Locked {
        name: "TOP".to_owned(),
        version: Version::new(1, 2, 0),
        digest: toy(b"a"),
        wanted,
        license: license.0,
        license_file: license.1,
    };
    lock.put(top);
    let base = Locked {
        name: "BASE".to_owned(),
        version: Version::new(1, 0, 0),
        digest: toy(b"b"),
        wanted: None,
        license: None,
        license_file: None,
    };
    lock.put(base.clone());
    assert_eq!(lock.packages[0].name, "BASE");
    let text = lock.write();
    assert!(text.contains("explicit ^1.0.0 || >=3.0.0\nlicense MIT OR Apache-2.0\nlicense file LICENSE\n"), "{text}");
    assert_eq!(Lock::parse(&text).unwrap(), lock);
    let unlicensed = format!("turborel-lock 1\nBASE 1.0.0 {} dependency\n", hex(&toy(b"b")));
    assert_eq!(Lock::parse(&unlicensed).unwrap().packages, [base]);
    assert!(Lock::parse("turborel-lock 2\n").is_err());
    assert!(Lock::parse("turborel-lock 1\nlicense MIT\n").is_err());
}

/// Verifies: [EXTENSION] RULES.md D265, D266, D268: a manifest's LICENSE
/// and LICENSE FILE are read, and the file is packed and its text read
/// back from the package installed by `package_license`; a LICENSE FILE
/// not in the project refuses the package (38T06), as do a LICENSE not of
/// the SPDX grammar, one written twice and a LICENSE FILE that leaves the
/// directory; a manifest of neither still reads, declaring no license.
#[test]
fn licenses_are_read_packed_and_shown() {
    let root = scratch("license");
    let (source, dirs) = (root.join("src"), Directories::parse(&root.join("plugins").display().to_string()));
    std::fs::create_dir_all(root.join("plugins")).unwrap();
    plugin(&source, "DEMO", "1.0.0", "LICENSE 'MIT OR Apache-2.0'; LICENSE FILE 'COPYING';");
    let missing = pack(&source, &Toy).unwrap_err();
    assert!(missing.sqlstate() == crate::plugin::errors::INVALID_MANIFEST && missing.msg.contains("COPYING"), "{}", missing.msg);
    std::fs::write(source.join("COPYING"), "the text").unwrap();
    let package = pack(&source, &Toy).unwrap();
    assert!(package.entries.iter().any(|e| e.path == "COPYING" && e.kind == EntryKind::Doc));
    let dir = install_package(&package, &dirs, (&Toy, &trust(&[], false)), Installed::Explicit).unwrap();
    let license = package_license(&dir).unwrap();
    assert_eq!(license.expression.as_deref(), Some("MIT OR Apache-2.0"));
    assert_eq!((license.file.as_deref(), license.text.as_deref()), (Some("COPYING"), Some("the text")));
    assert_eq!(license.told(), "license MIT OR Apache-2.0 (text in COPYING)");
    let manifest = |extra: &str| crate::plugin::manifest::Manifest::parse(&format!("NAME D; PROTOCOL 1; EXECUTABLE 'run'; {extra}"));
    assert!(!License::of(&manifest("").unwrap()).declared());
    assert_eq!(License::default().told(), "no license declared");
    for bad in ["LICENSE 'MIT or GPL-2.0';", "LICENSE 'MIT'; LICENSE 'MIT';", "LICENSE FILE '../COPYING';"] {
        assert_eq!(manifest(bad).unwrap_err().sqlstate(), crate::plugin::errors::INVALID_MANIFEST, "{bad}");
    }
    std::fs::remove_dir_all(&root).unwrap();
}
