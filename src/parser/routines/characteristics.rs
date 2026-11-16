//! [ISO/IEC 9075-2:2023 §11.60] The `<routine characteristics>` of an
//! `<SQL-invoked routine>`: read in any order, each at most once \[SR9a\],
//! then checked against the Syntax Rules that need no catalog. A routine
//! of a language other than SQL is an external routine \[SR9i\], which
//! turborel does not support.

use super::{RESULT, RETURNS, SPECIFIC, not_supported};
use crate::ast::{DataAccess, RoutineKind, TableName};
use crate::error::{Code, Result, SqlError};
use crate::lexer::kw;
use crate::parser::Parser;

// [ISO/IEC 9075-2:2023 §11.60] The words of the <routine characteristics>
// that are no key words of SQL-92.

/// PARAMETER, of a `<parameter style clause>`.
const PARAMETER: &str = "PARAMETER";
/// STYLE, of a `<parameter style clause>`.
const STYLE: &str = "STYLE";
/// GENERAL, a `<parameter style>`.
const GENERAL: &str = "GENERAL";
/// DETERMINISTIC, of a `<deterministic characteristic>`.
const DETERMINISTIC: &str = "DETERMINISTIC";
/// CONTAINS, of CONTAINS SQL.
const CONTAINS: &str = "CONTAINS";
/// READS, of READS SQL DATA.
const READS: &str = "READS";
/// MODIFIES, of MODIFIES SQL DATA.
const MODIFIES: &str = "MODIFIES";
/// DATA, of READS SQL DATA and MODIFIES SQL DATA.
const DATA: &str = "DATA";
/// CALLED, of CALLED ON NULL INPUT.
const CALLED: &str = "CALLED";
/// DYNAMIC, of DYNAMIC RESULT SETS.
const DYNAMIC: &str = "DYNAMIC";
/// SETS, of DYNAMIC RESULT SETS.
const SETS: &str = "SETS";
/// NEW, of NEW SAVEPOINT LEVEL.
const NEW: &str = "NEW";
/// OLD, of OLD SAVEPOINT LEVEL.
const OLD: &str = "OLD";
/// SAVEPOINT, of a `<savepoint level indication>`.
const SAVEPOINT: &str = "SAVEPOINT";

/// What the `<routine characteristics>` of an SQL routine say, checked.
pub(super) struct Characteristics {
    /// The `<specific name>`, if written.
    pub(super) specific: Option<TableName>,
    /// DETERMINISTIC was written; NOT DETERMINISTIC is implicit \[SR9e\].
    pub(super) deterministic: bool,
    /// The `<SQL-data access indication>`, which is written \[SR9g\].
    pub(super) access: DataAccess,
    /// RETURNS NULL ON NULL INPUT was written; CALLED ON NULL INPUT is
    /// implicit \[SR9f ii\].
    pub(super) null_call: bool,
}

/// An `<SQL-data access indication>` as written.
#[derive(Debug, Clone, Copy)]
enum Access {
    /// NO SQL, which no SQL routine has \[SR9m ii\].
    NoSql,
    /// CONTAINS SQL, READS SQL DATA or MODIFIES SQL DATA.
    Of(DataAccess),
}

/// Each `<routine characteristic>` written, with where it begins.
#[derive(Default)]
struct Written {
    /// The `<language clause>`: none for SQL, else the language's name.
    language: Option<(Option<String>, usize)>,
    /// The `<parameter style clause>`.
    style: Option<((), usize)>,
    /// SPECIFIC `<specific name>`.
    specific: Option<(TableName, usize)>,
    /// The `<deterministic characteristic>`: whether DETERMINISTIC.
    deterministic: Option<(bool, usize)>,
    /// The `<SQL-data access indication>`.
    access: Option<(Access, usize)>,
    /// The `<null-call clause>`: whether RETURNS NULL ON NULL INPUT.
    null_call: Option<(bool, usize)>,
    /// The `<maximum returned result sets>`.
    result_sets: Option<(u32, usize)>,
    /// The `<savepoint level indication>`: whether NEW SAVEPOINT LEVEL.
    savepoint: Option<(bool, usize)>,
}

