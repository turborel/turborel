//! The optimizer's rules turned on and off by settings (RULES.md D35,
//! D36): one setting a rule, `OPTIMIZER.` and its name, ON by default; a
//! session's own for itself, the database's kept with its definitions; a
//! rule a plug-in adds given its setting as it is added.

use turborel::db::{Database, OpenOptions};
use turborel::logical::{Cond, LogicalPlan, Truth};
use turborel::optimizer::{Rule, RuleContext, SIMPLIFY_CONDITIONS, switch_name};
use turborel::params::Params;
use turborel::settings::Source;
use turborel::value::Value;

use crate::common::{Scratch, after_crash, count, setup};

/// A query whose filter, TRUE, SIMPLIFY_CONDITIONS or `DropTrue` removes.
const ALWAYS: &str = "SELECT ID FROM T WHERE 1 = 1";

/// A rule of a plug-in: it drops a filter known TRUE.
struct DropTrue;

impl Rule for DropTrue {
    fn name(&self) -> &'static str {
        "drop_true"
    }
    fn apply(&self, plan: &LogicalPlan, _: &RuleContext<'_>) -> Option<LogicalPlan> {
        let LogicalPlan::Filter { cond: Cond::Const(Truth::True), input } = plan else { return None };
        Some((**input).clone())
    }
}

/// A rule of a plug-in named as a setting of another kind is.
struct MaxPasses;

impl Rule for MaxPasses {
    fn name(&self) -> &'static str {
        "max_passes"
    }
    fn apply(&self, _: &LogicalPlan, _: &RuleContext<'_>) -> Option<LogicalPlan> {
        None
    }
}

fn word(w: &str) -> Value {
    Value::Char(w.to_owned())
}

/// Verifies: [DESIGN] RULES.md D35, D36: each built-in rule has a setting
/// of its own, listed, ON by default; a session that sets one OFF plans
/// without the rule, other sessions with it; the database's OFF is every
/// session's that sets none, and a session's ON stays above it.
#[test]
fn a_rule_is_turned_off_by_its_setting() {
    let db = setup();
    let (s, other) = (db.session("APP"), db.session("APP"));
    let rules = db.with_optimizer(|o| o.rules()).unwrap();
    let listed = "SELECT SETTING_NAME FROM INFORMATION_SCHEMA.SETTINGS WHERE SETTING_NAME LIKE 'OPTIMIZER.%' AND SETTING_VALUE = 'ON'";
    assert_eq!(count(&s, listed), rules.len(), "{rules:?}");
    let filtered = |s: &turborel::db::Session| s.physical_plan(ALWAYS, &Params::new()).unwrap().contains("Filter TRUE");
    assert!(!filtered(&s));
    s.run("SET OPTIMIZER.SIMPLIFY_CONDITIONS = 'off'").unwrap();
    assert!(filtered(&s), "off for the session");
    assert!(!filtered(&other), "on for another");
    db.set_setting(&switch_name(SIMPLIFY_CONDITIONS), &word("OFF")).unwrap();
    assert!(filtered(&other), "the database's");
    s.run("SET OPTIMIZER.SIMPLIFY_CONDITIONS = 'ON'").unwrap();
    assert!(!filtered(&s), "the session's own over the database's");
}

/// Verifies: [DESIGN] RULES.md D35, D36, D25: the database's value of a
/// rule's setting is kept with its definitions, so it holds when the
/// database is opened again (what `with_optimizer` set was lost so); a
/// plug-in's rule has its setting as it is added, and a value the
/// database was given for it before, as it was opened again, is its
/// value; a rule whose setting would be another kind's is refused, and
/// nothing is changed.
///
/// Regression test (R-TEST-7): a rule turned off was on again once the
/// database was opened again.
#[test]
fn a_rules_setting_outlives_the_database() {
    let scratch = Scratch::new("rule-settings");
    let dir = scratch.path("db");
    let open = || {
        let db = Database::new();
        db.open_files(&after_crash(), &dir, &OpenOptions::default()).unwrap();
        db
    };
    let db = open();
    db.with_optimizer(|o| o.add(Box::new(DropTrue))).unwrap();
    db.set_setting(&switch_name(SIMPLIFY_CONDITIONS), &word("OFF")).unwrap();
    db.set_setting("OPTIMIZER.DROP_TRUE", &word("OFF")).unwrap();
    db.close().unwrap();
    drop(db);
    let db = open();
    assert_eq!(db.setting(&switch_name(SIMPLIFY_CONDITIONS)).unwrap().to_string(), "OFF", "kept");
    assert!(db.setting("OPTIMIZER.DROP_TRUE").is_err(), "not declared until the rule is added");
    db.with_optimizer(|o| o.add(Box::new(DropTrue))).unwrap();
    let listed = db.settings().unwrap();
    let drop_true = listed.iter().find(|l| l.definition.name == "OPTIMIZER.DROP_TRUE").unwrap();
    assert_eq!((drop_true.value.to_string(), drop_true.source), ("OFF".to_owned(), Source::Database), "the value given before");
    db.schema("CREATE SCHEMA AUTHORIZATION APP CREATE TABLE T (ID INTEGER)").unwrap();
    let s = db.session("APP");
    assert!(s.physical_plan(ALWAYS, &Params::new()).unwrap().contains("Filter TRUE"), "both off");
    s.run("SET OPTIMIZER.DROP_TRUE = 'ON'").unwrap();
    assert!(s.explain(ALWAYS, &Params::new()).unwrap().contains("drop_true"), "the plug-in's on for the session");
    let rules = db.with_optimizer(|o| o.rules().len()).unwrap();
    let refused = db.with_optimizer(|o| o.add(Box::new(MaxPasses))).map_err(|e| e.sqlcode());
    assert_eq!(refused, Err(-910_i32), "OPTIMIZER.MAX_PASSES is a number");
    assert_eq!(db.with_optimizer(|o| o.rules().len()).unwrap(), rules, "nothing changed");
}
