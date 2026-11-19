//! \[DESIGN\] The describe commands (RULES.md D214) read what the
//! database holds from the Information Schema [ISO/IEC 9075-11:2006 draft
//! §5], so they show what any session of the user may see and need
//! nothing of the server's own.
//!
//! `\d` lists the tables, views and sequences, and `\d PATTERN` describes
//! each table or view it matches, giving its columns, with their types,
//! defaults and nullability, then its indexes, constraints, foreign keys, the
//! tables whose foreign keys reference it, and its triggers, each section
//! left out when it has no rows; a view's definition after its columns.
//! The rest list one kind of object each, as `\?` names them. A pattern
//! is `SCHEMA.NAME` or `NAME`, `*` any text and `?` one character; an
//! undelimited part is taken in upper case, as SQL takes a name, and a
//! delimited one, in double quotes, as written. `+` after a command shows
//! more columns, and `S` the system's own schemas too.

use turborel::storage::Row;

use crate::format::cell;
use crate::vars::literal;

/// The schemas of the system, left out unless `S` asks for them.
const SYSTEM_SCHEMAS: &str = "('INFORMATION_SCHEMA', 'DEFINITION_SCHEMA', 'FT_INFORMATION_SCHEMA', 'ST_DEFINITION_SCHEMA')";
/// The escape character of a pattern's LIKE.
const ESCAPE: char = '\\';
/// The commands, before their `+` and `S`.
const COMMANDS: [&str; 14] = ["d", "dt", "dv", "di", "ds", "df", "dn", "du", "dT", "dD", "dx", "dG", "dp", "l"];

/// What `+` and `S` ask a command to show, more columns and the system's
/// own schemas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Show {
    more: bool,
    system: bool,
}

/// A query whose rows are one section of what a command shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// What is written before its rows, if anything.
    pub title: Option<String>,
    /// The query.
    pub sql: String,
    /// Whether it is left out when it has no rows.
    pub skip_empty: bool,
}

impl Section {
    fn listing(sql: String) -> Section {
        Section { title: None, sql, skip_empty: false }
    }

    fn titled(title: &str, sql: String) -> Section {
        Section { title: Some(title.to_owned()), sql, skip_empty: true }
    }
}

/// The tables and views a pattern names are found first, then described.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lookup {
    /// The query of their schemas, names and types.
    pub sql: String,
    /// The pattern, as typed.
    pub pattern: String,
    /// Whether more is shown.
    pub more: bool,
}

/// What a command runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Plan {
    /// These sections, in turn.
    Sections(Vec<Section>),
    /// A lookup, then the sections of each table found.
    Relation(Lookup),
}

/// The command's name, without its `+` and `S`, and whether each is
/// given.
fn modifiers(name: &str) -> (&str, bool, bool) {
    let (rest, more) = name.strip_suffix('+').map_or((name, false), |r| (r, true));
    let base = rest.strip_suffix('S').filter(|b| COMMANDS.contains(b)).map_or((rest, false), |b| (b, true));
    (base.0, more, base.1)
}

/// Whether `name` is a describe command.
pub fn is_describe(name: &str) -> bool {
    COMMANDS.contains(&modifiers(name).0)
}

/// One part of a pattern as the LIKE pattern of a name. A delimited part
/// is taken as written, else in upper case; `*` and `?` are the wild
/// cards, `%` and `_` themselves.
fn like_part(part: &str) -> String {
    let (text, delimited) = match part.strip_prefix('"').and_then(|p| p.strip_suffix('"')) {
        Some(body) => (body.replace("\"\"", "\""), true),
        None => (part.to_owned(), false),
    };
    let mut out = String::new();
    for c in text.chars() {
        match c {
            '*' => out.push('%'),
            '?' => out.push('_'),
            '%' | '_' | ESCAPE => {
                out.push(ESCAPE);
                out.push(c);
            }
            other => out.extend(if delimited { vec![other] } else { other.to_uppercase().collect() }),
        }
    }
    out
}

/// The schema and name parts of `pattern`, split at a period outside
/// double quotes.
fn parts(pattern: &str) -> (Option<&str>, &str) {
    let mut quoted = false;
    for (at, c) in pattern.char_indices() {
        quoted ^= c == '"';
        if c == '.' && !quoted {
            return (pattern.get(..at), pattern.get(at.saturating_add(1)..).unwrap_or_default());
        }
    }
    (None, pattern)
}

