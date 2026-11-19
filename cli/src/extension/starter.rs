//! \[EXTENSION\] `turborel extension new` (RULES.md D263): a starter
//! project of a plug-in on the plug-in SDK, a crate of its own that
//! builds as it is written, serves an object of each kind asked for,
//! tests them by the SDK's harness, and packs with `turborel extension
//! pack`. The project depends on this release's crates by version, or on
//! a turborel source tree by path with `--sdk`.

mod kinds;

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::io::Write;
use std::path::{Path, PathBuf};

use turborel::plugin::MANIFEST_FILE;

use super::args::Invocation;
use super::say;
use kinds::{DEFAULT_KIND, STARTERS, Starter};

/// The version a starter begins at.
const FIRST_VERSION: &str = "0.1.0";
/// The license a starter declares until its author chooses one.
const PLACEHOLDER_LICENSE: &str = "LicenseRef-Proprietary";
/// The file of its text.
const LICENSE_FILE: &str = "LICENSE";
/// The directory of the SDK's crate in a turborel source tree.
const SDK_CRATE: &str = "plugin";

/// A project being written: its name, its crate's, and its objects.
struct Starting<'a> {
    /// The package's NAME.
    name: String,
    /// The crate's and program's name.
    krate: String,
    /// The objects, in the order of their kinds.
    kinds: Vec<&'a Starter>,
}

/// Writes the starter project of the package `typed` into `--dir`, or a
/// directory of its name in lower case, which must not exist.
///
/// # Errors
///
/// Why it cannot, for the user.
pub(super) fn new(inv: &Invocation, typed: &str, out: &mut dyn Write) -> Result<(), String> {
    let starting = starting(typed, &inv.kinds)?;
    let dir = inv.dir.clone().unwrap_or_else(|| PathBuf::from(&starting.krate));
    let at = inv.context.at(&dir);
    if at.exists() {
        return Err(format!("{} is there already; new never writes over a directory", dir.display()));
    }
    let dependencies = dependencies(inv)?;
    let files = [
        ("Cargo.toml", cargo_toml(&starting, &dependencies)),
        ("src/main.rs", main_rs(&starting)),
        (MANIFEST_FILE, manifest(&starting)),
        (LICENSE_FILE, license_text(&starting)),
        ("README.md", readme(&starting)),
        (".gitignore", "/target\n*.tbpkg\n".to_owned()),
    ];
    for (path, text) in &files {
        let file = path.split('/').fold(at.clone(), |p, part| p.join(part));
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("cannot make {}: {e}", parent.display()))?;
        }
        std::fs::write(&file, text).map_err(|e| format!("cannot write {}: {e}", file.display()))?;
    }
    let served: Vec<&str> = starting.kinds.iter().map(|k| k.word).collect();
    say(out, &format!("made the project of {} in {}, serving {}", starting.name, dir.display(), served.join(", ")))?;
    let listed: Vec<&str> = files.iter().map(|(p, _)| *p).collect();
    say(out, &format!("  {}", listed.join(" ")))
}

/// The project `typed` names, of the kinds `words` name.
fn starting<'a>(typed: &str, words: &[String]) -> Result<Starting<'a>, String> {
    let first = typed.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
    let rest = typed.chars().skip(1).all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !first || !rest {
        return Err(format!("{typed} is not a plain identifier, a letter then letters, digits or _"));
    }
    let asked: Vec<&str> = if words.is_empty() { vec![DEFAULT_KIND] } else { words.iter().map(String::as_str).collect() };
    let mut kinds = Vec::new();
    for word in asked {
        let starter = STARTERS.iter().find(|s| s.word == word).ok_or_else(|| {
            let known: Vec<&str> = STARTERS.iter().map(|s| s.word).collect();
            format!("{word} is no kind of plug-in; --kind takes {}", known.join(", "))
        })?;
        kinds.push(starter);
    }
    kinds.sort_by_key(|k| k.code);
    kinds.dedup_by_key(|k| k.code);
    Ok(Starting { name: typed.to_ascii_uppercase(), krate: typed.to_ascii_lowercase(), kinds })
}

/// The project's dependencies on turborel and its SDK: by path into the
/// source tree of `--sdk`, else by this release's version.
fn dependencies(inv: &Invocation) -> Result<String, String> {
    let Some(sdk) = &inv.sdk else {
        let v = turborel::VERSION;
        return Ok(format!("turborel = \"={v}\"\nturborel-plugin = \"={v}\"\n"));
    };
    let tree = std::fs::canonicalize(inv.context.at(sdk)).map_err(|e| format!("the source tree {} cannot be found: {e}", sdk.display()))?;
    if !tree.join(SDK_CRATE).join("Cargo.toml").is_file() {
        return Err(format!("{} is no turborel source tree: it has no {SDK_CRATE}/Cargo.toml", sdk.display()));
    }
    let quoted = |p: &Path| p.display().to_string().replace('\\', "\\\\");
    Ok(format!("turborel = {{ path = \"{}\" }}\nturborel-plugin = {{ path = \"{}\" }}\n", quoted(&tree), quoted(&tree.join(SDK_CRATE))))
}

/// The project's Cargo.toml.
fn cargo_toml(s: &Starting<'_>, dependencies: &str) -> String {
    format!(
        "[package]\nname = \"{}\"\nversion = \"{FIRST_VERSION}\"\nedition = \"2024\"\nlicense-file = \"{LICENSE_FILE}\"\npublish = false\n\n\
         [dependencies]\n{dependencies}\n\
         # A project of its own, never a member of a workspace it is made inside.\n[workspace]\n",
        s.krate
    )
}

