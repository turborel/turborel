//! Unit tests of the Formats and the catalog-free Syntax Rules of the
//! statements of structured types [ISO/IEC 9075-2:2023 §11.51 to §11.58,
//! §11.60, §11.63 to §11.71, §19.13], of typed tables and views [§11.3,
//! §11.32], of the `<reference type>` [§6.1] and of the privileges of
//! structured types [§12.2, §12.3, §12.7]: each form read into its syntax
//! tree, and each refusal with its code and rule.

use crate::ast::{
    Action, AlterGroupAction, AlterTypeAction, DropBehavior, DropObject, MethodKind, MethodSignature, OrderingCategory, OrderingForm,
    PrivilegeObject, RefGeneration, RefRepresentation, RoutineType, SchemaStatement, SqlStatement, TableElement, TransformGroupSetting,
    TransformKind, TypeName, TypeOption,
};
use crate::error::{Code, SqlError};
use crate::parser::Parser;

/// The statement `text` is, which must parse.
fn statement(text: &str) -> SqlStatement {
    match Parser::new(text).and_then(|mut p| p.sql_statement()) {
        Ok(s) => s,
        Err(e) => panic!("{text}: {e:?}"),
    }
}

/// The schema statement `text` is, which must parse as one.
fn schema(text: &str) -> SchemaStatement {
    let SqlStatement::Schema { statement, .. } = statement(text) else { panic!("{text} is no schema statement") };
    statement
}

/// The error parsing `text` gives, which must fail.
fn refusal(text: &str) -> SqlError {
    match Parser::new(text).and_then(|mut p| p.sql_statement()) {
        Ok(s) => panic!("{text} parsed: {s:?}"),
        Err(e) => e,
    }
}

/// Asserts that `text` is refused with `code` under `rule` of ISO/IEC
/// 9075-2:2023.
fn refused(text: &str, code: Code, rule: &str) {
    let e = refusal(text);
    assert_eq!(e.code, code, "{text}: {e:?}");
    assert_eq!(e.rule.as_deref(), Some(format!("ISO/IEC 9075-2:2023 {rule}").as_str()), "{text}: {e:?}");
}

/// Asserts that `text` breaks the Syntax Rule `rule`, SQLCODE -103.
fn broken(text: &str, rule: &str) {
    refused(text, Code::SyntaxRule, rule);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.51, §11.52] CREATE TYPE of a