/// The condition that `schema` and `name`, columns, match `pattern`, and
/// that `schema` is not the system's unless `system`.
fn matching(pattern: Option<&str>, (schema, name): (&str, &str), system: bool) -> String {
    let mut conditions = Vec::new();
    if let Some((in_schema, named)) = pattern.map(parts) {
        if let Some(s) = in_schema {
            conditions.push(format!("{schema} LIKE {} ESCAPE '{ESCAPE}'", literal(&like_part(s))));
        }
        conditions.push(format!("{name} LIKE {} ESCAPE '{ESCAPE}'", literal(&like_part(named))));
    }
    if !system {
        conditions.push(format!("{schema} NOT IN {SYSTEM_SCHEMAS}"));
    }
    if conditions.is_empty() { "1 = 1".to_owned() } else { conditions.join(" AND ") }
}

/// The plan of the command `name`, of `pattern`.
///
/// # Errors
///
/// It is not a describe command.
pub fn plan(name: &str, pattern: Option<&str>) -> Result<Plan, String> {
    let (base, more, system) = modifiers(name);
    let show = Show { more, system };
    let listing = |sql: String| Ok(Plan::Sections(vec![Section::listing(sql)]));
    match base {
        "d" => Ok(relations(pattern, show)),
        "dt" => listing(tables(pattern, "TABLE_TYPE = 'BASE TABLE'", show)),
        "dv" => listing(tables(pattern, "TABLE_TYPE = 'VIEW'", show)),
        "di" => listing(indexes(pattern, show)),
        "ds" => listing(sequences(pattern, show)),
        "df" => listing(routines(pattern, show)),
        "dn" => listing(schemas(pattern, system)),
        "du" => Ok(users(pattern)),
        "dT" => listing(types(pattern, system)),
        "dD" => listing(domains(pattern, system)),
        "dx" => listing(extensions(pattern, more)),
        "dG" => listing(graphs(pattern, show)),
        "dp" => listing(privileges(pattern, system)),
        "l" => listing("SELECT CATALOG_NAME AS \"Catalog\" FROM INFORMATION_SCHEMA.INFORMATION_SCHEMA_CATALOG_NAME".to_owned()),
        other => Err(format!("\\{other} is no describe command")),
    }
}

/// `\d` lists every table, view and sequence; with a pattern, it
/// describes each table or view it matches.
fn relations(pattern: Option<&str>, show: Show) -> Plan {
    let Some(named) = pattern else {
        let mut sections = vec![Section::listing(tables(None, "TABLE_TYPE IN ('BASE TABLE', 'VIEW')", show))];
        sections.push(Section::titled("Sequences", sequences(None, show)));
        return Plan::Sections(sections);
    };
    let sql = format!(
        "SELECT TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE FROM INFORMATION_SCHEMA.TABLES WHERE {} ORDER BY TABLE_SCHEMA, TABLE_NAME",
        matching(Some(named), ("TABLE_SCHEMA", "TABLE_NAME"), true)
    );
    Plan::Relation(Lookup { sql, pattern: named.to_owned(), more: show.more })
}

/// The sections of each table or view `rows` found, their schema, name
/// and type.
///
/// # Errors
///
/// None was found.
pub fn resolved(lookup: &Lookup, rows: &[Row]) -> Result<Vec<Section>, String> {
    if rows.is_empty() {
        return Err(format!("\\d: no table or view matches {}", lookup.pattern));
    }
    let mut sections = Vec::new();
    for row in rows {
        let text = |at: usize| row.get(at).map(|v| cell(v, "")).unwrap_or_default();
        let (schema, name, kind) = (text(0), text(1), text(2));
        sections.extend(described(&schema, &name, &kind, lookup.more));
    }
    Ok(sections)
}

/// The condition that the columns `schema` and `name` are the table's.
fn of(columns: (&str, &str), (schema, name): (&str, &str)) -> String {
    format!("{} = {} AND {} = {}", columns.0, literal(schema), columns.1, literal(name))
}

/// The sections of the table or view `name` of `schema`, of `kind`.
fn described(schema: &str, name: &str, kind: &str, more: bool) -> Vec<Section> {
    let table = (schema, name);
    let extra = if more { ", ORDINAL_POSITION AS \"Position\", CHARACTER_MAXIMUM_LENGTH AS \"Length\"" } else { "" };
    let columns = format!(
        "SELECT COLUMN_NAME AS \"Column\", DATA_TYPE AS \"Type\", IS_NULLABLE AS \"Nullable\", COLUMN_DEFAULT AS \"Default\"{extra} \
         FROM INFORMATION_SCHEMA.COLUMNS WHERE {} ORDER BY ORDINAL_POSITION",
        of(("TABLE_SCHEMA", "TABLE_NAME"), table)
    );
    let what = if kind == "VIEW" { "View" } else { "Table" };
    let mut sections = vec![Section { title: Some(format!("{what} {schema}.{name}")), sql: columns, skip_empty: false }];
    if kind == "VIEW" {
        let definition = format!(
            "SELECT VIEW_DEFINITION AS \"Definition\" FROM INFORMATION_SCHEMA.VIEWS WHERE {}",
            of(("TABLE_SCHEMA", "TABLE_NAME"), table)
        );
        sections.push(Section::titled("Definition", definition));
    }
    sections.extend(table_sections(table));
    sections
}

