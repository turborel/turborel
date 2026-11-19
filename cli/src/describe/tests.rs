use super::*;

/// Verifies: [DESIGN] RULES.md D214: the describe commands are known with
/// their `+` and `S`, and nothing else is.
#[test]
fn describe_commands_are_known() {
    assert!(is_describe("d"));
    assert!(is_describe("dt+"));
    assert!(is_describe("dtS+"));
    assert!(is_describe("dS"));
    assert!(is_describe("dT"));
    assert!(!is_describe("dq"));
    assert!(!is_describe("S"));
    assert_eq!(modifiers("dvS+"), ("dv", true, true));
    assert_eq!(modifiers("dT"), ("dT", false, false));
    assert!(plan("x", None).is_err());
}

/// Verifies: [DESIGN] RULES.md D214: a pattern's undelimited part is
/// upper case, a delimited one as written; `*` and `?` are the wild cards
/// and `%`, `_` themselves; a period outside quotes splits the schema
/// from the name.
#[test]
fn patterns_become_like_patterns() {
    assert_eq!(like_part("st*f?"), "ST%F_");
    assert_eq!(like_part("\"Mixed_1\""), "Mixed\\_1");
    assert_eq!(like_part("a%b"), "A\\%B");
    assert_eq!(parts("s.t"), (Some("s"), "t"));
    assert_eq!(parts("\"a.b\""), (None, "\"a.b\""));
    assert_eq!(matching(None, ("S", "N"), true), "1 = 1");
    assert_eq!(matching(Some("x"), ("S", "N"), true), "N LIKE 'X' ESCAPE '\\'");
    assert!(matching(Some("s.x"), ("S", "N"), false).starts_with("S LIKE 'S' ESCAPE '\\' AND N LIKE 'X'"));
    assert!(matching(None, ("S", "N"), false).contains("NOT IN"));
}

/// Verifies: [DESIGN] RULES.md D214: `\d` with a pattern looks its tables
/// up first, then describes each in sections; none found is refused;
/// without one, it lists tables and views, then sequences if any.
#[test]
fn relations_are_looked_up_then_described() {
    let Ok(Plan::Relation(lookup)) = plan("d", Some("staff")) else { panic!("a lookup") };
    assert!(lookup.sql.contains("TABLE_NAME LIKE 'STAFF'"));
    assert!(resolved(&lookup, &[]).is_err());
    let row = vec![
        turborel::value::Value::Char("S".into()),
        turborel::value::Value::Char("STAFF".into()),
        turborel::value::Value::Char("VIEW".into()),
    ];
    let sections = resolved(&lookup, &[row]).unwrap();
    assert_eq!(sections[0].title.as_deref(), Some("View S.STAFF"));
    assert!(!sections[0].skip_empty);
    assert_eq!(sections[1].title.as_deref(), Some("Definition"));
    assert_eq!(sections.len(), 7);
    let Ok(Plan::Sections(listed)) = plan("d", None) else { panic!("sections") };
    assert_eq!(listed.len(), 2);
    assert!(listed[1].skip_empty);
}

/// Verifies: [DESIGN] RULES.md D214: each listing reads its own view of
/// the Information Schema, `+` adding columns and `S` the system's
/// schemas.
#[test]
fn listings_read_their_views() {
    let sql = |name: &str| match plan(name, Some("x")).unwrap() {
        Plan::Sections(s) => s[0].sql.clone(),
        Plan::Relation(_) => panic!("a listing"),
    };
    for (name, view) in [
        ("dt", "TABLES"),
        ("dv", "TABLES"),
        ("di", "INDEXES"),
        ("ds", "SEQUENCES"),
        ("df", "ROUTINES"),
        ("dn", "SCHEMATA"),
        ("du", "USERS"),
        ("dT", "USER_DEFINED_TYPES"),
        ("dD", "DOMAINS"),
        ("dx", "EXTENSIONS"),
        ("dG", "PROPERTY_GRAPHS"),
        ("dp", "TABLE_PRIVILEGES"),
        ("l", "INFORMATION_SCHEMA_CATALOG_NAME"),
    ] {
        assert!(sql(name).contains(&format!("INFORMATION_SCHEMA.{view} ")) || sql(name).ends_with(view), "{name}");
    }
    assert!(sql("dt+").contains("Insertable"));
    assert!(sql("dx+").contains("EXTENSION_PACKAGES") && sql("dx+").contains("\"License\""));
    assert!(!sql("dx").contains("License"));
    assert!(!sql("dt").contains("Insertable"));
    assert!(sql("dt").contains("NOT IN"));
    assert!(!sql("dtS").contains("NOT IN"));
    assert!(sql("dn").contains("NOT IN"));
    assert!(!sql("dnS").contains("NOT IN"));
}
