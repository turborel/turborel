//! Tests of `turborel extension`, the package manager of extensions
//! (design document §13, RULES.md D260 to D268): packages packed from
//! projects, signed, given as files, and installed, resolved, locked and
//! removed in plug-in directories of their own; there is no registry.

mod starter;

use std::path::{Path, PathBuf};

use turborel::plugin::package::crypto::Signer;
use turborel::plugin::package::hex;
use turborel_cli::extension::{parse, run};
use turborel_client::packages::SigningKey;

use crate::common::Scratch;

/// Runs `args` with paths taken from `cwd`: what it wrote, or why it
/// failed.
fn ext_in(cwd: &Path, args: &[&str]) -> Result<String, String> {
    let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
    let mut inv = parse(&args, &|_| None).unwrap().unwrap();
    inv.context.cwd = cwd.to_path_buf();
    let mut out = Vec::new();
    run(&inv, &mut out).map(|()| String::from_utf8(out).unwrap())
}

/// Writes the project of a plug-in of `name` and `version` under
/// `root/src`, `extra` manifest items after, its program already built.
fn source(root: &Path, name: &str, version: &str, extra: &str) -> PathBuf {
    let dir = root.join("src").join(format!("{name}-{version}"));
    std::fs::create_dir_all(dir.join("doc")).unwrap();
    let manifest =
        format!("NAME {name}; VERSION '{version}'; PROTOCOL 1; EXECUTABLE 'run'; FUNCTION {name}_F(INTEGER) RETURNS INTEGER; {extra}");
    std::fs::write(dir.join("turborel-plugin.manifest"), manifest).unwrap();
    std::fs::write(dir.join("run"), format!("program of {name} {version}")).unwrap();
    std::fs::write(dir.join("doc/README"), "read me").unwrap();
    dir
}

/// A signing key, written to `root/<name>.key`: its file and public key.
fn key(root: &Path, name: &str) -> (PathBuf, String) {
    let key = SigningKey::generate().unwrap();
    let file = root.join(format!("{name}.key"));
    std::fs::write(&file, key.file_text()).unwrap();
    (file, hex(&key.public_key()))
}

/// A server's plug-in directory, its options trusting `keys`, and a
/// directory of package files given to it.
struct Fixture {
    scratch: Scratch,
    plugins: PathBuf,
    files: PathBuf,
    options: Vec<String>,
}

impl Fixture {
    fn new(name: &str) -> Fixture {
        let scratch = Scratch::new(&format!("ext-{name}"));
        let (plugins, files) = (scratch.path("plugins"), scratch.path("files"));
        std::fs::create_dir_all(&plugins).unwrap();
        std::fs::create_dir_all(&files).unwrap();
        let options = vec!["--set".to_owned(), format!("PLUGINS.DIRECTORIES={}", plugins.display())];
        Fixture { scratch, plugins, files, options }
    }

    /// Packs a package into the files' directory: its file.
    fn pack(&self, name: &str, version: &str, extra: &str) -> String {
        let src = source(&self.scratch.dir, name, version, extra);
        let out = self.files.join(format!("{}-{version}.tbpkg", name.to_ascii_lowercase()));
        ext_in(&self.scratch.dir, &["pack", &src.display().to_string(), "--out", &out.display().to_string()]).unwrap();
        out.display().to_string()
    }

    fn on(&self, verb: &[&str]) -> Result<String, String> {
        let mut args: Vec<&str> = verb.to_vec();
        args.extend(self.options.iter().map(String::as_str));
        ext_in(&self.scratch.dir, &args)
    }
}

