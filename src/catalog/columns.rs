//! A column's description: its name, data type, nullability, default,
//! domain and collation; and of an identity column [ISO/IEC 9075-2:2023
//! §4.17.12] or a generated column [§4.17.13], how its values are given.

use super::{QualifiedName, SequenceDesc};
use crate::value::DataType;
use crate::value::Value;
use crate::value::datetime::{Clocked, Moment};
use crate::value::types::datetime::current;

/// [X3.135-1986 §4.3] a column's description is its name, data type, and nullability.
#[derive(Debug, Clone)]
pub struct ColumnDesc {
    /// The `<column name>`.
    pub name: String,
    /// The column's `<data type>`.
    pub data_type: DataType,
    /// Whether it holds no nulls: a NOT NULL constraint is on it, or it is
    /// in the PRIMARY KEY (`TableDesc::refresh_not_null`).
    pub not_null: bool,
    /// [ISO/IEC 9075:1992 §11.5 GR1] The default value it includes, if its
    /// definition has a `<default clause>`.
    pub default: Option<ColumnDefault>,
    /// [ISO/IEC 9075:1992 §11.4 GR5] The domain it is of, if its definition
    /// names one, with that domain's default, kept with it as the domain
    /// changes (`db/domains.rs`).
    pub domain: Option<ColumnDomain>,
    /// [ISO/IEC 9075-2:2023 §6.1 SR3] The declared type collation, if not
    /// that of the character set.
    pub collation: Option<QualifiedName>,
    /// [ISO/IEC 9075-2:2023 §11.4 GR5f, GR5j] Of an identity column, its
    /// generator; of a generated column, its generation expression. None
    /// of a base column that is no identity column, which is "NEVER"
    /// generated.
    pub generated: Option<Generated>,
}

/// [ISO/IEC 9075-2:2023 §11.4 GR5f, GR5j] How the values of an identity
/// column or a generated column are given.
#[derive(Debug, Clone, PartialEq)]
pub enum Generated {
    /// [§4.17.12] An identity column's.
    Identity(Identity),
    /// [§4.17.13] A generated column's.
    Expression(Generation),
}

/// [ISO/IEC 9075-2:2023 §4.17.12] What the descriptor of an identity
/// column holds: whether its values are always generated, or by default,
/// and its sequence generator \[§11.4 GR4, GR5f\].
#[derive(Debug, Clone, PartialEq)]
pub struct Identity {
    /// GENERATED ALWAYS, rather than BY DEFAULT.
    pub always: bool,
    /// \[DESIGN\] Its sequence generator, the table's own and in no schema
    /// (RULES.md D62, D63): its name is the table's, and its counter is
    /// keyed by that name and its epoch, as any generator's is.
    pub sequence: SequenceDesc,
}

/// [ISO/IEC 9075-2:2023 §4.17.13] A generated column's generation
/// expression, as written \[§11.4 GR5j\], bound over the columns of its
/// row as each statement that writes the table runs (RULES.md D63).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Generation {
    /// The `<value expression>`.
    pub text: String,
    /// The schema of the names in `text` written without one [§5.4 SR8]:
    /// its table's, in a `<schema definition>`; else the session's default
    /// schema as it was defined.
    pub names: String,
}

/// [ISO/IEC 9075:1992 §11.4 GR5] The domain a column is of.
#[derive(Debug, Clone)]
pub struct ColumnDomain {
    /// The `<domain name>`.
    pub name: QualifiedName,
    /// The domain's default, which is the column's if it has none of its
    /// own [§11.5 GR2b].
    pub default: Option<ColumnDefault>,
}

/// [ISO/IEC 9075:1992 §11.5 GR1] The default value of a column descriptor,
/// derived from a `<default option>`.
#[derive(Debug, Clone)]
pub enum ColumnDefault {
    /// That of a `<literal>`, or the null value \[GR1a, b\].
    Value(Value),
    /// CURRENT_USER, SESSION_USER or SYSTEM_USER, whichever it is when the
    /// default is taken \[GR1c\]: in this implementation, the authorization
    /// identifier the statement runs under, each.
    User,
    /// [ISO/IEC 9075:1992 §6.8] CURRENT_DATE, CURRENT_TIME or
    /// CURRENT_TIMESTAMP, of `precision`: the instant of the statement that
    /// takes the default \[GR1d\].
    Datetime(Moment, u32),
}

impl ColumnDesc {
    /// [ISO/IEC 9075:1992 §11.5 GR2] Its default value, for a statement run
    /// under the authorization identifier `auth` at `now`, whose instant
    /// only a datetime default reads: before store assignment pads a
    /// fixed-length string \[GR1b iii, GR1c ii\]. [ISO/IEC 9075-2:2023
    /// §11.5 GR3a] Of an identity column or a generated column, the null
    /// value, its site unassigned until its value is given it.
    pub fn default_value(&self, auth: &str, now: Clocked<'_>) -> Value {
        if self.generated.is_some() {
            return Value::Null;
        }
        // [GR2a] its own; [GR2b] its domain's; [GR2c] null.
        let of_domain = self.domain.as_ref().and_then(|d| d.default.as_ref());
        match self.default.as_ref().or(of_domain) {
            Some(ColumnDefault::Value(v)) => v.clone(),
            Some(ColumnDefault::User) => Value::Char(auth.to_string()),
            Some(ColumnDefault::Datetime(moment, precision)) => current(*moment, *precision, now.instant()),
            None => Value::Null,
        }
    }

    /// Whether its default, its own or its domain's, is the statement's
    /// instant: CURRENT_DATE, CURRENT_TIME or CURRENT_TIMESTAMP.
    pub fn default_reads_instant(&self) -> bool {
        let of_domain = self.domain.as_ref().and_then(|d| d.default.as_ref());
        self.generated.is_none() && matches!(self.default.as_ref().or(of_domain), Some(ColumnDefault::Datetime(..)))
    }

    /// Of an identity column, what its descriptor holds of it.
    pub fn identity(&self) -> Option<&Identity> {
        if let Some(Generated::Identity(identity)) = &self.generated { Some(identity) } else { None }
    }

    /// Of a generated column, its generation expression.
    pub fn generation(&self) -> Option<&Generation> {
        if let Some(Generated::Expression(generation)) = &self.generated { Some(generation) } else { None }
    }

    /// [ISO/IEC 9075-2:2023 §14.11 SR13b, §14.15 SR5] Whether it is a
    /// generated column, or an identity column whose values are always
    /// generated: one no statement gives a value of its own, without
    /// OVERRIDING SYSTEM VALUE.
    pub fn always_generated(&self) -> bool {
        match &self.generated {
            Some(Generated::Identity(identity)) => identity.always,
            Some(Generated::Expression(_)) => true,
            None => false,
        }
    }
}