/// `<member list>`: each `<attribute definition>` with its type, its
/// default and the default's text, and its collation; UNDER a supertype;
/// the options after the members.
#[test]
fn create_type_reads_its_members_11_51() {
    let SchemaStatement::Type(t) = schema(
        "CREATE TYPE S.ADDRESS UNDER S.PLACE AS (STREET VARCHAR(20) DEFAULT 'MAIN', ZIP CHAR(5) COLLATE C, N INT) NOT INSTANTIABLE NOT FINAL",
    ) else {
        panic!("no type")
    };
    assert_eq!(t.under.as_ref().map(|u| u.ident.as_str()), Some("PLACE"));
    let members = t.members.unwrap();
    assert_eq!(members.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["STREET", "ZIP", "N"]);
    assert_eq!(members[0].default_text.as_deref(), Some("'MAIN'"));
    assert!(members[0].default.is_some());
    assert_eq!(members[1].collate.as_ref().map(|c| c.ident.as_str()), Some("C"));
    assert!(members[2].default.is_none());
    assert!(t.representation.is_none());
    assert!(matches!(t.options.as_slice(), [(TypeOption::Instantiable(false), _), (TypeOption::Final(false), _)]));
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.51 SR5a] AS of a predefined type
/// still defines a distinct type, no members read.
#[test]
fn create_type_of_a_predefined_type_is_distinct_11_51() {
    let SchemaStatement::Type(t) = schema("CREATE TYPE MONEY AS DECIMAL(9,2) FINAL") else { panic!("no type") };
    assert!(t.members.is_none());
    assert!(t.representation.is_some());
    assert!(t.methods.is_empty());
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.51] the `<reference type
/// specification>`s and the casts of a user-defined representation.
#[test]
fn create_type_reads_its_reference_representation_11_51() {
    let SchemaStatement::Type(t) =
        schema("CREATE TYPE P AS (A INT) REF USING INTEGER CAST (SOURCE AS REF) WITH TO_REF CAST (REF AS SOURCE) WITH TO_INT NOT FINAL")
    else {
        panic!("no type")
    };
    assert!(matches!(&t.options[0].0, TypeOption::Reference(RefRepresentation::User(_))));
    assert!(matches!(&t.options[1].0, TypeOption::CastToRef(f) if f == "TO_REF"));
    assert!(matches!(&t.options[2].0, TypeOption::CastToType(f) if f == "TO_INT"));
    let SchemaStatement::Type(d) = schema("CREATE TYPE Q AS (A INT, B INT) REF FROM (A, B) NOT FINAL") else { panic!("no type") };
    assert!(matches!(&d.options[0].0, TypeOption::Reference(RefRepresentation::Derived(a)) if a.len() == 2));
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.51] a `<method specification
/// list>`: original specifications of each kind, SELF AS RESULT, SPECIFIC,
/// the characteristics in any order and their defaults [SR9b xiv 3, 5],
/// and an overriding specification.
#[test]
fn create_type_reads_its_method_specifications_11_51() {
    let SchemaStatement::Type(t) = schema(
        "CREATE TYPE PT AS (X INT) NOT FINAL \
         METHOD NORM () RETURNS INT SPECIFIC S.NORM1 READS SQL DATA DETERMINISTIC LANGUAGE SQL RETURNS NULL ON NULL INPUT, \
         STATIC METHOD ORIGIN () RETURNS PT CONTAINS SQL, \
         CONSTRUCTOR METHOD PT (X INT) RETURNS PT SELF AS RESULT CALLED ON NULL INPUT MODIFIES SQL DATA",
    ) else {
        panic!("no type")
    };
    let [norm, origin, ctor] = t.methods.as_slice() else { panic!("{:?}", t.methods) };
    assert_eq!((norm.kind, norm.name.as_str()), (MethodKind::Instance, "NORM"));
    assert_eq!(norm.specific.as_ref().map(|s| s.ident.as_str()), Some("NORM1"));
    assert!(norm.characteristics.deterministic && norm.characteristics.null_call);
    assert_eq!(norm.characteristics.access, crate::ast::DataAccess::Reads);
    assert_eq!(origin.kind, MethodKind::Static);
    assert!(!origin.characteristics.deterministic && !origin.characteristics.null_call);
    assert_eq!(ctor.kind, MethodKind::Constructor);
    assert!(ctor.self_as_result);
    assert_eq!(ctor.parameters.len(), 1);
    let SchemaStatement::Type(sub) = schema("CREATE TYPE SUB UNDER PT AS (Y INT) NOT FINAL OVERRIDING METHOD NORM () RETURNS INT") else {
        panic!("no type")
    };
    assert!(sub.methods[0].overriding);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.51 SR8f, §11.52 SR4] two attributes
/// of one name, and a collation both in the type and after it.
#[test]
fn create_type_refuses_bad_members_11_51() {
    broken("CREATE TYPE T AS (A INT, A INT) NOT FINAL", "§11.51 SR8f");
    broken("CREATE TYPE T AS (A CHAR(2) COLLATE C COLLATE D) NOT FINAL", "§11.52 SR4");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.51 SR9b] the rules on a method