/// Verifies: [EXTENSION] design document §13.1, §13.4, RULES.md D264:
/// keygen writes a key, never over a file; pack builds a package of a
/// project signed by it; installed from its file with
/// PLUGINS.REQUIRE_SIGNED ON and the key trusted, it is unpacked into
/// `<directory>/<name>/<version>/`, verifies, and is locked as installed
/// by name; a file changed after fails verify (38T07); an unsigned
/// package, or one signed by a key not trusted, is refused (38T08); a
/// package file tampered with is refused (38T07).
#[test]
fn a_signed_package_is_packed_installed_and_verified() {
    let f = Fixture::new("pack");
    let root = &f.scratch.dir;
    let said = ext_in(root, &["keygen", "--out", "author.key"]).unwrap();
    let public = std::fs::read_to_string(root.join("author.key.pub")).unwrap().trim().to_owned();
    assert!(said.contains(&public), "{said}");
    assert!(ext_in(root, &["keygen", "--out", "author.key"]).is_err(), "never written over");
    let src = source(root, "DEMO", "1.0.0", "").display().to_string();
    let packed = ext_in(root, &["pack", &src, "--key", "author.key", "--out", "demo.tbpkg"]).unwrap();
    assert!(packed.contains(&format!("signed by {public}")) && packed.contains("no license declared"), "{packed}");
    assert!(packed.starts_with("warning: the manifest declares no LICENSE"), "{packed}");
    ext_in(root, &["pack", &src, "--out", "unsigned.tbpkg"]).unwrap();
    let mut strict = f.options.clone();
    strict.extend(["--set".to_owned(), format!("PLUGINS.TRUSTED_KEYS={public}"), "--set".to_owned(), "PLUGINS.REQUIRE_SIGNED=ON".to_owned()]);
    let on = |verb: &[&str], options: &[String]| ext_in(root, &[verb, &options.iter().map(String::as_str).collect::<Vec<_>>()].concat());
    assert!(on(&["install", "unsigned.tbpkg"], &strict).unwrap_err().contains("not signed"));
    let (_, other) = key(root, "other");
    let mut untrusting = f.options.clone();
    untrusting.extend(["--set".to_owned(), format!("PLUGINS.TRUSTED_KEYS={other}"), "--set".to_owned(), "PLUGINS.REQUIRE_SIGNED=ON".to_owned()]);
    assert!(on(&["install", "demo.tbpkg"], &untrusting).unwrap_err().contains("not in PLUGINS.TRUSTED_KEYS"));
    let mut tampered = std::fs::read(root.join("demo.tbpkg")).unwrap();
    let at = tampered.windows(10).position(|w| w == b"program of").unwrap();
    tampered[at] = b'P';
    std::fs::write(root.join("tampered.tbpkg"), tampered).unwrap();
    assert!(on(&["install", "tampered.tbpkg"], &strict).unwrap_err().contains("not what its digest says"));
    let said = on(&["install", "demo.tbpkg"], &strict).unwrap();
    assert_eq!(said, "installed DEMO 1.0.0 from demo.tbpkg, no license declared\n");
    let dir = f.plugins.join("DEMO").join("1.0.0");
    assert_eq!(std::fs::read_to_string(dir.join("run")).unwrap(), "program of DEMO 1.0.0");
    assert!(on(&["verify"], &strict).unwrap().contains(&format!("ok DEMO 1.0.0 signed by {public}")));
    assert!(on(&["list"], &strict).unwrap().contains("DEMO 1.0.0 explicit ^1.0.0"));
    std::fs::write(dir.join("run"), "changed").unwrap();
    assert!(on(&["verify"], &strict).unwrap_err().contains("is not what it was when installed"));
}

