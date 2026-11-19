//! Tests of `turborel extension`, the package manager of extensions
//! (design document §13): packages packed, signed, published into a
//! registry directory, served over HTTP by the test's own process, and
//! installed, upgraded, locked and removed in plug-in directories of
//! their own.

mod served;

use std::path::{Path, PathBuf};

use served::Served;
use turborel::plugin::package::crypto::Signer;
use turborel::plugin::package::hex;
use turborel_cli::extension::{Invocation, parse, run};
use turborel_client::packages::SigningKey;

/// A directory of one test, empty.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("turborel-ext-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Writes the source of a plug-in of `name` and `version`, `extra`
/// manifest items after.
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

/// The invocation of `args`, plain HTTP allowed as only a test may.
fn invocation(args: &[&str]) -> Invocation {
    let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
    let mut inv = parse(&args, &|_| None).unwrap().unwrap();
    inv.context.allow_http = true;
    inv
}

/// Runs `args`: what it wrote, or why it failed.
fn ext(args: &[&str]) -> Result<String, String> {
    let mut out = Vec::new();
    run(&invocation(args), &mut out).map(|()| String::from_utf8(out).unwrap())
}

/// The `--set` options of a server whose plug-in directory is `dir`,
/// trusting `keys`, of `registries`.
fn server(dir: &Path, keys: &str, registries: &str) -> Vec<String> {
    let mut args = vec!["--set".to_owned(), format!("PLUGINS.DIRECTORIES={}", dir.display())];
    args.extend(["--set".to_owned(), format!("PLUGINS.TRUSTED_KEYS={keys}")]);
    args.extend(["--set".to_owned(), format!("PLUGINS.REGISTRIES={registries}")]);
    args
}

/// Runs `verb` with the `options` of a server.
fn on(options: &[String], verb: &[&str]) -> Result<String, String> {
    let mut args: Vec<&str> = verb.to_vec();
    args.extend(options.iter().map(String::as_str));
    ext(&args)
}

/// Publishes the source `dir` into `registry`, signed by `key`.
fn publish(dir: &Path, registry: &Path, key: &Path) {
    let (d, r, k) = (dir.display().to_string(), registry.display().to_string(), key.display().to_string());
    ext(&["publish", &d, "--registry-dir", &r, "--key", &k]).unwrap();
}

/// Verifies: [EXTENSION] design document §13.1, §13.4: keygen writes a
/// key; pack builds a package of a plug-in signed by it; installed from
/// its file with PLUGINS.REQUIRE_SIGNED ON and the key trusted, it is
/// unpacked into `<directory>/<name>/<version>/`, verifies, and is locked
/// as installed by name; a file changed after fails verify (38T07); an
/// unsigned package, or one signed by a key not trusted, is refused
/// (38T08); a package file tampered with is refused (38T07).
#[test]
fn a_signed_package_is_packed_installed_and_verified() {
    let root = scratch("pack");
    let key_file = root.join("author.key");
    let said = ext(&["keygen", "--output", &key_file.display().to_string()]).unwrap();
    let public = std::fs::read_to_string(root.join("author.key.pub")).unwrap().trim().to_owned();
    assert!(said.contains(&public), "{said}");
    assert!(ext(&["keygen", "--output", &key_file.display().to_string()]).is_err(), "never written over");
    let src = source(&root, "DEMO", "1.0.0", "");
    let (signed, unsigned) = (root.join("demo.tbpkg"), root.join("unsigned.tbpkg"));
    ext(&["pack", &src.display().to_string(), "--key", &key_file.display().to_string(), "--output", &signed.display().to_string()])
        .unwrap();
    ext(&["pack", &src.display().to_string(), "--output", &unsigned.display().to_string()]).unwrap();
    let plugins = root.join("plugins");
    std::fs::create_dir_all(&plugins).unwrap();
    let mut options = server(&plugins, &public, "");
    options.extend(["--set".to_owned(), "PLUGINS.REQUIRE_SIGNED=ON".to_owned()]);
    let refused = on(&options, &["install", &unsigned.display().to_string()]).unwrap_err();
    assert!(refused.contains("38T08") || refused.contains("not signed"), "{refused}");
    let (_, other) = key(&root, "other");
    let untrusting = server(&plugins, &other, "");
    let mut strict = untrusting.clone();
    strict.extend(["--set".to_owned(), "PLUGINS.REQUIRE_SIGNED=ON".to_owned()]);
    assert!(on(&strict, &["install", &signed.display().to_string()]).unwrap_err().contains("not in PLUGINS.TRUSTED_KEYS"));
    let mut tampered = std::fs::read(&signed).unwrap();
    let at = tampered.windows(10).position(|w| w == b"program of").unwrap();
    tampered[at] = b'P';
    std::fs::write(root.join("tampered.tbpkg"), tampered).unwrap();
    assert!(
        on(&options, &["install", &root.join("tampered.tbpkg").display().to_string()]).unwrap_err().contains("not what its digest says")
    );
    on(&options, &["install", &signed.display().to_string()]).unwrap();
    let dir = plugins.join("DEMO").join("1.0.0");
    assert_eq!(std::fs::read_to_string(dir.join("run")).unwrap(), "program of DEMO 1.0.0");
    assert!(on(&options, &["verify"]).unwrap().contains(&format!("ok DEMO 1.0.0 signed by {public}")));
    assert!(on(&options, &["list"]).unwrap().contains("DEMO 1.0.0 explicit ^1.0.0"));
    std::fs::write(dir.join("run"), "changed").unwrap();
    assert!(on(&options, &["verify"]).unwrap_err().contains("is not what it was when installed"));
    std::fs::remove_dir_all(&root).unwrap();
}

