//! \[DESIGN\] The prompt (RULES.md D217): PROMPT before a statement,
//! PROMPT_MORE before the rest of one begun, each with escapes read as it
//! is written. `%n` writes the session's user, `%c` the catalog, `%s` the
//! default schema, `%#` `#` for the database's owner and `>` for anyone
//! else, `%x` `*` while a transaction is active, `%R` `>` before a
//! statement and `-` within one, `%S` the server's product and version,
//! and `%%` a percent sign. Any other `%` is written as it is.
//!
//! The catalog and whether the user is the owner are read from the
//! Information Schema, only for a prompt that shows them, and again after
//! a statement that may change them; the schema is the user's at first,
//! then what SET SCHEMA set. A status the protocol sends with each
//! statement, once it has one, takes the place of both (`Status`).

/// What begins an escape.
const ESCAPE: char = '%';

/// What the prompt shows of the session that is read from the server.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Status {
    /// The catalog.
    pub catalog: String,
    /// Whether the session's user is the database's owner.
    pub owner: bool,
}

/// What a prompt is written of.
#[derive(Debug, Clone, Default)]
pub struct Context<'a> {
    /// The session's user.
    pub user: &'a str,
    /// The server's product and version.
    pub server: &'a str,
    /// The default schema.
    pub schema: &'a str,
    /// What is read from the server, if it was.
    pub status: Option<&'a Status>,
    /// Whether a transaction is active.
    pub active: bool,
    /// Whether a statement is begun.
    pub pending: bool,
}

/// Whether `template` needs the status read from the server.
pub fn needs_status(template: &str) -> bool {
    template.contains("%c") || template.contains("%#")
}

/// `template` with its escapes read in `context`.
pub fn expand(template: &str, context: &Context<'_>) -> String {
    let mut out = String::new();
    let mut chars = template.chars();
    while let Some(c) = chars.next() {
        if c != ESCAPE {
            out.push(c);
            continue;
        }
        let Some(code) = chars.next() else {
            out.push(ESCAPE);
            break;
        };
        match code {
            'n' => out.push_str(context.user),
            'c' => out.push_str(context.status.map_or("", |s| s.catalog.as_str())),
            's' => out.push_str(context.schema),
            '#' => out.push(if context.status.is_some_and(|s| s.owner) { '#' } else { '>' }),
            'x' => out.push_str(if context.active { "*" } else { "" }),
            'R' => out.push(if context.pending { '-' } else { '>' }),
            'S' => out.push_str(context.server),
            '%' => out.push(ESCAPE),
            other => {
                out.push(ESCAPE);
                out.push(other);
            }
        }
    }
    out
}

/// \[DESIGN\] RULES.md D217: the default schema SET SCHEMA `value` sets,
/// `value` its literal's text, of a session of `user`. The text is
/// trimmed, then taken as the part after a catalog's name and period, a
/// delimited identifier's body, or a regular identifier in upper case.
pub fn schema_of(value: &str, user: &str) -> String {
    let trimmed = value.trim();
    let named = trimmed.rsplit_once('.').filter(|(_, s)| !s.ends_with('"') || s.starts_with('"')).map_or(trimmed, |(_, s)| s);
    if named.is_empty() {
        return user.to_owned();
    }
    match named.strip_prefix('"').and_then(|n| n.strip_suffix('"')) {
        Some(body) => body.replace("\"\"", "\""),
        None => named.to_uppercase(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D217: each escape is read, the user,
    /// catalog, schema, owner marker, transaction, continuation, server
    /// and percent; an unknown escape and a last `%` are written as they
    /// are; the status is needed only by `%c` and `%#`.
    #[test]
    fn escapes_are_read() {
        let status = Status { catalog: "TURBOREL".into(), owner: true };
        let mut context = Context { user: "ANN", server: "turborel 1", schema: "S", status: Some(&status), active: true, pending: false };
        assert_eq!(expand("%n@%c.%s%x%#%R %S %% %q %", &context), "ANN@TURBOREL.S*#> turborel 1 % %q %");
        context.status = None;
        context.active = false;
        context.pending = true;
        assert_eq!(expand("%c%x%#%R", &context), ">-");
        assert!(needs_status("%n%#"));
        assert!(needs_status("%c"));
        assert!(!needs_status("%n%s%x"));
    }

    /// Verifies: [DESIGN] RULES.md D217: the schema SET SCHEMA sets is its
    /// value trimmed, after a catalog's name, a delimited identifier's
    /// body, or a regular one in upper case.
    #[test]
    fn set_schema_values_name_schemas() {
        assert_eq!(schema_of(" sales ", "ANN"), "SALES");
        assert_eq!(schema_of("TURBOREL.sales", "ANN"), "SALES");
        assert_eq!(schema_of("\"Sales\"", "ANN"), "Sales");
        assert_eq!(schema_of("\"a.b\"", "ANN"), "a.b");
        assert_eq!(schema_of("", "ANN"), "ANN");
    }
}