/// Verifies: [EXTENSION] design document §13.2, RULES.md D261: install
/// resolves among the files given, a directory of them, and the packages
/// installed. A file another given requires is a dependency, the newest
/// version in range chosen, and a file nothing chosen needs passed over;
/// why tells which package pulled a dependency in; a newer file of a
/// package asked for by name moves it on, from what is installed, and
/// leaves its old dependency to autoremove; remove refuses a package
/// another requires, naming it.
#[test]
fn install_resolves_among_the_files_given_and_installed() {
    let f = Fixture::new("resolve");
    f.pack("BASE", "1.0.0", "");
    f.pack("BASE", "1.2.0", "");
    let demo = f.pack("DEMO", "1.0.0", "REQUIRES BASE '^1.0.0';");
    let old = f.pack("OLD", "0.9.0", "REQUIRES BASE '<1.0.0';");
    std::fs::remove_file(&old).unwrap();
    let files = f.files.display().to_string();
    let said = f.on(&["install", &files]).unwrap();
    assert!(said.contains("installed BASE 1.2.0 from") && said.contains("installed DEMO 1.0.0 from"), "{said}");
    assert!(said.contains("passed over BASE 1.0.0"), "{said}");
    let listed = f.on(&["list"]).unwrap();
    assert!(listed.contains("BASE 1.2.0 dependency") && listed.contains("DEMO 1.0.0 explicit ^1.0.0"), "{listed}");
    assert!(f.on(&["why", "base"]).unwrap().contains("BASE is required by DEMO 1.0.0 (^1.0.0)"));
    assert!(f.on(&["remove", "base"]).unwrap_err().contains("required by DEMO 1.0.0"));
    std::fs::remove_file(&demo).unwrap();
    let newer = f.pack("DEMO", "2.0.0", "");
    let moved = f.on(&["install", &newer]).unwrap();
    assert!(moved.contains("installed DEMO 2.0.0"), "{moved}");
    assert!(f.on(&["list"]).unwrap().contains("DEMO 2.0.0 explicit ^2.0.0"));
    let removed = f.on(&["autoremove"]).unwrap();
    assert!(removed.contains("removed BASE 1.2.0") && removed.contains("removed DEMO 1.0.0"), "{removed}");
    assert!(f.plugins.join("DEMO").join("2.0.0").exists() && !f.plugins.join("BASE").join("1.2.0").exists());
    assert!(f.on(&["remove", "demo"]).unwrap().contains("removed DEMO 2.0.0"));
}

/// Verifies: [EXTENSION] RULES.md D261: a requirement neither the files
/// given nor the packages installed meet is refused with SQLSTATE 38T09,
/// naming it and the package file to add; once that file is given too,
/// or installed before, the package installs; files that require each
/// other in a cycle are refused naming it; two files of one version with
/// other contents, and a directory of no package file, are refused.
#[test]
fn what_is_missing_is_refused_naming_the_file_to_add() {
    let f = Fixture::new("missing");
    let top = f.pack("TOP", "1.0.0", "REQUIRES UNITS '^2.0.0';");
    let refused = f.on(&["install", &top]).unwrap_err();
    assert!(refused.contains("38T09") && refused.contains("requires UNITS ^2.0.0"), "{refused}");
    assert!(refused.contains("add a package file of UNITS"), "{refused}");
    let units = f.pack("UNITS", "2.1.0", "");
    assert!(f.on(&["install", &units]).unwrap().contains("installed UNITS 2.1.0"));
    assert!(f.on(&["install", &top]).unwrap().contains("installed TOP 1.0.0"));
    let cycle = Fixture::new("cycle");
    let (a, b) = (cycle.pack("A", "1.0.0", "REQUIRES B '*';"), cycle.pack("B", "1.0.0", "REQUIRES A '*';"));
    assert!(cycle.on(&["install", &a, &b]).unwrap_err().contains("cycle"));
    let twin = cycle.scratch.path("twin.tbpkg");
    let other = source(&cycle.scratch.path("other"), "A", "1.0.0", "FUNCTION EXTRA(INTEGER) RETURNS INTEGER;");
    ext_in(&cycle.scratch.dir, &["pack", &other.display().to_string(), "--out", &twin.display().to_string()]).unwrap();
    assert!(cycle.on(&["install", &a, &twin.display().to_string()]).unwrap_err().contains("with other contents"));
    std::fs::create_dir_all(cycle.scratch.path("empty")).unwrap();
    assert!(cycle.on(&["install", "empty"]).unwrap_err().contains("holds no .tbpkg file"));
    assert!(cycle.on(&["install"]).unwrap_err().contains("name the package files"));
}

