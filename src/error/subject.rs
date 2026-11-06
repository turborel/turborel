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

    /// The cursor `name` \[GR3h\].
    pub fn cursor(name: &str) -> Subject {
        Subject { cursor: Some(name.to_owned()), ..Subject::default() }
    }
}
