//! A host plug-in's listener and package cryptography against a real
//! server and a real process of the example plug-in
//! (`plugin/examples/plugin_example`): a client served through the
//! program's own connections, its bytes relayed; and the program's
//! digest pinned and checked by the host's own cryptography (RULES.md
//! D229, D252, D254).

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use turborel::db::Database;
use turborel::plugin::package::hex;
use turborel::plugin::package::crypto::PackageCrypto;
use turborel::value::Value;
use turborel_client::connection::{self, Trust};
use turborel_server::packages::Crypto;

use crate::common::{Scratch, rows, start};

/// The example's program, as the workspace's build made it.
fn program() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let debug = exe.parent().and_then(Path::parent).unwrap();
    let built = debug.join("examples").join(format!("plugin_example{}", std::env::consts::EXE_SUFFIX));
    assert!(built.exists(), "{} is not built: cargo build -p turborel-plugin --example plugin_example", built.display());
    built
}

/// The host plug-in HOSTED of `objects` in `scratch`: its directory.
fn hosted(scratch: &Scratch, objects: &str) -> PathBuf {
    let dir = scratch.path("HOSTED");
    std::fs::create_dir_all(&dir).unwrap();
    let exe = format!("plugin_example{}", std::env::consts::EXE_SUFFIX);
    let manifest = format!("NAME HOSTED; VERSION '1.0.0'; PROTOCOL 1; EXECUTABLE '{exe}';\n{objects}");
    std::fs::write(dir.join("turborel-plugin.manifest"), manifest).unwrap();
    std::fs::copy(program(), dir.join(&exe)).unwrap();
    dir
}

/// A database of the host's cryptography, PLUGINS.HOST `spec`.
fn database(spec: &str) -> Database {
    let db = Database::new();
    db.set_package_crypto(std::sync::Arc::new(Crypto)).unwrap();
    db.set_setting("PLUGINS.HOST", &Value::Char(spec.to_owned())).unwrap();
    db
}

/// The port the program's process listens at, once it has written it.
fn port(dir: &Path) -> u16 {
    let until = Instant::now() + Duration::from_secs(10);
    loop {
        let written = std::fs::read_dir(dir).unwrap().filter_map(Result::ok).find(|e| e.file_name().to_string_lossy().ends_with(".port"));
        if let Some(port) = written.and_then(|e| std::fs::read_to_string(e.path()).ok()).and_then(|t| t.trim().parse().ok()) {
            return port;
        }
        assert!(Instant::now() < until, "the program listens nowhere");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Verifies: [EXTENSION] RULES.md D254: a server listens through a host
/// plug-in's listener beside its own: a client the program took is
/// served, its requests and replies relayed in RELAY calls, the server's
/// operator shown where it listens.
#[test]
fn a_client_is_served_through_a_host_plugins_listener() {
    let scratch = Scratch::new("host_listener");
    let dir = hosted(&scratch, "HOST LISTENER TCP;");
    let db = database(&format!("HOSTED={}", dir.display()));
    db.start_host_plugins().unwrap();
    let server = start(&db, &[]);
    assert!(server.names.iter().any(|n| n == "host plug-in HOSTED.TCP"), "{:?}", server.names);
    let at: SocketAddr = format!("127.0.0.1:{}", port(&dir)).parse().unwrap();
    let mut c = connection::connect(at, "X", &mut Trust).unwrap();
    assert_eq!(rows(&c.query("VALUES (1 + 1)").unwrap()), ["2"]);
    assert_eq!(rows(&c.query("VALUES ('relayed')").unwrap()), ["relayed"]);
    drop(c);
    server.stop();
}

/// Verifies: [EXTENSION] RULES.md D229, D252: a host plug-in bringing
/// package cryptography starts only with its program's digest pinned in
/// PLUGINS.HOST, checked by the host's own cryptography as each process
/// starts; a program of another digest is refused.
#[test]
fn package_cryptography_is_pinned_by_its_digest() {
    let scratch = Scratch::new("host_crypto");
    let dir = hosted(&scratch, "HOST PACKAGE CRYPTO DIGESTS;");
    let exe = dir.join(format!("plugin_example{}", std::env::consts::EXE_SUFFIX));
    let digest = hex(&Crypto.sha256(&std::fs::read(&exe).unwrap()));
    let db = database(&format!("HOSTED={}@{digest}", dir.display()));
    db.start_host_plugins().unwrap();
    let wrong = format!("HOSTED={}@{}", dir.display(), "0".repeat(64));
    let refused = database(&wrong).start_host_plugins().unwrap_err();
    assert!(refused.msg.contains("not the"), "{refused:?}");
}