/// The project's manifest.
fn manifest(s: &Starting<'_>) -> String {
    let mut text = format!(
        "NAME {};\nVERSION '{FIRST_VERSION}';\nPROTOCOL 1;\nEXECUTABLE '{}{}';\nLICENSE '{PLACEHOLDER_LICENSE}';\nLICENSE FILE '{LICENSE_FILE}';\n",
        s.name,
        s.krate,
        std::env::consts::EXE_SUFFIX
    );
    for k in &s.kinds {
        let _ = writeln!(text, "{};", k.declaration);
    }
    text
}

/// The `use` lines of the objects of `s`, a line a module.
fn uses(s: &Starting<'_>) -> String {
    let mut modules: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let all = ["turborel_plugin::Answer", "turborel_plugin::Plugin", "turborel_plugin::serve"];
    for path in all.iter().chain(s.kinds.iter().flat_map(|k| k.uses)) {
        if let Some((module, item)) = path.rsplit_once("::") {
            modules.entry(module).or_default().insert(item);
        }
    }
    let mut text = String::new();
    for (module, items) in modules {
        let items: Vec<&str> = items.into_iter().collect();
        let listed = if let [one] = items.as_slice() { (*one).to_owned() } else { format!("{{{}}}", items.join(", ")) };
        let _ = writeln!(text, "use {module}::{listed};");
    }
    text
}

/// The project's program.
fn main_rs(s: &Starting<'_>) -> String {
    let mut text = format!(
        "//! The turborel plug-in {}, served over plug-in protocol 1 by the\n//! plug-in SDK: each object its manifest, \
         `{MANIFEST_FILE}`,\n//! declares, by its name.\n\n{}",
        s.name,
        uses(s)
    );
    for k in &s.kinds {
        let _ = write!(text, "\n{}", k.source);
    }
    let added = s.kinds.iter().fold(String::new(), |mut all, k| {
        let _ = write!(all, "\n        {}", k.added);
        all
    });
    let _ = write!(
        text,
        "\n/// The plug-in, each object by the name its manifest declares.\nfn plugin() -> Plugin {{\n    \
         Plugin::new(env!(\"CARGO_PKG_VERSION\")){added}\n}}\n\nfn main() {{\n    if serve(plugin()).is_err() {{\n        \
         std::process::exit(1);\n    }}\n}}\n{}",
        tests(s)
    );
    text
}

/// The program's tests, on the SDK's harness.
fn tests(s: &Starting<'_>) -> String {
    let codes: Vec<String> = s.kinds.iter().map(|k| k.code.to_string()).collect();
    let scalar = s.kinds.iter().any(|k| k.word == DEFAULT_KIND);
    let (value_use, twice) = if scalar {
        (
            "    use turborel::value::{Decimal, Value};\n",
            "\n    /// TWICE doubles 21.\n    #[test]\n    fn twice_doubles() {\n        let mut h = Harness::new(plugin());\n        \
             let got = h.scalar(\"TWICE\", &[vec![Value::Exact(Decimal::new(21, 0))]]).unwrap();\n        \
             assert!(matches!(got.as_slice(), [Value::Exact(d)] if d.units == 42 && d.scale == 0));\n    }\n",
        )
    } else {
        ("", "")
    };
    format!(
        "\n#[cfg(test)]\nmod tests {{\n{value_use}    use turborel_plugin::Harness;\n\n    use super::plugin;\n\n    \
         /// WELCOME names every kind the manifest declares.\n    #[test]\n    fn it_serves_every_kind_declared() {{\n        \
         assert_eq!(Harness::new(plugin()).kinds(\"{}\"), [{}]);\n    }}\n{twice}}}\n",
        s.name,
        codes.join(", ")
    )
}

/// The placeholder of the license's text.
fn license_text(s: &Starting<'_>) -> String {
    format!(
        "Copyright the authors of {}. All rights reserved.\n\n\
         This file is a placeholder. Replace it with the text of the license the\n\
         plug-in is offered under, and LICENSE in {MANIFEST_FILE} with\n\
         that license's SPDX expression, as MIT or Apache-2.0 OR MIT.\n",
        s.name
    )
}

/// The project's README.
fn readme(s: &Starting<'_>) -> String {
    let (name, krate) = (&s.name, &s.krate);
    let call = if s.kinds.iter().any(|k| k.word == DEFAULT_KIND) { "\n    VALUES (TWICE(21));" } else { "" };
    format!(
        "# {name}\n\nA plug-in of turborel, served by the plug-in SDK. Its objects are\ndeclared in `{MANIFEST_FILE}` and \
         served by `src/main.rs`.\n\n\
         ## Build\n\n    cargo build --release\n\n\
         ## Test\n\nThe tests run the plug-in on the SDK's harness, in their own process,\nwith no database.\n\n    cargo test\n\n\
         ## Pack\n\nChoose a license first: put its text in `{LICENSE_FILE}` and its SPDX\nexpression in the manifest's LICENSE.\n\n\
         \x20   turborel extension pack . --key author.key\n\n\
         This writes `{krate}-{FIRST_VERSION}.tbpkg`, signed if `--key` names a key\n\
         `turborel extension keygen --out author.key` made.\n\n\
         ## Install\n\nOn the server's machine, into its plug-in directory:\n\n\
         \x20   turborel extension install {krate}-{FIRST_VERSION}.tbpkg --set PLUGINS.DIRECTORIES=/srv/turborel/plugins\n\n\
         Then, as the database's owner:\n\n    CREATE EXTENSION {name};{call}\n"
    )
}
