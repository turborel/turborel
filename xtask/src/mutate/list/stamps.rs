//! Mutations of the creation and last-altered timestamps of Feature T011:
//! the instant a definition is stamped with, logged with it and kept in
//! the catalog's image, and what the views show (RULES.md R-TEST-9, D37).

use super::{Mutation, Package};

/// Each mechanism broken alone, and the tests that fail.
pub const STAMPS: [Mutation; 7] = [
    Mutation {
        name: "stamps-replayed-clock",
        package: Package::Core,
        breaks: "a definition is stamped with the clock as it is made, so a replayed one takes the clock of its replay",
        file: "src/db/definitions.rs",
        original: "        self.stamp = now;",
        replacement: "        self.stamp = crate::value::datetime::Now { zone: now.zone, ..self.now(&crate::settings::NONE) };",
        integration: &[
            "information_timestamps::timestamps_survive_recovery_and_checkpoints_t011",
            "information_timestamps::a_standby_shows_its_primarys_timestamps_t011",
        ],
        unit: &[],
    },
    Mutation {
        name: "stamps-schema-unstamped",
        package: Package::Core,
        breaks: "a <schema definition>'s routines keep the instant of the definition made before it",
        file: "src/db/schema.rs",
        original: "        self.stamp = now;",
        replacement: "",
        integration: &["information_timestamps::created_is_the_current_timestamp_of_the_definition_t011"],
        unit: &[],
    },
    Mutation {
        name: "stamps-record-instant",
        package: Package::Core,
        breaks: "a definition's record loses its instant",
        file: "src/log/record.rs",
        original: "    Ok(Now { utc, zone })",
        replacement: "    Ok(Now { utc: 0, zone })",
        integration: &[
            "information_timestamps::timestamps_survive_recovery_and_checkpoints_t011",
            "information_timestamps::a_standby_shows_its_primarys_timestamps_t011",
        ],
        unit: &["log::record::tests::every_kind_of_record_reads_back_as_written"],
    },
    Mutation {
        name: "stamps-image-created",
        package: Package::Core,
        breaks: "the catalog's image loses a descriptor's creation timestamp",
        file: "src/catalog/codec/routines.rs",
        original: "    put_instant(out, stamps.created);",
        replacement: "    put_instant(out, Now::default());",
        integration: &["information_timestamps::timestamps_survive_recovery_and_checkpoints_t011"],
        unit: &["catalog::codec::routines::tests::each_kind_of_routine_reads_back_as_written"],
    },
    Mutation {
        name: "stamps-method-body",
        package: Package::Core,
        breaks: "a method's body takes its specification's timestamps, not CREATE METHOD's",
        file: "src/db/structured/bodies.rs",
        original: "            stamps: Stamps::at(self.stamp),",
        replacement: "            stamps: spec.stamps,",
        integration: &["information_timestamps::created_is_the_current_timestamp_of_the_definition_t011"],
        unit: &[],
    },
    Mutation {
        name: "stamps-xquery-module-altered",
        package: Package::Core,
        breaks: "ALTER XQUERY MODULE moves a registered XQuery module's creation timestamp too",
        file: "src/db/definitions/xquery_modules/alter.rs",
        original: "        desc.stamps.created = old.stamps.created;",
        replacement: "",
        integration: &[],
        unit: &["db::definitions::xquery_modules::tests::alter_xquery_module_moves_last_altered_alone"],
    },
    Mutation {
        name: "stamps-trigger-shown",
        package: Package::Core,
        breaks: "TRIGGERS shows a null CREATED",
        file: "src/system/definition/triggers.rs",
        original: "                vec![stamped(t.created_at)],",
        replacement: "                vec![Value::Null],",
        integration: &[
            "information_timestamps::created_is_the_current_timestamp_of_the_definition_t011",
            "trigger_views::triggers_are_described_5_69",
        ],
        unit: &[],
    },
];