/// The indexes, constraints, foreign keys, references and triggers of
/// `table`.
fn table_sections(table: (&str, &str)) -> Vec<Section> {
    let indexes = format!(
        "SELECT INDEX_NAME AS \"Index\", METHOD_NAME AS \"Method\", IS_UNIQUE AS \"Unique\", COLUMN_NAMES AS \"Columns\" \
         FROM INFORMATION_SCHEMA.INDEXES WHERE {} ORDER BY INDEX_NAME",
        of(("TABLE_SCHEMA", "TABLE_NAME"), table)
    );
    let constraints = format!(
        "SELECT C.CONSTRAINT_NAME AS \"Constraint\", C.CONSTRAINT_TYPE AS \"Type\", K.CHECK_CLAUSE AS \"Check\" \
         FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS AS C LEFT JOIN INFORMATION_SCHEMA.CHECK_CONSTRAINTS AS K \
         ON K.CONSTRAINT_SCHEMA = C.CONSTRAINT_SCHEMA AND K.CONSTRAINT_NAME = C.CONSTRAINT_NAME \
         WHERE {} AND C.CONSTRAINT_TYPE <> 'FOREIGN KEY' ORDER BY C.CONSTRAINT_NAME",
        of(("C.TABLE_SCHEMA", "C.TABLE_NAME"), table)
    );
    let keys = format!(
        "SELECT R.CONSTRAINT_NAME AS \"Foreign key\", U.TABLE_SCHEMA AS \"References schema\", U.TABLE_NAME AS \"References\", \
         R.UPDATE_RULE AS \"On update\", R.DELETE_RULE AS \"On delete\" \
         FROM INFORMATION_SCHEMA.REFERENTIAL_CONSTRAINTS AS R JOIN INFORMATION_SCHEMA.TABLE_CONSTRAINTS AS F \
         ON F.CONSTRAINT_SCHEMA = R.CONSTRAINT_SCHEMA AND F.CONSTRAINT_NAME = R.CONSTRAINT_NAME \
         JOIN INFORMATION_SCHEMA.TABLE_CONSTRAINTS AS U \
         ON U.CONSTRAINT_SCHEMA = R.UNIQUE_CONSTRAINT_SCHEMA AND U.CONSTRAINT_NAME = R.UNIQUE_CONSTRAINT_NAME \
         WHERE {} ORDER BY R.CONSTRAINT_NAME",
        of(("F.TABLE_SCHEMA", "F.TABLE_NAME"), table)
    );
    let referenced = format!(
        "SELECT F.TABLE_SCHEMA AS \"Schema\", F.TABLE_NAME AS \"Referenced by\", R.CONSTRAINT_NAME AS \"Foreign key\" \
         FROM INFORMATION_SCHEMA.REFERENTIAL_CONSTRAINTS AS R JOIN INFORMATION_SCHEMA.TABLE_CONSTRAINTS AS F \
         ON F.CONSTRAINT_SCHEMA = R.CONSTRAINT_SCHEMA AND F.CONSTRAINT_NAME = R.CONSTRAINT_NAME \
         JOIN INFORMATION_SCHEMA.TABLE_CONSTRAINTS AS U \
         ON U.CONSTRAINT_SCHEMA = R.UNIQUE_CONSTRAINT_SCHEMA AND U.CONSTRAINT_NAME = R.UNIQUE_CONSTRAINT_NAME \
         WHERE {} ORDER BY F.TABLE_SCHEMA, F.TABLE_NAME",
        of(("U.TABLE_SCHEMA", "U.TABLE_NAME"), table)
    );
    let triggers = format!(
        "SELECT TRIGGER_NAME AS \"Trigger\", ACTION_TIMING AS \"Timing\", EVENT_MANIPULATION AS \"Event\", ACTION_ORIENTATION AS \"For each\" \
         FROM INFORMATION_SCHEMA.TRIGGERS WHERE {} ORDER BY TRIGGER_NAME",
        of(("EVENT_OBJECT_SCHEMA", "EVENT_OBJECT_TABLE"), table)
    );
    vec![
        Section::titled("Indexes", indexes),
        Section::titled("Constraints", constraints),
        Section::titled("Foreign keys", keys),
        Section::titled("Referenced by", referenced),
        Section::titled("Triggers", triggers),
    ]
}