/// [ISO/IEC 9075-2:2023 §11.60 SR9a] Puts `value`, a `what` written at
/// `at`, in `slot`, unless one is there already, which `rule` refuses.
fn once<T>(slot: &mut Option<(T, usize)>, value: T, at: usize, rule: &str, what: &str) -> Result<bool> {
    if slot.is_some() {
        let msg = format!("[ISO/IEC 9075-2:2023 §11.60 {rule}] the <routine characteristics> contain more than one {what}");
        return Err(SqlError::at(Code::SyntaxRule, at, msg));
    }
    *slot = Some((value, at));
    Ok(true)
}

/// The refusal at `at` of a `rule` of §11.60 that `msg` states.
fn broken(rule: &str, at: usize, msg: &str) -> SqlError {
    SqlError::at(Code::SyntaxRule, at, format!("[ISO/IEC 9075-2:2023 §11.60 {rule}] {msg}"))
}

impl Written {
    /// [ISO/IEC 9075-2:2023 §11.60 SR9c, SR9f, SR9j, SR9l] The rules on
    /// which characteristics a `kind` may have together.
    fn check(&self, kind: RoutineKind) -> Result<()> {
        match kind {
            RoutineKind::Function => {
                if let Some((_, at)) = self.result_sets {
                    return Err(broken("SR9c", at, "a function has no <returned result sets characteristic>"));
                }
                if let Some((_, at)) = self.savepoint {
                    return Err(broken("SR9j", at, "a <savepoint level indication> is of a procedure alone"));
                }
            }
            RoutineKind::Procedure => {
                if let Some((_, at)) = self.null_call {
                    return Err(broken("SR9f i 1", at, "a procedure has no <null-call clause>"));
                }
            }
        }
        let modifies = matches!(self.access, Some((Access::Of(DataAccess::Modifies), _)));
        if let Some((true, at)) = self.savepoint
            && !modifies
        {
            return Err(broken("SR9l", at, "NEW SAVEPOINT LEVEL needs MODIFIES SQL DATA"));
        }
        Ok(())
    }
}

impl Parser {
    /// [ISO/IEC 9075-2:2023 §11.60] `<routine characteristics>` of a
    /// `kind`, checked: `sql_only` is the first refusal of what an external
    /// routine alone may have, given if the routine is an SQL one.
    pub(super) fn routine_characteristics(&mut self, kind: RoutineKind, sql_only: Option<SqlError>) -> Result<Characteristics> {
        let mut written = Written::default();
        while self.routine_characteristic(&mut written)? {}
        if let Some((Some(language), at)) = &written.language {
            let msg =
                format!("[ISO/IEC 9075-2:2023 §11.60 SR9i] LANGUAGE {language} makes an external routine, which turborel does not support");
            return Err(not_supported(*at, &msg));
        }
        if let Some(refused) = sql_only {
            return Err(refused);
        }
        if let Some(((), at)) = written.style {
            return Err(broken("SR9m iii", at, "an SQL routine has no <parameter style clause>"));
        }
        written.check(kind)?;
        let access = match written.access {
            Some((Access::Of(access), _)) => access,
            Some((Access::NoSql, at)) => return Err(broken("SR9m ii", at, "an SQL routine is not NO SQL")),
            None => return Err(broken("SR9g", self.here(), "the <routine characteristics> shall contain an <SQL-data access indication>")),
        };
        if let Some((sets, at)) = written.result_sets
            && sets > 0
        {
            let msg = "[ISO/IEC 9075-2:2023 §11.60 CR1] DYNAMIC RESULT SETS above 0 needs Feature T471, “Result sets return value”, which \
                       turborel does not support";
            self.refuse_later(not_supported(at, msg));
        }
        if let Some((true, at)) = written.savepoint {
            let msg = "[ISO/IEC 9075-2:2023 §11.60 CR31] NEW SAVEPOINT LEVEL needs Feature T272, “Enhanced savepoint management”, which \
                       turborel does not support";
            self.refuse_later(not_supported(at, msg));
        }
        let deterministic = written.deterministic.is_some_and(|(deterministic, _)| deterministic);
        let null_call = written.null_call.is_some_and(|(null_call, _)| null_call);
        Ok(Characteristics { specific: written.specific.map(|(name, _)| name), deterministic, access, null_call })
    }