/// specification that need no catalog: of a static method, of a
/// constructor, of SELF AS RESULT, of the parameters and of the
/// characteristics. MC/DC: a constructor SELF AS RESULT, named and
/// returning its type, passes (create_type_reads_its_method_specifications)
/// and each of its conditions false alone fails.
#[test]
fn create_type_refuses_bad_method_specifications_11_51() {
    let t = "CREATE TYPE T AS (A INT) NOT FINAL ";
    broken(&format!("{t}STATIC METHOD M () RETURNS T SELF AS RESULT CONTAINS SQL"), "§11.51 SR9b ii 1");
    broken(&format!("{t}OVERRIDING STATIC METHOD M () RETURNS T"), "§11.51 SR9b ii 1");
    broken(&format!("{t}CONSTRUCTOR METHOD T () RETURNS T CONTAINS SQL"), "§11.51 SR9b iii 1");
    broken(&format!("{t}OVERRIDING CONSTRUCTOR METHOD T () RETURNS T"), "§11.51 SR9b iii 2");
    broken(&format!("{t}CONSTRUCTOR METHOD U () RETURNS T SELF AS RESULT CONTAINS SQL"), "§11.51 SR9b iii 3");
    broken(&format!("{t}CONSTRUCTOR METHOD T () RETURNS INT SELF AS RESULT CONTAINS SQL"), "§11.51 SR9b iii 4");
    broken(&format!("{t}METHOD M () RETURNS INT SELF AS RESULT CONTAINS SQL"), "§11.51 SR9b xi");
    broken(&format!("{t}METHOD M (P INT, P INT) RETURNS INT CONTAINS SQL"), "§11.51 SR9b viii 1");
    broken(&format!("{t}METHOD M (SELF INT) RETURNS INT CONTAINS SQL"), "§11.51 SR9b viii 2");
    broken(&format!("{t}METHOD M (IN P INT) RETURNS INT CONTAINS SQL"), "§11.51 SR9b ix 1");
    broken(&format!("{t}METHOD M (P INT RESULT) RETURNS INT CONTAINS SQL"), "§11.51 SR9b ix 2");
    broken(&format!("{t}METHOD M (P ROW(A INT)) RETURNS INT CONTAINS SQL"), "§11.51 SR9b ix 3");
    broken(&format!("{t}METHOD M (P CLOB AS LOCATOR) RETURNS INT CONTAINS SQL"), "§11.51 SR9b ix 4 A");
    broken(&format!("{t}METHOD M () RETURNS CLOB AS LOCATOR CONTAINS SQL"), "§11.51 SR9b x 1");
    broken(&format!("{t}METHOD M () RETURNS INT CAST FROM CHAR(2) CONTAINS SQL"), "§11.51 SR9b xiv 6 A I");
    broken(&format!("{t}METHOD M (INT) RETURNS INT CONTAINS SQL"), "§11.51 SR9b xiv 6 A IV");
    broken(&format!("{t}METHOD M () RETURNS INT DETERMINISTIC NOT DETERMINISTIC CONTAINS SQL"), "§11.51 SR9b xiv 1");
    broken(&format!("{t}METHOD M () RETURNS INT"), "§11.51 SR9b xiv 4");
    broken(&format!("{t}METHOD M () RETURNS INT NO SQL"), "§11.51 SR9b xiv 6 A II");
    broken(&format!("{t}METHOD M () RETURNS INT CONTAINS SQL PARAMETER STYLE SQL"), "§11.51 SR9b xiv 6 A III");
    refused(&format!("{t}METHOD M () RETURNS INT CONTAINS SQL LANGUAGE C"), Code::Unsupported, "§11.51 SR9b xiv 6 B");
    refused(&format!("{t}METHOD M () RETURNS T SELF AS LOCATOR CONTAINS SQL"), Code::Unsupported, "§11.51");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.53 to §11.58] each `<alter type
/// action>`.
#[test]
fn alter_type_reads_each_action_11_53() {
    let SchemaStatement::AlterType(added) = schema("ALTER TYPE S.T ADD ATTRIBUTE C INT DEFAULT 1") else { panic!("no alter") };
    assert_eq!(added.name.ident, "T");
    assert!(matches!(&added.action, AlterTypeAction::AddAttribute(d) if d.name == "C" && d.default_text.as_deref() == Some("1")));
    let SchemaStatement::AlterType(dropped) = schema("ALTER TYPE T DROP ATTRIBUTE C RESTRICT") else { panic!("no alter") };
    assert!(matches!(&dropped.action, AlterTypeAction::DropAttribute(c, _) if c == "C"));
    let SchemaStatement::AlterType(method) = schema("ALTER TYPE T ADD METHOD M (P INT) RETURNS INT") else { panic!("no alter") };
    let AlterTypeAction::AddMethod(spec) = &method.action else { panic!("no method") };
    // [§11.56 SR9d] CONTAINS SQL when none is written.
    assert_eq!(spec.characteristics.access, crate::ast::DataAccess::Contains);
    let SchemaStatement::AlterType(overriding) = schema("ALTER TYPE T ADD OVERRIDING METHOD M (P INT) RETURNS INT") else {
        panic!("no alter")
    };
    assert!(matches!(&overriding.action, AlterTypeAction::AddMethod(o) if o.overriding));
    let SchemaStatement::AlterType(designated) = schema("ALTER TYPE T DROP STATIC METHOD M (INT, CHAR(2)) RESTRICT") else {
        panic!("no alter")
    };
    assert!(matches!(&designated.action, AlterTypeAction::DropMethod(d) if d.kind == MethodKind::Static && d.parameters.len() == 2));
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.55, §11.56 SR5, SR6, §11.57 SR2,
/// §11.58] ALTER TYPE refusals: DROP without RESTRICT, and the rules of an
/// added method cited from §11.56 and §11.57.
#[test]
fn alter_type_refuses_bad_actions_11_53() {
    assert_eq!(refusal("ALTER TYPE T DROP ATTRIBUTE C CASCADE").code, Code::Syntax);
    broken("ALTER TYPE T ADD CONSTRUCTOR METHOD U () RETURNS T SELF AS RESULT", "§11.56 SR5b");
    broken("ALTER TYPE T ADD STATIC METHOD M () RETURNS T SELF AS RESULT", "§11.56 SR6a");
    broken("ALTER TYPE T ADD OVERRIDING STATIC METHOD M () RETURNS T", "§11.57 SR2");
    broken("ALTER TYPE T ADD METHOD M () RETURNS INT NO SQL", "§11.56 SR9b i 2");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.60] CREATE METHOD of a signature
/// and of a specific name: its kind, parameters, returns, type, body and
/// texts.
#[test]
fn create_method_reads_its_designator_and_body_11_60() {
    let SchemaStatement::Method(m) = schema("CREATE INSTANCE METHOD NORM (K INT) RETURNS INT FOR S.PT RETURN SELF.X * K") else {
        panic!("no method")
    };
    let MethodSignature::Signature { kind, name, parameters, returns, self_as_result, udt } = &m.signature else { panic!("no signature") };
    assert_eq!((*kind, name.0.as_str(), parameters.len()), (MethodKind::Instance, "NORM", 1));
    assert!(returns.is_some() && !self_as_result);
    assert_eq!(udt.ident, "PT");
    assert_eq!(m.body_text, "RETURN SELF.X * K");
    assert!(m.text.starts_with("CREATE INSTANCE METHOD"));
    let SchemaStatement::Method(c) =
        schema("CREATE CONSTRUCTOR METHOD PT (X INT) RETURNS PT SELF AS RESULT FOR PT BEGIN SET SELF.X = X; RETURN SELF; END")
    else {
        panic!("no method")
    };
    assert!(matches!(&c.signature, MethodSignature::Signature { self_as_result: true, kind: MethodKind::Constructor, .. }));
    let SchemaStatement::Method(s) = schema("CREATE SPECIFIC METHOD S.NORM1 RETURN 1") else { panic!("no method") };
    assert!(matches!(&s.signature, MethodSignature::Specific(n) if n.ident == "NORM1"));
    let SchemaStatement::Method(st) = schema("CREATE STATIC METHOD ORIGIN () FOR PT RETURN NEW PT(0)") else { panic!("no method") };
    assert!(matches!(&st.signature, MethodSignature::Signature { kind: MethodKind::Static, returns: None, .. }));
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.60, §11.51 SR9b ii 1] CREATE METHOD
/// refused: no FOR, a static method SELF AS RESULT, and an external body.
#[test]
fn create_method_refusals_11_60() {
    assert_eq!(refusal("CREATE METHOD M () RETURNS INT RETURN 1").code, Code::Syntax);
    broken("CREATE STATIC METHOD M () RETURNS T SELF AS RESULT FOR T RETURN NULL", "§11.51 SR9b ii 1");
    broken("CREATE METHOD M () FOR T EXTERNAL", "§11.60 SR23a");
}

/// Verifies: [ISO/IEC 9075-2:2023 §10.6] a `<specific routine
/// designator>` of a method: its kind, INSTANCE if none [SR2], and FOR its
/// type, which a method's designator has and no other's [SR3a]. MC/DC:
/// FOR with METHOD and none without pass; FOR without METHOD and METHOD
/// without FOR fail.
#[test]
fn routine_designator_of_a_method_10_6() {
    let SqlStatement::Drop { object: DropObject::Routine(d), behavior } = statement("DROP METHOD M (INT) FOR S.PT RESTRICT") else {
        panic!("no drop")
    };
    assert_eq!(d.routine_type, RoutineType::Method(MethodKind::Instance));
    assert_eq!(d.udt.as_ref().map(|u| u.ident.as_str()), Some("PT"));
    assert_eq!(behavior, DropBehavior::Restrict);
    let SqlStatement::Drop { object: DropObject::Routine(c), .. } = statement("DROP CONSTRUCTOR METHOD PT FOR PT CASCADE") else {
        panic!("no drop")
    };
    assert_eq!(c.routine_type, RoutineType::Method(MethodKind::Constructor));
    let SqlStatement::Drop { object: DropObject::Routine(s), .. } = statement("DROP SPECIFIC STATIC METHOD S.ORIGIN1 CASCADE") else {
        panic!("no drop")
    };
    assert!(s.specific && s.routine_type == RoutineType::Method(MethodKind::Static));
    let SqlStatement::Drop { object: DropObject::Routine(f), .. } = statement("DROP FUNCTION F (INT) CASCADE") else { panic!("no drop") };
    assert!(f.udt.is_none());
    broken("DROP FUNCTION F (INT) FOR T CASCADE", "§10.6 SR3a");
    broken("DROP METHOD M (INT) CASCADE", "§10.6 SR3a");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.63, §11.64] CREATE CAST, AS
/// ASSIGNMENT or not, and DROP CAST.
#[test]
fn casts_are_defined_and_dropped_11_63() {
    let SchemaStatement::Cast(c) = schema("CREATE CAST (PT AS INT) WITH FUNCTION PT_INT (PT) AS ASSIGNMENT") else { panic!("no cast") };
    assert!(c.assignment);
    assert_eq!(c.function.name.ident, "PT_INT");
    let SchemaStatement::Cast(m) = schema("CREATE CAST (INT AS PT) WITH METHOD TO_PT (INT) FOR PT") else { panic!("no cast") };
    assert!(!m.assignment && m.function.udt.is_some());
    let SqlStatement::Drop { object: DropObject::Cast(types, _), behavior } = statement("DROP CAST (PT AS INT) CASCADE") else {
        panic!("no drop")
    };
    assert!(matches!(types.0, TypeName::Domain(_)));
    assert_eq!(behavior, DropBehavior::Cascade);
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.65, §11.66] CREATE ORDERING of each
/// form and category, and DROP ORDERING.
#[test]
fn orderings_are_defined_and_dropped_11_65() {
    let SchemaStatement::Ordering(o) = schema("CREATE ORDERING FOR PT EQUALS ONLY BY STATE") else { panic!("no ordering") };
    assert_eq!(o.form, OrderingForm::Equals);
    assert!(matches!(o.category, OrderingCategory::State(None)));
    let SchemaStatement::Ordering(state) = schema("CREATE ORDERING FOR PT ORDER FULL BY STATE S.PT_STATE") else { panic!("no ordering") };
    assert!(matches!(state.category, OrderingCategory::State(Some(_))));
    let SchemaStatement::Ordering(map) = schema("CREATE ORDERING FOR PT ORDER FULL BY MAP WITH FUNCTION PT_MAP (PT)") else {
        panic!("no ordering")
    };
    assert_eq!(map.form, OrderingForm::Full);
    assert!(matches!(map.category, OrderingCategory::Map(_)));
    let SchemaStatement::Ordering(relative) = schema("CREATE ORDERING FOR PT ORDER FULL BY RELATIVE WITH SPECIFIC FUNCTION PT_CMP") else {
        panic!("no ordering")
    };
    assert!(matches!(relative.category, OrderingCategory::Relative(_)));
    let SqlStatement::Drop { object: DropObject::Ordering(t), .. } = statement("DROP ORDERING FOR PT RESTRICT") else { panic!("no drop") };
    assert_eq!(t.ident, "PT");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.67 to §11.71] CREATE, ALTER and
/// DROP TRANSFORM(S), and their rules: no two groups of one name [§11.67
/// SR2], each kind once [§11.67 SR4b, §11.69 SR4, §11.70 SR3].
#[test]
fn transforms_are_defined_altered_and_dropped_11_67() {
    let SchemaStatement::Transform(t) = schema(
        "CREATE TRANSFORMS FOR PT G1 (TO SQL WITH FUNCTION F1 (INT), FROM SQL WITH FUNCTION F2 (PT)) G2 (FROM SQL WITH FUNCTION F3)",
    ) else {
        panic!("no transform")
    };
    assert_eq!(t.groups.len(), 2);
    assert_eq!(t.groups[0].elements[0].0, TransformKind::ToSql);
    assert_eq!(t.groups[1].elements[0].0, TransformKind::FromSql);
    let SchemaStatement::AlterTransform(a) =
        schema("ALTER TRANSFORM FOR PT G1 (ADD (TO SQL WITH FUNCTION F4), DROP (FROM SQL, TO SQL CASCADE))")
    else {
        panic!("no alter")
    };
    assert_eq!(a.groups.len(), 2);
    assert!(matches!(&a.groups[0].action, AlterGroupAction::Add(e) if e.len() == 1));
    assert!(matches!(&a.groups[1].action, AlterGroupAction::Drop(k, DropBehavior::Cascade) if k.len() == 2));
    let SqlStatement::Drop { object: DropObject::Transform(_, g), .. } = statement("DROP TRANSFORM ALL FOR PT RESTRICT") else {
        panic!("no drop")
    };
    assert!(g.is_none());
    let SqlStatement::Drop { object: DropObject::Transform(_, named), .. } = statement("DROP TRANSFORMS G1 FOR PT CASCADE") else {
        panic!("no drop")
    };
    assert_eq!(named.as_deref(), Some("G1"));
    broken("CREATE TRANSFORM FOR PT G (TO SQL WITH FUNCTION F) G (FROM SQL WITH FUNCTION F)", "§11.67 SR2");
    broken("CREATE TRANSFORM FOR PT G (TO SQL WITH FUNCTION F, TO SQL WITH FUNCTION F)", "§11.67 SR4b");
    broken("ALTER TRANSFORM FOR PT G (ADD (FROM SQL WITH FUNCTION F, FROM SQL WITH FUNCTION F))", "§11.69 SR4");
    broken("ALTER TRANSFORM FOR PT G (DROP (TO SQL, TO SQL RESTRICT))", "§11.70 SR3");
}

/// Verifies: [ISO/IEC 9075-2:2023 §19.13] SET DEFAULT TRANSFORM GROUP and
/// SET TRANSFORM GROUP FOR TYPE, of a literal or a name.
#[test]
fn set_transform_group_19_13() {
    let SqlStatement::SetTransformGroup(TransformGroupSetting::Default(g)) = statement("SET DEFAULT TRANSFORM GROUP 'G1'") else {
        panic!("no setting")
    };
    assert_eq!(g, "G1");
    let SqlStatement::SetTransformGroup(TransformGroupSetting::ForType(t, group)) = statement("SET TRANSFORM GROUP FOR TYPE S.PT G2")
    else {
        panic!("no setting")
    };
    assert_eq!((t.ident.as_str(), group.as_str()), ("PT", "G2"));
}

/// Verifies: [ISO/IEC 9075-2:2023 §6.1] REF ( `<referenced type>` )
/// \[SCOPE `<table name>`\] as a column's type and an array's element.
#[test]
fn reference_type_6_1() {
    let SchemaStatement::Table(t) = schema("CREATE TABLE T (R REF(PT) SCOPE S.PEOPLE, Q REF(PT), A REF(PT) ARRAY[3])") else {
        panic!("no table")
    };
    let types: Vec<_> = t
        .elements
        .iter()
        .map(|e| {
            let TableElement::Column(c) = e else { panic!("{e:?}") };
            c.data_type.clone().unwrap()
        })
        .collect();
    assert!(matches!(&types[0], TypeName::Reference(r, Some(s)) if r.ident == "PT" && s.ident == "PEOPLE"));
    assert!(matches!(&types[1], TypeName::Reference(_, None)));
    assert!(matches!(&types[2], TypeName::Collection(e, _) if matches!(**e, TypeName::Reference(..))));
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.3] CREATE TABLE OF a type: REF IS
/// USER GENERATED, column options and a table constraint.
#[test]
fn typed_table_11_3() {
    let SchemaStatement::Table(t) = schema(
        "CREATE TABLE PEOPLE OF PERSON (REF IS OID USER GENERATED, NAME WITH OPTIONS NOT NULL, BOSS WITH OPTIONS SCOPE PEOPLE DEFAULT NULL, \
         UNIQUE (NAME))",
    ) else {
        panic!("no table")
    };
    let typed = t.typed.unwrap();
    assert_eq!(typed.udt.ident, "PERSON");
    assert!(typed.under.is_none());
    assert!(matches!(&t.elements[0], TableElement::SelfReferencing(n, RefGeneration::User, _) if n == "OID"));
    assert!(matches!(&t.elements[1], TableElement::Options(o) if o.constraints.len() == 1 && o.scope.is_none()));
    assert!(matches!(&t.elements[2], TableElement::Options(o) if o.scope.is_some() && o.default.is_some()));
    assert!(matches!(&t.elements[3], TableElement::Constraint(_)));
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.3] the generation of REF IS, SYSTEM
/// GENERATED if none is written, and a subtable UNDER its supertable of no
/// elements.
#[test]
fn typed_table_generations_and_subtable_11_3() {
    for (text, generation) in
        [("SYSTEM GENERATED", RefGeneration::System), ("DERIVED", RefGeneration::Derived), ("", RefGeneration::System)]
    {
        let SchemaStatement::Table(table) = schema(&format!("CREATE TABLE P OF PERSON (REF IS OID {text})")) else { panic!("no table") };
        assert!(matches!(&table.elements[0], TableElement::SelfReferencing(_, g, _) if *g == generation), "{text}");
    }
    let SchemaStatement::Table(sub) = schema("CREATE TABLE EMPS OF EMP UNDER PEOPLE") else { panic!("no table") };
    assert_eq!(sub.typed.unwrap().under.unwrap().ident, "PEOPLE");
    assert!(sub.elements.is_empty());
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.3 SR11b, SR11c ii, SR11d v, SR11g
/// ii, SR11g iii] the rules of a typed table that need no catalog. MC/DC
/// of SR11b: a subtable with REF IS fails, a table no subtable without it
/// fails, or with two, and with one passes (typed_table_11_3).
#[test]
fn typed_table_refusals_11_3() {
    broken("CREATE TABLE E OF EMP UNDER P (REF IS OID)", "§11.3 SR11b");
    broken("CREATE TABLE P OF PERSON", "§11.3 SR11b");
    broken("CREATE TABLE P OF PERSON (REF IS A, REF IS B)", "§11.3 SR11b");
    broken("CREATE GLOBAL TEMPORARY TABLE P OF PERSON (REF IS OID)", "§11.3 SR11c ii");
    broken("CREATE TABLE E OF EMP UNDER P (PRIMARY KEY (NAME))", "§11.3 SR11d v");
    broken("CREATE TABLE P OF PERSON (REF IS OID, N WITH OPTIONS NOT NULL, N WITH OPTIONS UNIQUE)", "§11.3 SR11g ii");
    broken("CREATE TABLE P OF PERSON (REF IS OID, N WITH OPTIONS)", "§11.3 SR11g iii");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.32] CREATE VIEW OF a type: REF IS,
/// USER GENERATED if none is written, column options of a scope, a
/// subview; and its rules SR23e, SR23h i to iii.
#[test]
fn referenceable_view_11_32() {
    let SchemaStatement::View(v) =
        schema("CREATE VIEW VP OF PERSON (REF IS OID DERIVED, BOSS WITH OPTIONS SCOPE VP) AS SELECT * FROM PEOPLE WITH CHECK OPTION")
    else {
        panic!("no view")
    };
    let (typed, elements) = *v.typed.unwrap();
    assert_eq!(typed.udt.ident, "PERSON");
    assert!(matches!(&elements[0], TableElement::SelfReferencing(_, RefGeneration::Derived, _)));
    assert!(matches!(&elements[1], TableElement::Options(o) if o.scope.is_some()));
    assert!(v.check_option.is_some() && v.columns.is_none());
    let SchemaStatement::View(u) = schema("CREATE VIEW VQ OF PERSON (REF IS OID) AS SELECT * FROM PEOPLE") else { panic!("no view") };
    assert!(matches!(&u.typed.unwrap().1[0], TableElement::SelfReferencing(_, RefGeneration::User, _)));
    let SchemaStatement::View(s) = schema("CREATE VIEW VE OF EMP UNDER VP AS SELECT * FROM EMPS") else { panic!("no view") };
    assert!(s.typed.unwrap().0.under.is_some());
    broken("CREATE VIEW V OF P AS SELECT * FROM T", "§11.32 SR23e");
    broken("CREATE VIEW V OF P UNDER Q (REF IS OID) AS SELECT * FROM T", "§11.32 SR23h ii");
    broken("CREATE VIEW V OF P (REF IS A, REF IS B) AS SELECT * FROM T", "§11.32 SR23h i");
    broken("CREATE VIEW V OF P (REF IS A SYSTEM GENERATED) AS SELECT * FROM T", "§11.32 SR23h iii");
}

/// Verifies: [ISO/IEC 9075-2:2023 §12.2, §12.3, §12.7] UNDER on a type,
/// WITH HIERARCHY OPTION of SELECT alone [§12.2 SR3a], HIERARCHY OPTION
/// FOR, SELECT of a `<privilege method list>`, and EXECUTE on a method.
#[test]
fn privileges_of_structured_types_12_3() {
    let SchemaStatement::Grant(under) = schema("GRANT UNDER ON TYPE PT TO U") else { panic!("no grant") };
    assert_eq!(under.actions, Some(vec![Action::Under]));
    let SchemaStatement::Grant(hierarchy) = schema("GRANT SELECT ON PEOPLE TO U WITH HIERARCHY OPTION WITH GRANT OPTION") else {
        panic!("no grant")
    };
    assert!(hierarchy.hierarchy_option && hierarchy.grant_option);
    let SchemaStatement::Grant(methods) = schema("GRANT SELECT (METHOD NORM () FOR PT, SPECIFIC METHOD S.N2), DELETE ON PEOPLE TO U")
    else {
        panic!("no grant")
    };
    assert_eq!(methods.actions, Some(vec![Action::SelectMethods, Action::Delete]));
    assert_eq!(methods.methods.len(), 2);
    let SchemaStatement::Grant(execute) = schema("GRANT EXECUTE ON STATIC METHOD ORIGIN () FOR PT TO U") else { panic!("no grant") };
    assert_eq!(execute.object, PrivilegeObject::Routine(RoutineType::Method(MethodKind::Static)));
    assert_eq!(execute.udt.unwrap().ident, "PT");
    let SchemaStatement::Revoke(revoke) = schema("REVOKE HIERARCHY OPTION FOR SELECT ON PEOPLE FROM U CASCADE") else {
        panic!("no revoke")
    };
    assert!(revoke.hierarchy_option_for && !revoke.grant_option_for);
    broken("GRANT DELETE ON PEOPLE TO U WITH HIERARCHY OPTION", "§12.2 SR3a");
}

/// Verifies: [ISO/IEC 9075-2:2023 §11.1 SR] a cast and an ordering in a
/// `<schema definition>` are kept with the count of the routines before
/// them, so each is defined after the routines written before it.
#[test]
fn defined_for_a_type_in_a_schema_definition_is_kept_in_order_11_1() {
    let sql = "CREATE SCHEMA S CREATE CAST (PT AS INT) WITH FUNCTION F CREATE FUNCTION F () RETURNS INT CONTAINS SQL RETURN 1 \
               CREATE ORDERING FOR PT EQUALS ONLY BY STATE";
    let s = Parser::new(sql).and_then(|mut p| p.schema()).unwrap();
    let at: Vec<usize> = s.defined.iter().map(|(before, _)| *before).collect();
    assert_eq!(at, [0, 1]);
    assert!(matches!(s.defined.as_slice(), [(_, SchemaStatement::Cast(_)), (_, SchemaStatement::Ordering(_))]), "{:?}", s.defined);
}
