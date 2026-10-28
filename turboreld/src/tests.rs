//! Unit tests of `turboreld`'s command line (RULES.md R-TEST-1, R-TEST-2).
//! Split from lib.rs (RULES.md R-SIZE-4).

use super::*;

fn args(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_owned).collect()
}

fn config(line: &str) -> Config {
    match parse(&args(line)).unwrap() {
        Command::Serve(config) => config,
        Command::Help | Command::Version | Command::User(..) => panic!("not a server"),
    }
}

/// Verifies: [DESIGN] RULES.md D36, D42: a database kept in a directory or
/// nowhere; settings to set, in order, each value a whole number, another
/// number, or else a word, an empty one too.
#[test]
fn the_command_line_names_the_database_and_its_settings() {
    let kept = config("--dir /var/db");
    assert_eq!(kept.store, Store::Dir(PathBuf::from("/var/db")));
    assert!(kept.settings.is_empty());
    let set = config("--memory --set SERVER.WORKERS=3 --set x=1.5 --set SESSION_END=rollback --set SERVER.HOST=");
    assert_eq!(set.store, Store::Memory);
    let given: Vec<String> = set.settings.iter().map(|(name, value)| format!("{name}={value}")).collect();
    assert_eq!(given, ["SERVER.WORKERS=3", "x=1.5E0", "SESSION_END=rollback", "SERVER.HOST="]);
    assert!(matches!(parse(&args("--memory --help")), Ok(Command::Help)));
    assert!(matches!(parse(&args("--version")), Ok(Command::Version)));
}

/// Verifies: [DESIGN] RULES.md D42: a command line is refused, saying why,
/// with no database named, two named, an option without its value, an
/// unknown option, or a setting not NAME=VALUE.
#[test]
fn a_wrong_command_line_says_what_is_wrong() {
    let wrong = |line: &str| parse(&args(line)).unwrap_err();
    assert!(wrong("--set A=1").contains("--dir or --memory"));
    assert!(wrong("--memory --dir x").contains("once"));
    assert!(wrong("--memory --memory").contains("once"));
    assert!(wrong("--dir").contains("--dir needs a value"));
    assert!(wrong("--memory --verbose").contains("--verbose is not an option"));
    assert!(wrong("--memory --set SERVER.WORKERS").contains("NAME=VALUE"));
}

/// Verifies: [DESIGN] RULES.md D43: `user` and the words after it are a
/// user command, on the database named before; each command with its
/// name, the owner asked for or not.
#[test]
fn user_commands_are_read_after_the_database() {
    let Command::User(config, command) = parse(&args("--dir /db --set SERVER.PASSWORD_ITERATIONS=10000 user add ann --owner")).unwrap()
    else {
        panic!("a user command")
    };
    assert_eq!((config.store, config.settings.len()), (Store::Dir(PathBuf::from("/db")), 1));
    assert_eq!(command, UserCommand::Add { name: "ann".into(), owner: true });
    let read = |line: &str| parse_user(&args(line));
    assert_eq!(read("add bob"), Ok(UserCommand::Add { name: "bob".into(), owner: false }));
    assert_eq!(read("password bob"), Ok(UserCommand::Password { name: "bob".into() }));
    assert_eq!(read("drop bob"), Ok(UserCommand::Drop { name: "bob".into() }));
    assert_eq!(read("owner bob"), Ok(UserCommand::Owner { name: "bob".into() }));
    assert_eq!(read("list"), Ok(UserCommand::List));
    for wrong in ["", "add", "add a b", "remove bob", "list all", "add bob --admin"] {
        assert!(read(wrong).unwrap_err().contains("user takes"), "{wrong}");
    }
}
