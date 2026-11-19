//! The examples of the client's own commands, its backslash commands,
//! `turborel admin` and importing, run as printed.

use turborel::db::Database;
use turborel_cli::Dialer;
use turborel_client::agent::Agent;

use super::{Client, examples};
use crate::common::Scratch;

/// What `turborel admin` shows run as `invocation` says, nothing asked
/// of the terminal.
pub(super) fn administered(invocation: &turborel_cli::admin::Invocation) -> String {
    let connection = turborel_cli::connect(&invocation.options, || Ok(String::new())).unwrap();
    let mut agent = Agent::connected(Dialer::new(invocation.options.clone(), Box::new(|| Ok(String::new()))), connection);
    let mut out = Vec::new();
    let mut console = turborel_cli::admin::Console { out: &mut out, confirm: None, secret: None };
    let done = turborel_cli::admin::run(&mut agent, invocation, &mut console);
    agent.terminate().unwrap();
    let mut shown = String::from_utf8(out).unwrap();
    if let Err(e) = done {
        shown.extend(format!("turborel admin: {e}\n").chars());
    }
    shown
}

/// Verifies: [DESIGN] RULES.md R-DOC-8, D210 to D219: the chapter of the
/// client's backslash commands runs as printed, as ANN, the database's
/// owner, each file shown made.
#[test]
fn the_command_guide_runs_as_printed() {
    let db = Database::new();
    db.create_user("ANN", b"x").unwrap();
    db.set_owner("ANN").unwrap();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    let scratch = Scratch::new("manual-commands");
    let page = "client/05-turborel-commands.md";
    assert!(ann.run(page, &examples(page), Some(&scratch)) > 0, "the chapter shows its examples");
}

/// Verifies: [DESIGN] RULES.md R-DOC-8, D59: the Client Reference's
/// examples of importing run as printed, as ANN, each file shown made.
#[test]
fn the_import_examples_run_as_printed() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION ANN").unwrap();
    let mut ann = Client::of(&db, "ANN");
    let scratch = Scratch::new("manual-import");
    assert!(ann.section("client/01-turborel.md", "## Importing", &scratch) > 0, "the section shows its examples");
}
