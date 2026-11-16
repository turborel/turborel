//! What a condition is about, as GET DIAGNOSTICS gives it [ISO/IEC
//! 9075:1992 §18.1 GR3e–GR3i]: the constraint broken, the table or view,
//! the column, or the cursor.

/// \[EXTENSION\] [ISO/IEC 9075:1992 §18.1 GR3e–GR3i] The objects a
/// condition names: each by its schema's name and its own, the cursor by
/// its name. One it does not name is none, given as a zero-length string
/// \[GR3o\].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Subject {
    /// The constraint broken \[GR3f\].
    pub constraint: Option<(String, String)>,
    /// The table or view: the constraint's table \[GR3f\], the one a syntax
    /// error or access rule violation is about \[GR3g\], or the view whose
    /// CHECK OPTION a row breaks \[GR3i\].
    pub table: Option<(String, String)>,
    /// The column a syntax error or access rule violation is about \[GR3g\].
    pub column: Option<String>,
    /// The cursor in an invalid state \[GR3h\].
    pub cursor: Option<String>,
    /// Whether the constraint is an assertion, which GR3j iv shows to its
    /// schema's owner alone.
    pub assertion: bool,
    /// [ISO/IEC 9075-2:2023 §23.1 GR4g] The trigger whose triggered action
    /// failed, of a triggered action exception.
    pub trigger: Option<(String, String)>,
    /// [ISO/IEC 9075-2:2023 §23.1 GR4l, GR4q] The SQL-invoked routine of
    /// an SQL routine exception, or of an assignment to an SQL parameter.
    pub routine: Option<RoutineSubject>,
    /// [ISO/IEC 9075-4:2006 draft §17.2 GR2 to GR4] Of a condition SIGNAL
    /// or RESIGNAL raised, its condition identifier and the items it set.
    pub signalled: Option<Box<Signalled>>,
}

/// [ISO/IEC 9075-4:2006 draft §17.2 GR2 to GR4] What SIGNAL or RESIGNAL set
/// of the condition it raised.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Signalled {
    /// CONDITION_IDENTIFIER: the `<condition name>` signalled, else a
    /// zero-length string.
    pub identifier: String,
    /// Each `<condition information item name>` set, and its value.
    pub items: Vec<(&'static str, String)>,
    /// Whether a RESIGNAL of a `<signal value>` raised it ahead of the
    /// condition its handler handles [ISO/IEC 9075-4:2006 draft §17.3 GR5],
    /// which an area of one condition leaves out, MORE Y \[GR5b\].
    pub stacked: bool,
}

impl Signalled {
    /// The value item `name` was set to, if it was.
    pub fn item(&self, name: &str) -> Option<&str> {
        self.items.iter().find(|(n, _)| *n == name).map(|(_, v)| v.as_str())
    }
}

/// [ISO/IEC 9075-2:2023 §23.1 GR4l, GR4q] The SQL-invoked routine a
/// condition names, and the parameter it is related to, if any.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RoutineSubject {
    /// Its schema's name and its `<routine name>`'s identifier.
    pub name: (String, String),
    /// Its `<specific name>`'s identifier.
    pub specific: String,
    /// The parameter the condition is related to: its ordinal position,
    /// its `<parameter mode>` and its name.
    pub parameter: Option<(usize, &'static str, String)>,
}

impl Subject {
    /// The constraint `constraint` of the table `table` \[GR3f\].
    pub fn constraint(constraint: (String, String), table: (String, String)) -> Subject {
        Subject { constraint: Some(constraint), table: Some(table), ..Subject::default() }
    }

    /// The assertion `assertion` \[GR3f i\].
    pub fn assertion(assertion: (String, String)) -> Subject {
        Subject { constraint: Some(assertion), assertion: true, ..Subject::default() }
    }

    /// The constraint `constraint` of a domain, of no table \[GR3f i\].
    pub fn domain_constraint(constraint: (String, String)) -> Subject {
        Subject { constraint: Some(constraint), ..Subject::default() }
    }

    /// The table or view `table` \[GR3g, GR3i\].
    pub fn table(table: (String, String)) -> Subject {
        Subject { table: Some(table), ..Subject::default() }
    }

    /// [ISO/IEC 9075-2:2023 §23.1 GR4g] The trigger `trigger` of the table
    /// `table`.
    pub fn trigger(trigger: (String, String), table: (String, String)) -> Subject {
        Subject { trigger: Some(trigger), table: Some(table), ..Subject::default() }
    }

    /// [ISO/IEC 9075-2:2023 §23.1 GR4l] The SQL-invoked routine `name`,
    /// of the specific name `specific`.
    pub fn routine(name: (String, String), specific: (String, String)) -> Subject {
        Subject { routine: Some(RoutineSubject { name, specific: specific.1, parameter: None }), ..Subject::default() }
    }

    /// [ISO/IEC 9075-2:2023 §23.1 GR4q] The parameter `parameter`, its
    /// ordinal position, mode and name, of the SQL-invoked routine `name`
    /// of the specific name `specific`.
    pub fn parameter(name: (String, String), specific: (String, String), parameter: (usize, &'static str, String)) -> Subject {
        Subject { routine: Some(RoutineSubject { name, specific: specific.1, parameter: Some(parameter) }), ..Subject::default() }
    }

    /// The cursor `name` \[GR3h\].
    pub fn cursor(name: &str) -> Subject {
        Subject { cursor: Some(name.to_owned()), ..Subject::default() }
    }
}