    /// One `<routine characteristic>`, put in `written`, if one is next.
    fn routine_characteristic(&mut self, written: &mut Written) -> Result<bool> {
        let at = self.here();
        if self.accept_kw(kw::LANGUAGE) {
            let language = if self.accept_kw(kw::SQL) { None } else { Some(self.identifier()?) };
            return once(&mut written.language, language, at, "SR9a", "<language clause>");
        }
        if self.accept_word(PARAMETER) {
            self.expect_word(STYLE)?;
            if !self.accept_kw(kw::SQL) {
                self.expect_word(GENERAL)?;
            }
            return once(&mut written.style, (), at, "SR9a", "<parameter style clause>");
        }
        if self.accept_word(SPECIFIC) {
            let name = self.table_name()?;
            return once(&mut written.specific, name, at, "SR9a", "<specific name>");
        }
        let deterministic = self.deterministic_characteristic()?;
        if let Some(deterministic) = deterministic {
            return once(&mut written.deterministic, deterministic, at, "SR9a", "<deterministic characteristic>");
        }
        let access = self.data_access_indication()?;
        if let Some(access) = access {
            return once(&mut written.access, access, at, "SR9a", "<SQL-data access indication>");
        }
        let null_call = self.null_call_clause()?;
        if let Some(null_call) = null_call {
            return once(&mut written.null_call, null_call, at, "SR9a", "<null-call clause>");
        }
        if self.accept_word(DYNAMIC) {
            self.expect_word(RESULT)?;
            self.expect_word(SETS)?;
            let sets = self.unsigned_integer()?;
            return once(&mut written.result_sets, sets, at, "SR9a", "<returned result sets characteristic>");
        }
        let savepoint = self.savepoint_level_indication()?;
        if let Some(new) = savepoint {
            return once(&mut written.savepoint, new, at, "SR9f i 2", "<savepoint level indication>");
        }
        Ok(false)
    }

    /// `<deterministic characteristic>` ::= DETERMINISTIC | NOT
    /// DETERMINISTIC, whether the former, if one is next.
    fn deterministic_characteristic(&mut self) -> Result<Option<bool>> {
        if self.accept_word(DETERMINISTIC) {
            return Ok(Some(true));
        }
        if !self.accept_kw(kw::NOT) {
            return Ok(None);
        }
        self.expect_word(DETERMINISTIC)?;
        Ok(Some(false))
    }

    /// `<SQL-data access indication>` ::= NO SQL | CONTAINS SQL | READS
    /// SQL DATA | MODIFIES SQL DATA, if one is next.
    fn data_access_indication(&mut self) -> Result<Option<Access>> {
        let access = if self.accept_kw(kw::NO) {
            Access::NoSql
        } else if self.accept_word(CONTAINS) {
            Access::Of(DataAccess::Contains)
        } else if self.accept_word(READS) {
            Access::Of(DataAccess::Reads)
        } else if self.accept_word(MODIFIES) {
            Access::Of(DataAccess::Modifies)
        } else {
            return Ok(None);
        };
        self.expect_kw(kw::SQL)?;
        let data = matches!(access, Access::Of(DataAccess::Reads)) || matches!(access, Access::Of(DataAccess::Modifies));
        if data {
            self.expect_word(DATA)?;
        }
        Ok(Some(access))
    }

    /// `<null-call clause>` ::= RETURNS NULL ON NULL INPUT | CALLED ON
    /// NULL INPUT, whether the former, if one is next.
    fn null_call_clause(&mut self) -> Result<Option<bool>> {
        let null_call = if self.accept_word(RETURNS) {
            self.expect_kw(kw::NULL)?;
            true
        } else if self.accept_word(CALLED) {
            false
        } else {
            return Ok(None);
        };
        self.expect_kw(kw::ON)?;
        self.expect_kw(kw::NULL)?;
        self.expect_kw(kw::INPUT)?;
        Ok(Some(null_call))
    }

    /// `<savepoint level indication>` ::= NEW SAVEPOINT LEVEL | OLD
    /// SAVEPOINT LEVEL, whether the former, if one is next.
    fn savepoint_level_indication(&mut self) -> Result<Option<bool>> {
        let new = if self.accept_word(NEW) {
            true
        } else if self.accept_word(OLD) {
            false
        } else {
            return Ok(None);
        };
        self.expect_word(SAVEPOINT)?;
        self.expect_kw(kw::LEVEL)?;
        Ok(Some(new))
    }
}
