//! \[EXTENSION\] Whether a path calls an item method of the session's
//! time zone [ISO/IEC 9075-2:2023 §9.46 GR11g ii 6]: `datetime()`, whose
//! string without a time zone is taken in it (`Path::zone`), and
//! `time_tz()` and `timestamp_tz()`, so a call of the path is no key part
//! of an index (RULES.md D92).

use super::{Accessor, Expr, Method, Path, Predicate};

impl Path {
    /// Whether it calls `datetime()`, with a template or none,
    /// `time_tz()` or `timestamp_tz()`. A path is nested at most
    /// `MAX_PATH_NESTING` deep (RULES.md D84), so the walk recurses
    /// without a count of its own.
    pub fn zoned(&self) -> bool {
        !untimed(&self.expr)
    }
}

/// Whether the path `e` calls none of those methods.
fn untimed(e: &Expr) -> bool {
    match e {
        Expr::Literal(_) | Expr::Context | Expr::Variable(_) | Expr::Current | Expr::Last => true,
        Expr::Signed(_, x) => untimed(x),
        Expr::Arithmetic(a, _, b) => untimed(a) && untimed(b),
        Expr::Access(x, accessor) => untimed(x) && accessor_untimed(accessor),
    }
}

/// `untimed` of an accessor.
fn accessor_untimed(accessor: &Accessor) -> bool {
    match accessor {
        Accessor::Member(_) | Accessor::AnyMember | Accessor::AnyElement => true,
        Accessor::Subscripts(each) => each.iter().all(|s| untimed(&s.from) && s.to.as_ref().is_none_or(untimed)),
        Accessor::Filter(p) => predicate_untimed(p),
        Accessor::Method(m) => !matches!(m, Method::Datetime | Method::DatetimeTemplate(_) | Method::TimeTz(_) | Method::TimestampTz(_)),
    }
}

/// `untimed` of a filter's predicate.
fn predicate_untimed(p: &Predicate) -> bool {
    match p {
        Predicate::Exists(e) | Predicate::LikeRegex(e, _) => untimed(e),
        Predicate::Compare(a, _, b) | Predicate::StartsWith(a, b) => untimed(a) && untimed(b),
        Predicate::IsUnknown(x) | Predicate::Not(x) => predicate_untimed(x),
        Predicate::And(a, b) | Predicate::Or(a, b) => predicate_untimed(a) && predicate_untimed(b),
    }
}
