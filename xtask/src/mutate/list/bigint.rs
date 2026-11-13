//! The mutations of BIGINT (RULES.md R-TEST-9): its precision, its name
//! as a type where a domain could be named, its tag in the catalog, and
//! its code in a descriptor area.

use super::{Mutation, Package};

/// The mutations of BIGINT, in the order they run.
pub const BIGINT: [Mutation; 6] = [
    Mutation {
        name: "bigint-precision-integer",
        package: Package::Core,
        breaks: "BIGINT is INTEGER's precision, so it holds no more than ten digits",
        file: "src/value.rs",
        original: "pub const BIGINT_PRECISION: u32 = MAX_EXACT_PRECISION;",
        replacement: "pub const BIGINT_PRECISION: u32 = INTEGER_PRECISION;",
        integration: &["bigint::the_bigint_type_6_1"],
        unit: &[],
    },
    Mutation {
        name: "bigint-a-domain-name",
        package: Package::Core,
        breaks: "BIGINT as a column's type is a domain named BIGINT if one is there",
        file: "src/parser/domains.rs",
        original: "            || self.at_bigint_type()\n",
        replacement: "",
        integration: &["bigint::the_bigint_type_6_1"],
        unit: &[],
    },
    Mutation {
        name: "bigint-described-numeric",
        package: Package::Core,
        breaks: "a BIGINT column is described as NUMERIC, not by the name it was written with",
        file: "src/value/types/exact.rs",
        original: "            Spelled::Bigint => BIGINT_NAME,",
        replacement: "            Spelled::Bigint => self.name(),",
        integration: &["bigint::the_bigint_type_6_1"],
        unit: &[],
    },
    Mutation {
        name: "bigint-tag-read-integer",
        package: Package::Core,
        breaks: "a BIGINT column read back from the catalog is written INTEGER",
        file: "src/value/types/tags.rs",
        original: "        BIGINT_TYPE => Some(Spelled::Bigint),",
        replacement: "        BIGINT_TYPE => Some(Spelled::Integer),",
        integration: &["bigint::bigint_columns_are_kept"],
        unit: &["value::types::tags::tests::number_types_keep_their_names"],
    },
    Mutation {
        name: "bigint-described-code-integer",
        package: Package::Core,
        breaks: "DESCRIBE gives a BIGINT item INTEGER's code 4, not 25",
        file: "src/db/dynamic/descriptor.rs",
        original: "            Spelled::Bigint => code::BIGINT,",
        replacement: "            Spelled::Bigint => code::INTEGER,",
        integration: &["bigint::bigint_in_a_descriptor_20_1"],
        unit: &[],
    },
    Mutation {
        name: "bigint-code-read-integer",
        package: Package::Core,
        breaks: "an item of TYPE 25 is INTEGER, so it takes an INTEGER value and refuses a BIGINT one",
        file: "src/db/dynamic/descriptor.rs",
        original: "            code::BIGINT => crate::value::types::BIGINT_NAME.to_owned(),",
        replacement: "            code::BIGINT => \"INTEGER\".to_owned(),",
        integration: &["bigint::bigint_in_a_descriptor_20_1"],
        unit: &[],
    },
];
