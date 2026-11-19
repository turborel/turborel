//! Tests of reading backslash commands.

use super::*;

fn vars() -> Variables {
    let mut vars = Variables::default();
    vars.set("T", "staff");
    vars.set("Q", "it's \"x\"");
    vars
}

fn read(line: &str) -> Result<Command, String> {
    let (name, rest) = split_name(line);
    parse(name, arguments(rest, &vars())?, rest)
}

/// Verifies: [DESIGN] RULES.md D210, D212: a command's arguments are
/// words apart by spaces; a word in single quotes is taken as written, its
/// doubled quotes one; one in double quotes keeps them; `:NAME`, `:'NAME'`
/// and `:"NAME"` read a variable as text, a literal and an identifier,
/// `:{?NAME}` whether it is set; a variable not set, or a colon that
/// begins none, is kept as written.
#[test]
fn arguments_read_quotes_and_variables() {
    let v = vars();
    assert_eq!(arguments("a  'b c' \"D e\"", &v).unwrap(), ["a", "b c", "\"D e\""]);
    assert_eq!(arguments("'it''s'", &v).unwrap(), ["it's"]);
    assert_eq!(arguments(":T x:T :'Q' :\"Q\"", &v).unwrap(), ["staff", "xstaff", "'it''s \"x\"'", "\"it's \"\"x\"\"\""]);
    assert_eq!(arguments(":{?T} :{?NONE} :NONE :", &v).unwrap(), ["true", "false", ":NONE", ":"]);
    assert!(arguments("'open", &v).is_err());
    assert!(arguments("\"open", &v).is_err());
    assert_eq!(arguments("", &v).unwrap(), Vec::<String>::new());
}

/// Verifies: [DESIGN] RULES.md D210: each command is read by its name,
/// short or long, with its arguments; a name no command has, or wrong
/// arguments, is refused.
#[test]
fn commands_are_read_by_name() {
    assert_eq!(read("q"), Ok(Command::Quit));
    assert_eq!(read("quit"), Ok(Command::Quit));
    assert_eq!(read("set A 1 2"), Ok(Command::Set(Some(("A".into(), "12".into())))));
    assert_eq!(read("set"), Ok(Command::Set(None)));
    assert_eq!(read("unset A"), Ok(Command::Unset("A".into())));
    assert_eq!(read("ir sub/x.sql"), Ok(Command::Include { path: "sub/x.sql".into(), relative: true }));
    assert_eq!(read("g out.txt"), Ok(Command::Go { target: Target::File("out.txt".into()), expanded: false }));
    assert_eq!(read("o | sort -r"), Ok(Command::Output(Target::Program(vec!["sort".into(), "-r".into()]))));
    assert_eq!(read("G"), Ok(Command::Go { target: Target::Default, expanded: true }));
    assert_eq!(read("watch 0.5 3"), Ok(Command::Watch { every: Duration::from_millis(500), count: Some(3) }));
    assert_eq!(read("watch"), Ok(Command::Watch { every: Duration::from_secs(2), count: None }));
    assert_eq!(read("x auto"), Ok(Command::Expanded(Expansion::Auto)));
    assert_eq!(read("x"), Ok(Command::Expanded(Expansion::Toggle)));
    assert_eq!(read("x off"), Ok(Command::Expanded(Expansion::Set(false))));
    assert_eq!(read("timing on"), Ok(Command::Timing(Some(true))));
    assert_eq!(read("format border 2"), Ok(Command::Format(Some(("border".into(), Some("2".into()))))));
    assert_eq!(read("dt+ S*"), Ok(Command::Describe("dt+".into(), Some("S*".into()))));
    assert_eq!(read("c - BOB"), Ok(Command::Connect(None, Some("BOB".into()))));
    assert_eq!(read("export (SELECT 'a b' FROM T) out.csv csv"), Ok(Command::Export(vec!["(SELECT 'a b' FROM T)".into(), "out.csv".into(), "csv".into()])));
    assert_eq!(read("h CREATE TABLE"), Ok(Command::Syntax(Some("CREATE TABLE".into()))));
    assert!(read("nope").is_err());
    assert!(read("q now").is_err());
    assert!(read("unset").is_err());
    assert!(read("timing maybe").is_err());
    assert!(read("watch 1 2 3").is_err());
    assert!(read("watch x").is_err());
}

/// Verifies: [DESIGN] RULES.md D215: an interval is whole seconds and
/// at most three digits of fraction.
#[test]
fn intervals_are_read_in_seconds() {
    assert_eq!(seconds("2"), Ok(Duration::from_secs(2)));
    assert_eq!(seconds("1.25"), Ok(Duration::from_millis(1250)));
    assert_eq!(seconds(".5"), Ok(Duration::from_millis(500)));
    assert!(seconds("1.2345").is_err());
    assert!(seconds("-1").is_err());
    assert!(seconds("1.x").is_err());
}