/// A registry of signed packages served over HTTP, and a server's
/// plug-in directory and options trusting it.
struct Fixture {
    root: PathBuf,
    registry: PathBuf,
    key: PathBuf,
    public: String,
    served: Served,
    plugins: PathBuf,
    options: Vec<String>,
}

impl Fixture {
    fn new(name: &str) -> Fixture {
        let root = scratch(name);
        let registry = root.join("registry");
        let (key, public) = key(&root, "registry");
        std::fs::create_dir_all(&registry).unwrap();
        let served = Served::new(&registry);
        let plugins = root.join("plugins");
        std::fs::create_dir_all(&plugins).unwrap();
        let options = server(&plugins, &public, &served.url);
        Fixture { root, registry, key, public, served, plugins, options }
    }

    fn publish(&self, name: &str, version: &str, extra: &str) {
        publish(&source(&self.root, name, version, extra), &self.registry, &self.key);
    }

    fn on(&self, verb: &[&str]) -> Result<String, String> {
        on(&self.options, verb)
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Verifies: [EXTENSION] design document §13.3, §13.6: publish writes
/// packages and a signed index into a registry directory; served over
/// HTTP it is searched, told of by info with versions, requirements and
/// signer, and installed from by name and range with what each package
/// requires, the newest versions in range; outdated tells newer versions;
/// upgrade honours the range and --latest goes beyond it; why tells which
/// package pulled a dependency in; a version that no longer requires a
/// dependency leaves it to autoremove.
#[test]
fn a_registry_installs_upgrades_and_autoremoves() {
    let f = Fixture::new("registry");
    f.publish("BASE", "1.0.0", "");
    f.publish("BASE", "1.2.0", "");
    f.publish("DEMO", "1.0.0", "REQUIRES BASE '^1.0.0';");
    f.publish("DEMO", "2.0.0", "");
    let found = f.on(&["search", "dem"]).unwrap();
    assert!(found.contains("DEMO 2.0.0") && !found.contains("BASE"), "{found}");
    let info = f.on(&["info", "demo"]).unwrap();
    assert!(info.contains("1.0.0 from") && info.contains("requires BASE ^1.0.0") && info.contains(&f.public), "{info}");
    let said = f.on(&["install", "demo@^1.0.0"]).unwrap();
    assert!(said.contains("installed BASE 1.2.0") && said.contains("installed DEMO 1.0.0"), "{said}");
    assert!(f.plugins.join("BASE").join("1.2.0").join("run").exists());
    let listed = f.on(&["list"]).unwrap();
    assert!(listed.contains("BASE 1.2.0 dependency") && listed.contains("DEMO 1.0.0 explicit ^1.0.0"), "{listed}");
    assert!(f.on(&["why", "base"]).unwrap().contains("BASE is required by DEMO 1.0.0 (^1.0.0)"));
    assert!(f.on(&["outdated"]).unwrap().contains("DEMO 1.0.0 1.0.0 2.0.0"));
    f.publish("DEMO", "1.1.0", "REQUIRES BASE '^1.0.0';");
    assert!(f.on(&["upgrade"]).unwrap().contains("installed DEMO 1.1.0"));
    assert!(f.on(&["upgrade", "demo", "--latest"]).unwrap().contains("installed DEMO 2.0.0"));
    assert!(f.on(&["list"]).unwrap().contains("DEMO 2.0.0 explicit ^2.0.0"));
    let removed = f.on(&["autoremove"]).unwrap();
    assert!(removed.contains("removed BASE 1.2.0") && removed.contains("removed DEMO 1.0.0"), "{removed}");
    assert!(!f.plugins.join("BASE").join("1.2.0").exists());
    assert!(f.plugins.join("DEMO").join("2.0.0").exists());
}

/// Verifies: [EXTENSION] design document §13.6: remove refuses a package
/// another installed requires, naming it, and removes one nothing does.
#[test]
fn remove_checks_dependants() {
    let f = Fixture::new("remove");
    f.publish("BASE", "1.0.0", "");
    f.publish("DEMO", "1.0.0", "REQUIRES BASE '1.x';");
    f.on(&["install", "demo"]).unwrap();
    assert!(f.on(&["remove", "base"]).unwrap_err().contains("required by DEMO 1.0.0"));
    assert!(f.on(&["remove", "demo"]).unwrap().contains("removed DEMO 1.0.0"));
    assert!(f.on(&["remove", "base"]).unwrap().contains("removed BASE 1.0.0"));
}

/// Verifies: [EXTENSION] design document §13.6: an index is refused once
/// it expires, when it was changed after it was signed, and when signed
/// by a key not trusted; a package whose file is not what the index's
/// digest says is refused (38T07); plain HTTP is refused unless a test
/// allows it.
#[test]
fn a_registry_index_must_be_signed_fresh_and_true() {
    let f = Fixture::new("index");
    f.publish("DEMO", "1.0.0", "");
    let (_, stranger) = key(&f.root, "stranger");
    let untrusting = server(&f.plugins, &stranger, &f.served.url);
    assert!(on(&untrusting, &["search", "demo"]).unwrap_err().contains("not in PLUGINS.TRUSTED_KEYS"));
    let index = std::fs::read_to_string(f.registry.join("index")).unwrap();
    std::fs::write(f.registry.join("index"), index.replace("DEMO 1.0.0", "DEMO 1.0.1")).unwrap();
    assert!(f.on(&["search", "demo"]).unwrap_err().contains("does not verify"));
    std::fs::write(f.registry.join("index"), &index).unwrap();
    let file = f.registry.join("demo-1.0.0.tbpkg");
    let good = std::fs::read(&file).unwrap();
    std::fs::write(&file, b"other bytes").unwrap();
    assert!(f.on(&["install", "demo"]).unwrap_err().contains("not what the registry's index says"));
    std::fs::write(&file, good).unwrap();
    let src = source(&f.root, "DEMO", "1.0.1", "");
    let (d, r, k) = (src.display().to_string(), f.registry.display().to_string(), f.key.display().to_string());
    ext(&["publish", &d, "--registry-dir", &r, "--key", &k, "--expires-days", "0"]).unwrap();
    assert!(f.on(&["search", "demo"]).unwrap_err().contains("expired"));
    let mut args = vec!["search"];
    args.extend(f.options.iter().map(String::as_str));
    let mut plain = invocation(&args);
    plain.context.allow_http = false;
    let mut out = Vec::new();
    assert!(run(&plain, &mut out).unwrap_err().contains("plain HTTP"));
}

/// Verifies: [EXTENSION] design document §13.6: registries are tried in
/// the order PLUGINS.REGISTRIES names them, and PLUGINS.PINS takes a
/// package from the registry it names.
#[test]
fn registries_have_priority_and_pins() {
    let f = Fixture::new("priority");
    f.publish("DEMO", "1.0.0", "");
    let second = f.root.join("second");
    publish(&source(&f.root, "DEMO", "3.0.0", ""), &second, &f.key);
    let other = Served::new(&second);
    let both = server(&f.plugins, &f.public, &format!("{};{}", f.served.url, other.url));
    assert!(on(&both, &["search", "demo"]).unwrap().contains("DEMO 1.0.0"));
    let mut pinned = both.clone();
    pinned.extend(["--set".to_owned(), format!("PLUGINS.PINS=DEMO={}", other.url)]);
    assert!(on(&pinned, &["install", "demo"]).unwrap().contains("installed DEMO 3.0.0"));
}

/// Verifies: [EXTENSION] design document §13.6: the lock exported is
/// installed exactly on another server by lock import, from the cache
/// with the registry gone, and by install with no package from the lock;
/// clean empties the cache, after which the registry is fetched again;
/// a lock whose digest the registry no longer has is refused.
#[test]
fn the_lock_reproduces_a_set_exactly() {
    let f = Fixture::new("lock");
    f.publish("BASE", "1.0.0", "");
    f.publish("DEMO", "1.0.0", "REQUIRES BASE '*';");
    f.on(&["install", "demo"]).unwrap();
    let lock = f.root.join("exported.lock");
    f.on(&["lock", "export", &lock.display().to_string()]).unwrap();
    let cache = f.plugins.join(".cache");
    let other = f.root.join("other");
    std::fs::create_dir_all(&other).unwrap();
    let mut elsewhere = server(&other, &f.public, "https://unreachable.invalid/");
    elsewhere.extend(["--cache".to_owned(), cache.display().to_string()]);
    let said = on(&elsewhere, &["lock", "import", &lock.display().to_string()]).unwrap();
    assert!(said.contains("installed BASE 1.0.0") && said.contains("installed DEMO 1.0.0"), "{said}");
    assert_eq!(std::fs::read_to_string(other.join("turborel-extensions.lock")).unwrap(), std::fs::read_to_string(&lock).unwrap());
    assert!(on(&elsewhere, &["install"]).unwrap().contains("kept DEMO 1.0.0"));
    f.on(&["clean"]).unwrap();
    assert!(!cache.exists());
    let third = f.root.join("third");
    std::fs::create_dir_all(&third).unwrap();
    std::fs::copy(&lock, third.join("turborel-extensions.lock")).unwrap();
    let before = f.served.requests.load(std::sync::atomic::Ordering::SeqCst);
    assert!(on(&server(&third, &f.public, &f.served.url), &["install"]).unwrap().contains("installed DEMO 1.0.0"));
    assert!(f.served.requests.load(std::sync::atomic::Ordering::SeqCst) > before);
    f.publish("DEMO", "1.0.0", "REQUIRES BASE '*'; FUNCTION EXTRA(INTEGER) RETURNS INTEGER;");
    let fourth = f.root.join("fourth");
    std::fs::create_dir_all(&fourth).unwrap();
    std::fs::copy(&lock, fourth.join("turborel-extensions.lock")).unwrap();
    let mut no_cache = server(&fourth, &f.public, &f.served.url);
    no_cache.extend(["--cache".to_owned(), f.root.join("empty-cache").display().to_string()]);
    assert!(on(&no_cache, &["install"]).unwrap_err().contains("another digest"));
}

/// Verifies: [EXTENSION] design document §13.2: a registry's packages that
/// require each other in a cycle are refused naming it, and requirements
/// no version meets are refused naming one (38T09).
#[test]
fn cycles_and_conflicts_are_refused() {
    let f = Fixture::new("conflict");
    f.publish("A", "1.0.0", "REQUIRES B '*';");
    f.publish("B", "1.0.0", "REQUIRES A '*';");
    assert!(f.on(&["install", "a"]).unwrap_err().contains("cycle"));
    f.publish("C", "1.0.0", "REQUIRES D '^2.0.0';");
    f.publish("D", "1.0.0", "");
    let refused = f.on(&["install", "c"]).unwrap_err();
    assert!(refused.contains("38T09") || refused.contains("requires D ^2.0.0"), "{refused}");
    f.publish("E", "1.0.0", "REQUIRES D '^1.0.0';");
    f.on(&["install", "e"]).unwrap();
    f.publish("D", "2.0.0", "");
    assert!(f.on(&["install", "c"]).unwrap_err().contains("38T09"));
}

/// Verifies: [EXTENSION] design document §13.5: the command line of
/// `turborel extension` takes its verbs, options and settings, from
/// --set before the environment, and refuses what it does not know.
#[test]
fn the_extension_command_line() {
    let env = |name: &str| (name == "TURBOREL_PLUGINS_REGISTRIES").then(|| "https://env/".to_owned());
    let args = |a: &[&str]| a.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    let inv = parse(&args(&["install", "x", "--set", "plugins.require_signed=on"]), &env).unwrap().unwrap();
    assert_eq!(inv.context.registries.urls, ["https://env/"]);
    assert!(inv.context.trust.require_signed && !inv.context.allow_http);
    let set = parse(&args(&["list", "--set", "PLUGINS.REGISTRIES=https://set/"]), &env).unwrap().unwrap();
    assert_eq!(set.context.registries.urls, ["https://set/"]);
    assert!(parse(&args(&[]), &env).unwrap().is_none() && parse(&args(&["--help"]), &env).unwrap().is_none());
    for bad in [&["nope"][..], &["list", "--allow-http"], &["list", "--set", "SERVER.PORT=1"], &["lock", "drop"]] {
        assert!(parse(&args(bad), &env).is_err(), "{bad:?}");
    }
    assert!(matches!(turborel_cli::parse(&args(&["extension", "list"]), &env).unwrap(), turborel_cli::Command::Extension(_)));
}
