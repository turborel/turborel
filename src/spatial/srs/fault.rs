//! The exception conditions the engine raises, each one of the SQL/MM
//! Spatial subconditions of ISO/IEC 13249-3:2016 (DIS, SC32 N2593) §22
//! Table 16. The engine carries no SQLSTATE machinery of its own; the SQL
//! layer maps each [`Condition`] to its class 2F subclass.

use std::fmt;

/// A subcondition of §22 Table 16, class 2F, that this engine raises.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Condition {
    /// F02 invalid argument.
    InvalidArgument,
    /// F13 attempted division by zero.
    DivisionByZero,
    /// F14 unsupported unit specified.
    UnsupportedUnit,
    /// F15 failed to transform geometry.
    FailedToTransform,
    /// F17 empty point value.
    EmptyPoint,
    /// F18 point value not well formed.
    PointNotWellFormed,
    /// F19 points are equal.
    PointsEqual,
    /// F20 linestring is not a line.
    NotALine,
    /// F21 degenerate line has no direction.
    DegenerateLine,
    /// F22 invalid well-known text representation.
    InvalidText,
    /// F24 invalid GML representation.
    InvalidGml,
    /// F59 unknown spatial reference system.
    UnknownSystem,
}

impl Condition {
    /// The subclass of §22 Table 16, after the class value 2F.
    pub fn subclass(self) -> &'static str {
        match self {
            Condition::InvalidArgument => "F02",
            Condition::DivisionByZero => "F13",
            Condition::UnsupportedUnit => "F14",
            Condition::FailedToTransform => "F15",
            Condition::EmptyPoint => "F17",
            Condition::PointNotWellFormed => "F18",
            Condition::PointsEqual => "F19",
            Condition::NotALine => "F20",
            Condition::DegenerateLine => "F21",
            Condition::InvalidText => "F22",
            Condition::InvalidGml => "F24",
            Condition::UnknownSystem => "F59",
        }
    }

    /// The subcondition's name as Table 16 prints it.
    pub fn text(self) -> &'static str {
        match self {
            Condition::InvalidArgument => "invalid argument",
            Condition::DivisionByZero => "attempted division by zero",
            Condition::UnsupportedUnit => "unsupported unit specified",
            Condition::FailedToTransform => "failed to transform geometry",
            Condition::EmptyPoint => "empty point value",
            Condition::PointNotWellFormed => "point value not well formed",
            Condition::PointsEqual => "points are equal",
            Condition::NotALine => "linestring is not a line",
            Condition::DegenerateLine => "degenerate line has no direction",
            Condition::InvalidText => "invalid well-known text representation",
            Condition::InvalidGml => "invalid GML representation",
            Condition::UnknownSystem => "unknown spatial reference system",
        }
    }
}

/// A raised condition with a message for people. The message begins with
/// the rule it breaks, as RULES.md R-ERR-2 asks of every failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fault {
    /// Which subcondition was raised.
    pub condition: Condition,
    /// The broken rule and what broke it.
    pub message: String,
}

impl Fault {
    /// A fault of `condition` with `message`.
    pub fn new(condition: Condition, message: impl Into<String>) -> Fault {
        Fault { condition, message: message.into() }
    }

    /// F02, invalid argument.
    pub(crate) fn argument(message: impl Into<String>) -> Fault {
        Fault::new(Condition::InvalidArgument, message)
    }

    /// F15, failed to transform geometry.
    pub(crate) fn transform(message: impl Into<String>) -> Fault {
        Fault::new(Condition::FailedToTransform, message)
    }

    /// F22, invalid well-known text representation.
    pub(crate) fn text(message: impl Into<String>) -> Fault {
        Fault::new(Condition::InvalidText, message)
    }

    /// F24, invalid GML representation.
    pub(crate) fn gml(message: impl Into<String>) -> Fault {
        Fault::new(Condition::InvalidGml, message)
    }
}

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "2F{}: {}", self.condition.subclass(), self.message)
    }
}

/// The engine's result type.
pub type Outcome<T> = Result<T, Fault>;
