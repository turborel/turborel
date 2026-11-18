//! The SQL/MM Full-Text Information Schema [ISO/IEC 13249-2:2002 §9]:
//! FT_FEATURES and FT_SCHEMATA, read through SQL.

use turborel::db::Database;
use turborel::error::Code;

use crate::common::rows;

/// Verifies: [ISO/IEC 13249-2:2002 §9.2, §10.2] FT_FEATURES has a row of
/// each feature and implementation-defined value: the lengths of a text, a
/// language, a thesaurus name and term, the default language, the least
/// and greatest rank, and YES for each unit of proximity and context.
#[test]
fn ft_features_lists_each_feature_9_2() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = db.session("A");
    let features = "SELECT FT_FEATURENAME, FT_FEATUREVALUE FROM FT_INFORMATION_SCHEMA.FT_FEATURES ORDER BY FT_FEATURENAME";
    assert_eq!(
        rows(&a, features).unwrap(),
        [
            "FT_ContextParagraph,YES",
            "FT_ContextSentence,YES",
            "FT_DefaultLanguage,ENGLISH",
            "FT_MaxLanguageLength,32",
            "FT_MaxRankValue,1.7976931348623157E308",
            "FT_MaxTextLength,32767",
            "FT_MinRankValue,0E0",
            "FT_ProximityCharacters,YES",
            "FT_ProximityParagraphs,YES",
            "FT_ProximitySentences,YES",
            "FT_ProximityWords,YES",
            "FT_ThesNameLength,128",
            "FT_ThesTermLength,1024",
        ]
    );
    let one = "SELECT FT_FEATUREVALUE FROM FT_INFORMATION_SCHEMA.FT_FEATURES WHERE FT_FEATURENAME = 'FT_DefaultLanguage'";
    assert_eq!(rows(&a, one).unwrap(), ["ENGLISH"]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §9.3, §10.3] FT_SCHEMATA has a row of
/// each schema of the catalog, its owner, and SQL_TEXT as the character
/// set of its full-text types and routines; a schema made later is listed
/// too.
#[test]
fn ft_schemata_lists_each_schema_9_3() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    db.schema("CREATE SCHEMA STORE AUTHORIZATION B").unwrap();
    let a = db.session("A");
    let schemata = "SELECT SCHEMA_NAME, SCHEMA_OWNER, DEFAULT_CHARACTER_SET_SCHEMA, DEFAULT_CHARACTER_SET_NAME \
        FROM FT_INFORMATION_SCHEMA.FT_SCHEMATA WHERE SCHEMA_NAME IN ('A', 'STORE') ORDER BY SCHEMA_NAME";
    assert_eq!(rows(&a, schemata).unwrap(), ["A,A,INFORMATION_SCHEMA,SQL_TEXT", "STORE,B,INFORMATION_SCHEMA,SQL_TEXT"]);
    let catalogs = "SELECT COUNT(*) FROM FT_INFORMATION_SCHEMA.FT_SCHEMATA WHERE CATALOG_NAME = DEFAULT_CHARACTER_SET_CATALOG";
    let all = rows(&a, "SELECT COUNT(*) FROM FT_INFORMATION_SCHEMA.FT_SCHEMATA").unwrap();
    assert_eq!(rows(&a, catalogs).unwrap(), all);
    let listed = "SELECT COUNT(*) FROM FT_INFORMATION_SCHEMA.FT_SCHEMATA WHERE SCHEMA_NAME = 'FT_INFORMATION_SCHEMA'";
    assert_eq!(rows(&a, listed).unwrap(), ["1"]);
    db.schema("CREATE SCHEMA LATER AUTHORIZATION A").unwrap();
    let later = "SELECT SCHEMA_OWNER FROM FT_INFORMATION_SCHEMA.FT_SCHEMATA WHERE SCHEMA_NAME = 'LATER'";
    assert_eq!(rows(&a, later).unwrap(), ["A"]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §9.1] SELECT on the views is PUBLIC's
/// and nothing else is granted, so an INSERT, UPDATE or DELETE of one is
/// SQLCODE -104.
#[test]
fn ft_views_cannot_be_updated_9_1() {
    let db = Database::new();
    db.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
    let a = db.session("A");
    for sql in [
        "INSERT INTO FT_INFORMATION_SCHEMA.FT_FEATURES VALUES ('a', 'b')",
        "UPDATE FT_INFORMATION_SCHEMA.FT_FEATURES SET FT_FEATUREVALUE = 'NO'",
        "DELETE FROM FT_INFORMATION_SCHEMA.FT_SCHEMATA",
    ] {
        assert_eq!(a.run(sql).unwrap_err().code, Code::Privilege, "{sql}");
    }
    assert_eq!(rows(&a, "SELECT COUNT(*) FROM FT_INFORMATION_SCHEMA.FT_FEATURES").unwrap(), ["13"]);
}
