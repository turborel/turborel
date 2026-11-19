//! \[DESIGN\] The options of how results are written, of the variables
//! set before a run, and of a run as one transaction (RULES.md D212,
//! D213, D215, D219); and what the environment names for the session:
//! the pager, the editor and the history file.

use std::path::PathBuf;

use crate::format::{Expanded, Layout, Style};
use crate::vars;

/// The environment variable naming the pager, before `PAGER`.
pub const PAGER_VARIABLE: &str = "TURBOREL_PAGER";
/// The environment variable naming a pager for any program.
const PAGER: &str = "PAGER";
/// The environment variables naming the editor, in the order tried.
const EDITORS: [&str; 2] = ["TURBOREL_EDITOR", "EDITOR"];
/// The environment variable naming the history file.
pub const HISTORY_VARIABLE: &str = "TURBOREL_HISTORY";
/// The environment variables naming the user's home directory, in the
/// order tried.
const HOMES: [&str; 2] = ["HOME", "USERPROFILE"];
/// The history file in the home directory, unless one is named.
pub const HISTORY_FILE: &str = ".turborel_history";
/// The environment variable of the directories programs are found in.
const PATH: &str = "PATH";

/// What the environment names for a session.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Environment {
    /// The pager, if `TURBOREL_PAGER` or `PAGER` names one; empty for
    /// none.
    pub pager: Option<String>,
    /// The editor, if `TURBOREL_EDITOR` or `EDITOR` names one.
    pub editor: Option<String>,
    /// The history file: as `TURBOREL_HISTORY` names it, else
    /// `.turborel_history` in the home directory, if one is named.
    pub history: Option<PathBuf>,
    /// The directories programs are found in.
    pub path: Option<String>,
}

impl Environment {
    /// What `env` names.
    pub fn of(env: &dyn Fn(&str) -> Option<String>) -> Environment {
        let home = HOMES.iter().find_map(|h| env(h)).map(|h| PathBuf::from(h).join(HISTORY_FILE));
        Environment {
            pager: env(PAGER_VARIABLE).or_else(|| env(PAGER)),
            editor: EDITORS.iter().find_map(|e| env(e)),
            history: env(HISTORY_VARIABLE).map(PathBuf::from).or(home),
            path: env(PATH),
        }
    }
}

/// How results are written, and what is set before a run.
#[derive(Debug, Clone, Default)]
pub struct Settings {
    /// How results are written.
    pub layout: Layout,
    /// The variables `-v` set, in order.
    pub variables: Vec<(String, String)>,
    /// Whether the run is one transaction, committed if every statement
    /// succeeds, else rolled back.
    pub single_transaction: bool,
    /// What the environment names.
    pub environment: Environment,
}

/// Takes `arg`, an option of these settings, into `settings`, `value`
/// giving its value if it takes one: whether it was one.
///
/// # Errors
///
/// Its value is missing or wrong.
pub fn option(arg: &str, value: &mut dyn FnMut() -> Result<String, String>, settings: &mut Settings) -> Result<bool, String> {
    let layout = &mut settings.layout;
    match arg {
        "-v" | "--set" => settings.variables.push(assignment(&value()?)?),
        "-1" | "--single-transaction" => settings.single_transaction = true,
        "-A" | "--unaligned" => layout.style = Style::Unaligned,
        "--csv" => layout.style = Style::Csv,
        "--html" => layout.style = Style::Html,
        "--json" => layout.style = Style::Json,
        "-B" | "--tab-separated" => layout.style = Style::Tabs,
        "--style" => layout.set("style", &value()?)?,
        "--border" => layout.set("border", &value()?)?,
        "-t" | "--tuples-only" => layout.tuples_only = true,
        "-F" | "--separator" => layout.separator = value()?,
        "-x" | "--expanded" => layout.expanded = Expanded::On,
        "--auto-expanded" => layout.expanded = Expanded::Auto,
        "--safe-updates" => settings.variables.push((vars::SAFE_UPDATES.to_owned(), "on".to_owned())),
        "-e" | "--echo" => settings.variables.push((vars::ECHO.to_owned(), "queries".to_owned())),
        "-q" | "--quiet" => settings.variables.push((vars::QUIET.to_owned(), "on".to_owned())),
        _ => return Ok(false),
    }
    Ok(true)
}

/// The variable and value `NAME=VALUE` sets.
fn assignment(text: &str) -> Result<(String, String), String> {
    let (name, value) = text.split_once('=').unwrap_or((text, ""));
    if !vars::is_name(name) {
        return Err(format!("-v {text}: {name:?} is no variable's name"));
    }
    Ok((name.to_owned(), value.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D212, D213: output options set the
    /// layout, `-v` a variable and its value, empty if no `=`; a wrong
    /// name or value is refused; other arguments are not these options'.
    #[test]
    fn output_options_set_the_layout_and_variables() {
        let mut settings = Settings::default();
        let given = |text: &str| -> Result<String, String> { Ok(text.to_owned()) };
        for (arg, v) in [("-A", ""), ("-t", ""), ("-F", ";"), ("--border", "2"), ("-v", "A=1=2"), ("--set", "B"), ("-x", "")] {
            assert!(option(arg, &mut || given(v), &mut settings).unwrap(), "{arg}");
        }
        assert_eq!(settings.layout.style, Style::Unaligned);
        assert!(settings.layout.tuples_only);
        assert_eq!(settings.layout.separator, ";");
        assert_eq!(settings.layout.border, 2);
        assert_eq!(settings.layout.expanded, Expanded::On);
        assert_eq!(settings.variables, [("A".to_owned(), "1=2".to_owned()), ("B".to_owned(), String::new())]);
        assert!(option("-v", &mut || given("A-B=1"), &mut settings).is_err());
        assert!(option("--border", &mut || given("5"), &mut settings).is_err());
        assert!(!option("--host", &mut || given(""), &mut settings).unwrap());
    }

    /// Verifies: [DESIGN] RULES.md D211, D213: the pager is
    /// `TURBOREL_PAGER`'s, else `PAGER`'s; the history file
    /// `TURBOREL_HISTORY`'s, else `.turborel_history` in the home.
    #[test]
    fn the_environment_names_pager_editor_and_history() {
        let env = |name: &str| match name {
            "PAGER" => Some("more".to_owned()),
            "HOME" => Some("/home/ann".to_owned()),
            "EDITOR" => Some("ed".to_owned()),
            _ => None,
        };
        let found = Environment::of(&env);
        assert_eq!(found.pager.as_deref(), Some("more"));
        assert_eq!(found.editor.as_deref(), Some("ed"));
        assert_eq!(found.history, Some(PathBuf::from("/home/ann").join(HISTORY_FILE)));
        let named = |name: &str| (name == HISTORY_VARIABLE || name == PAGER_VARIABLE).then(|| "x".to_owned());
        let other = Environment::of(&named);
        assert_eq!((other.pager.as_deref(), other.history), (Some("x"), Some(PathBuf::from("x"))));
    }
}