/// The tables whose type `types`, a condition, allows.
fn tables(pattern: Option<&str>, types: &str, show: Show) -> String {
    let extra = if show.more { ", IS_INSERTABLE_INTO AS \"Insertable\", COMMIT_ACTION AS \"On commit\"" } else { "" };
    format!(
        "SELECT TABLE_SCHEMA AS \"Schema\", TABLE_NAME AS \"Name\", TABLE_TYPE AS \"Type\"{extra} FROM INFORMATION_SCHEMA.TABLES \
         WHERE {types} AND {} ORDER BY TABLE_SCHEMA, TABLE_NAME",
        matching(pattern, ("TABLE_SCHEMA", "TABLE_NAME"), show.system)
    )
}

/// The indexes.
fn indexes(pattern: Option<&str>, show: Show) -> String {
    let extra = if show.more { ", COLUMN_NAMES AS \"Columns\"" } else { "" };
    format!(
        "SELECT TABLE_SCHEMA AS \"Schema\", INDEX_NAME AS \"Name\", TABLE_NAME AS \"Table\", METHOD_NAME AS \"Method\", \
         IS_UNIQUE AS \"Unique\"{extra} FROM INFORMATION_SCHEMA.INDEXES WHERE {} ORDER BY TABLE_SCHEMA, INDEX_NAME",
        matching(pattern, ("TABLE_SCHEMA", "INDEX_NAME"), show.system)
    )
}

/// The sequences.
fn sequences(pattern: Option<&str>, show: Show) -> String {
    let extra = if show.more { ", MINIMUM_VALUE AS \"Minimum\", MAXIMUM_VALUE AS \"Maximum\", CYCLE_OPTION AS \"Cycles\"" } else { "" };
    format!(
        "SELECT SEQUENCE_SCHEMA AS \"Schema\", SEQUENCE_NAME AS \"Name\", DATA_TYPE AS \"Type\", START_VALUE AS \"Start\", \
         INCREMENT AS \"Increment\"{extra} FROM INFORMATION_SCHEMA.SEQUENCES WHERE {} ORDER BY SEQUENCE_SCHEMA, SEQUENCE_NAME",
        matching(pattern, ("SEQUENCE_SCHEMA", "SEQUENCE_NAME"), show.system)
    )
}

/// The routines.
fn routines(pattern: Option<&str>, show: Show) -> String {
    let extra =
        if show.more { ", ROUTINE_BODY AS \"Body\", EXTERNAL_LANGUAGE AS \"Language\", IS_DETERMINISTIC AS \"Deterministic\"" } else { "" };
    format!(
        "SELECT ROUTINE_SCHEMA AS \"Schema\", ROUTINE_NAME AS \"Name\", ROUTINE_TYPE AS \"Type\", DATA_TYPE AS \"Returns\"{extra} \
         FROM INFORMATION_SCHEMA.ROUTINES WHERE {} ORDER BY ROUTINE_SCHEMA, ROUTINE_NAME, SPECIFIC_NAME",
        matching(pattern, ("ROUTINE_SCHEMA", "ROUTINE_NAME"), show.system)
    )
}

/// The condition that the column `column` matches `pattern`, if given.
fn named(column: &str, pattern: Option<&str>) -> String {
    pattern.map_or_else(|| "1 = 1".to_owned(), |p| format!("{column} LIKE {} ESCAPE '{ESCAPE}'", literal(&like_part(p))))
}

/// The schemas.
fn schemas(pattern: Option<&str>, system: bool) -> String {
    let hidden = if system { String::new() } else { format!(" AND SCHEMA_NAME NOT IN {SYSTEM_SCHEMAS}") };
    let condition = format!("{}{hidden}", named("SCHEMA_NAME", pattern));
    format!(
        "SELECT SCHEMA_NAME AS \"Name\", SCHEMA_OWNER AS \"Owner\" FROM INFORMATION_SCHEMA.SCHEMATA WHERE {condition} ORDER BY SCHEMA_NAME"
    )
}