/// Verifies: [EXTENSION] design document §13.6, RULES.md D262, D267: the
/// lock exported carries each package's digest and license; install
/// --lock on another server installs exactly what it names from the
/// files given and keeps it as that server's lock, and keeps what is
/// installed already; a file of a locked version whose digest differs is
/// refused (38T07), and a locked package neither installed nor given is
/// refused naming it (38T09).
#[test]
fn the_lock_reproduces_a_set_from_files() {
    let f = Fixture::new("lock");
    f.pack("BASE", "1.0.0", "LICENSE 'MIT';");
    f.pack("DEMO", "1.0.0", "REQUIRES BASE '*'; LICENSE 'Apache-2.0 OR MIT';");
    let files = f.files.display().to_string();
    f.on(&["install", &files]).unwrap();
    f.on(&["lock", "export", "exported.lock"]).unwrap();
    let lock = std::fs::read_to_string(f.scratch.path("exported.lock")).unwrap();
    assert!(lock.contains("BASE 1.0.0 ") && lock.contains(" dependency\nlicense MIT\n"), "{lock}");
    assert!(lock.contains(" explicit ^1.0.0\nlicense Apache-2.0 OR MIT\n"), "{lock}");
    let other = f.scratch.path("other");
    std::fs::create_dir_all(&other).unwrap();
    let elsewhere = ["--set".to_owned(), format!("PLUGINS.DIRECTORIES={}", other.display())];
    let with = |verb: &[&str]| ext_in(&f.scratch.dir, &[verb, &elsewhere.iter().map(String::as_str).collect::<Vec<_>>()].concat());
    let said = with(&["install", "--lock", "exported.lock", &files]).unwrap();
    assert!(said.contains("installed BASE 1.0.0") && said.contains("installed DEMO 1.0.0"), "{said}");
    assert_eq!(std::fs::read_to_string(other.join("turborel-extensions.lock")).unwrap(), lock);
    assert!(with(&["install", "--lock", "exported.lock"]).unwrap().contains("kept DEMO 1.0.0"));
    let third = f.scratch.path("third");
    std::fs::create_dir_all(&third).unwrap();
    let at_third = ["--set".to_owned(), format!("PLUGINS.DIRECTORIES={}", third.display())];
    let third_on = |verb: &[&str]| ext_in(&f.scratch.dir, &[verb, &at_third.iter().map(String::as_str).collect::<Vec<_>>()].concat());
    let missing = third_on(&["install", "--lock", "exported.lock"]).unwrap_err();
    assert!(missing.contains("38T09") && missing.contains("add its package file"), "{missing}");
    f.pack("BASE", "1.0.0", "LICENSE 'MIT'; FUNCTION EXTRA(INTEGER) RETURNS INTEGER;");
    assert!(third_on(&["install", "--lock", "exported.lock", &files]).unwrap_err().contains("another digest than the lock's"));
}

/// Verifies: [EXTENSION] design document §13.5: the command line of
/// `turborel extension` takes its verbs, options and settings, from
/// --set before the environment, and refuses what it does not know, the
/// registry's settings and commands among them.
#[test]
fn the_extension_command_line() {
    let env = |name: &str| (name == "TURBOREL_PLUGINS_DIRECTORIES").then(|| "/env".to_owned());
    let args = |a: &[&str]| a.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    let inv = parse(&args(&["install", "x.tbpkg", "--set", "plugins.require_signed=on"]), &env).unwrap().unwrap();
    assert_eq!(inv.context.directories.paths(), [PathBuf::from("/env")]);
    assert!(inv.context.trust.require_signed);
    let set = parse(&args(&["new", "hello", "--kind", "metric", "--kind", "scalar", "--dir", "d", "--set", "PLUGINS.DIRECTORIES=/set"]), &env)
        .unwrap()
        .unwrap();
    assert_eq!((set.kinds.len(), set.dir.as_deref()), (2, Some(Path::new("d"))));
    assert_eq!(set.context.directories.paths(), [PathBuf::from("/set")]);
    assert!(parse(&args(&[]), &env).unwrap().is_none() && parse(&args(&["--help"]), &env).unwrap().is_none());
    let bad: [&[&str]; 6] = [
        &["nope"],
        &["publish", "dir"],
        &["search", "x"],
        &["list", "--set", "PLUGINS.REGISTRIES=https://r/"],
        &["list", "--registry-dir", "r"],
        &["lock", "import"],
    ];
    for b in bad {
        assert!(parse(&args(b), &env).is_err(), "{b:?}");
    }
    assert!(matches!(turborel_cli::parse(&args(&["extension", "list"]), &env).unwrap(), turborel_cli::Command::Extension(_)));
}
