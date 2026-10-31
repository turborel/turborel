//! [ISO/IEC 9075:1992 §16.5] SET TIME ZONE: a session's time zone
//! displacement, the setting SESSION.TIME_ZONE (RULES.md D54). Split from
//! db/session.rs (RULES.md R-SIZE-2).

use super::{Session, State};
use crate::ast::ValueExpression;
use crate::binder::{self, displacement_of, is_displacement};
use crate::error::{Code, Result, SqlState, fail, fail_with};
use crate::exec::{Heard, eval};
use crate::params::Params;
use crate::settings::SESSION_TIME_ZONE;
use crate::value::{Datetime, Value};

impl Session {
    /// [ISO/IEC 9075:1992 §16.5] SET TIME ZONE `v`, an INTERVAL HOUR TO
    /// MINUTE [SR1], from -12:59 to +13:00, else SQLSTATE 22009 [GR1b];
    /// or, for none, LOCAL: the database's displacement again, the one a
    /// session begins with [GR1a].
    pub(super) fn set_time_zone(&self, st: &State, v: Option<&ValueExpression>, params: &Params) -> Result<()> {
        let name = st.settings.definition(SESSION_TIME_ZONE).map(|d| d.name.clone()).unwrap_or_default();
        let Some(v) = v else { return self.own.borrow_mut().set(&st.settings, &name, None) };
        let (e, t) = self.bound(st, params, |cx| binder::bind_value(cx, v))?;
        if !is_displacement(&t) {
            return fail(
                Code::SyntaxRule,
                format!("[ISO/IEC 9075:1992 §16.5 SR1] SET TIME ZONE takes an INTERVAL HOUR TO MINUTE, not {t}"),
            );
        }
        let value = eval(&e, &Vec::new(), &Heard(self.stop.warnings()))?;
        let Some(zone) = displacement_of(&value) else {
            let msg =
                format!("[ISO/IEC 9075:1992 §16.5 GR1b ii] {} is no time zone displacement from -12:59 to +13:00", value.to_literal());
            return fail_with(Code::Data, SqlState::INVALID_TIME_ZONE_DISPLACEMENT, msg);
        };
        self.own.borrow_mut().set(&st.settings, &name, Some(&Value::Char(Datetime::displacement_text(zone))))
    }
}