/// The users, then the roles enabled and those the user may grant.
fn users(pattern: Option<&str>) -> Plan {
    let (users, roles) = (named("USER_NAME", pattern), named("ROLE_NAME", pattern));
    Plan::Sections(vec![
        Section::listing(format!(
            "SELECT USER_NAME AS \"User\", IS_OWNER AS \"Owner\", CREATE_ROLE AS \"Creates roles\" FROM INFORMATION_SCHEMA.USERS \
             WHERE {users} ORDER BY USER_NAME"
        )),
        Section::titled(
            "Roles",
            format!(
                "SELECT GRANTEE AS \"Grantee\", ROLE_NAME AS \"Role\", IS_GRANTABLE AS \"Grantable\" FROM INFORMATION_SCHEMA.APPLICABLE_ROLES \
                 WHERE {roles} ORDER BY GRANTEE, ROLE_NAME"
            ),
        ),
    ])
}

/// The user-defined types.
fn types(pattern: Option<&str>, system: bool) -> String {
    format!(
        "SELECT USER_DEFINED_TYPE_SCHEMA AS \"Schema\", USER_DEFINED_TYPE_NAME AS \"Name\", USER_DEFINED_TYPE_CATEGORY AS \"Category\", \
         DATA_TYPE AS \"Source\", IS_FINAL AS \"Final\" FROM INFORMATION_SCHEMA.USER_DEFINED_TYPES WHERE {} \
         ORDER BY USER_DEFINED_TYPE_SCHEMA, USER_DEFINED_TYPE_NAME",
        matching(pattern, ("USER_DEFINED_TYPE_SCHEMA", "USER_DEFINED_TYPE_NAME"), system)
    )
}

/// The domains.
fn domains(pattern: Option<&str>, system: bool) -> String {
    format!(
        "SELECT DOMAIN_SCHEMA AS \"Schema\", DOMAIN_NAME AS \"Name\", DATA_TYPE AS \"Type\", DOMAIN_DEFAULT AS \"Default\" \
         FROM INFORMATION_SCHEMA.DOMAINS WHERE {} ORDER BY DOMAIN_SCHEMA, DOMAIN_NAME",
        matching(pattern, ("DOMAIN_SCHEMA", "DOMAIN_NAME"), system)
    )
}

/// The extension modules the server runs, with `more` their processes,
/// calls and license.
fn extensions(pattern: Option<&str>, more: bool) -> String {
    let names = named("EXTENSION_NAME", pattern);
    // The license is the package's, of the database's lock (RULES.md D267).
    let extra = if more {
        ", PROCESSES_RUNNING AS \"Running\", CALLS AS \"Calls\", CALLS_FAILED AS \"Calls failed\", \
         (SELECT P.LICENSE FROM INFORMATION_SCHEMA.EXTENSION_PACKAGES AS P WHERE P.PACKAGE_NAME = E.EXTENSION_NAME) AS \"License\""
    } else {
        ""
    };
    format!(
        "SELECT EXTENSION_NAME AS \"Name\", VERSION AS \"Version\", SIGNER AS \"Signer\"{extra} FROM INFORMATION_SCHEMA.EXTENSIONS AS E \
         WHERE {names} ORDER BY EXTENSION_NAME"
    )
}

/// The property graphs.
fn graphs(pattern: Option<&str>, show: Show) -> String {
    let extra = if show.more { ", PROPERTY_GRAPH_DEFINITION AS \"Definition\"" } else { "" };
    format!(
        "SELECT PROPERTY_GRAPH_SCHEMA AS \"Schema\", PROPERTY_GRAPH_NAME AS \"Name\", PROPERTY_GRAPH_MODE AS \"Mode\", IS_VALID AS \"Valid\"{extra} \
         FROM INFORMATION_SCHEMA.PROPERTY_GRAPHS WHERE {} ORDER BY PROPERTY_GRAPH_SCHEMA, PROPERTY_GRAPH_NAME",
        matching(pattern, ("PROPERTY_GRAPH_SCHEMA", "PROPERTY_GRAPH_NAME"), show.system)
    )
}

/// The privileges on tables.
fn privileges(pattern: Option<&str>, system: bool) -> String {
    format!(
        "SELECT TABLE_SCHEMA AS \"Schema\", TABLE_NAME AS \"Table\", GRANTEE AS \"Grantee\", PRIVILEGE_TYPE AS \"Privilege\", \
         IS_GRANTABLE AS \"Grantable\", GRANTOR AS \"Grantor\" FROM INFORMATION_SCHEMA.TABLE_PRIVILEGES WHERE {} \
         ORDER BY TABLE_SCHEMA, TABLE_NAME, GRANTEE, PRIVILEGE_TYPE",
        matching(pattern, ("TABLE_SCHEMA", "TABLE_NAME"), system)
    )
}

#[cfg(test)]
mod tests;
