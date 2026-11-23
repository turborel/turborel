//! The mutations of a character string held in its value or in a buffer
//! its clones share (RULES.md R-TEST-9, D960): the zeros past a string
//! held in the value read as its text, trailing spaces kept where they are
//! left out, octets counted as characters, a string padded with what is
//! not spaces, a long padding dropped, the second of two strings joined
//! lost, a hash that is not its text's, a CHARACTER read unpadded, a
//! concatenation that keeps its left operand alone, and the zeros past a
//! binary string held in its value read as its octets (D961).

use super::super::{Mutation, Package};

/// The file of the character string.
const CHARS: &str = "src/value/chars.rs";
/// The unit test of strings at the capacity.
const HELD: &str = "value::chars::tests::a_string_is_held_inline_up_to_the_capacity";
/// The unit test of strings trimmed and counted.
const TRIMMED: &str = "value::chars::tests::a_string_is_trimmed_and_counted_over_its_octets";
/// The unit test of strings padded.
const PADDED: &str = "value::chars::tests::a_string_is_padded_in_either_form";
/// The unit test of strings joined.
const JOINED: &str = "value::chars::tests::strings_are_joined_in_either_form";
/// The test of string functions over both forms.
const FUNCTIONS: &str = "char_strings::string_functions_give_the_same_text_in_either_form";
/// The test of strings read back as assigned.
const READ_BACK: &str = "char_strings::strings_read_back_as_assigned_at_every_length";

/// Mechanisms of the character string whose removal must fail their tests.
pub const CHAR_STRINGS: [Mutation; 11] = [
    Mutation {
        name: "chars-zeros-read-as-text",
        package: Package::Core,
        breaks: "a string held in its value reads as the whole array, the zeros past its length with it",
        file: CHARS,
        original: "std::str::from_utf8(&bytes.0).ok().and_then(|s| s.get(..usize::from(*len))).unwrap_or_default()",
        replacement: "std::str::from_utf8(&bytes.0).unwrap_or_default()",
        integration: &[READ_BACK],
        unit: &[HELD],
    },
    Mutation {
        name: "chars-unpadded-keeps-spaces",
        package: Package::Core,
        breaks: "a string's octets without its trailing spaces keep them",
        file: CHARS,
        original: "bytes.get(..kept).unwrap_or_default()",
        replacement: "bytes",
        integration: &[],
        unit: &[TRIMMED],
    },
    Mutation {
        name: "chars-count-octets",
        package: Package::Core,
        breaks: "a string's characters are counted as its octets, those that continue a character with them",
        file: CHARS,
        original: "self.as_bytes().iter().filter(|&&b| !continues(b)).count()",
        replacement: "self.as_bytes().len()",
        integration: &[READ_BACK],
        unit: &[TRIMMED],
    },
    Mutation {
        name: "chars-padded-not-spaces",
        package: Package::Core,
        breaks: "a string is padded with zeros, not spaces",
        file: CHARS,
        original: "let mut stack = [SPACE; STACKED];",
        replacement: "let mut stack = [0; STACKED];",
        integration: &[READ_BACK],
        unit: &[PADDED],
    },
    Mutation {
        name: "chars-padded-past-stack-dropped",
        package: Package::Core,
        breaks: "a string padded past what the stack builds is left unpadded",
        file: CHARS,
        original: "let Some(whole) = stack.get(..total) else { return CharString::from(spaced(text, pad)) };",
        replacement: "let Some(whole) = stack.get(..total) else { return CharString::from(text) };",
        integration: &[READ_BACK],
        unit: &[PADDED],
    },
    Mutation {
        name: "chars-joined-in-value-loses-second",
        package: Package::Core,
        breaks: "two strings joined in the value keep the first alone, zeros where the second goes",
        file: CHARS,
        original: "            tail.copy_from_slice(other.as_bytes());\n",
        replacement: "",
        integration: &[FUNCTIONS],
        unit: &[JOINED],
    },
    Mutation {
        name: "chars-joined-in-buffer-loses-second",
        package: Package::Core,
        breaks: "two strings joined past the capacity keep the first alone",
        file: CHARS,
        original: "        text.push_str(other);\n",
        replacement: "",
        integration: &[FUNCTIONS],
        unit: &[JOINED],
    },
    Mutation {
        name: "chars-hash-not-of-text",
        package: Package::Core,
        breaks: "a string hashes by its length, not as its text does, so a table keyed by strings is not asked by a str",
        file: CHARS,
        original: "self.as_str().hash(state);",
        replacement: "self.len().hash(state);",
        integration: &[],
        unit: &["value::chars::tests::the_forms_compare_and_hash_as_their_text"],
    },
    Mutation {
        name: "chars-fixed-read-unpadded",
        package: Package::Core,
        breaks: "a CHARACTER read from its record is not padded to its length",
        file: "src/value/types/character.rs",
        original: "Some(Value::Char(CharString::padded(s, pad)))",
        replacement: "Some(Value::Char(CharString::from(s)))",
        integration: &[READ_BACK],
        unit: &["value::chars::tests::a_fixed_string_is_read_padded_in_either_form"],
    },
    Mutation {
        name: "chars-concatenation-keeps-left",
        package: Package::Core,
        breaks: "a concatenation of two strings held as values is its left operand alone",
        file: "src/functions/strings.rs",
        original: "return Ok(Value::Char(a.joined(b)));",
        replacement: "return Ok(Value::Char(a.clone()));",
        integration: &[FUNCTIONS],
        unit: &[],
    },
    Mutation {
        name: "binary-zeros-read-as-octets",
        package: Package::Core,
        breaks: "a binary string held in its value reads as the whole array, the zeros past its length with it",
        file: "src/value/binary.rs",
        original: "Form::Inline { len, octets } => octets.get(..usize::from(*len)).unwrap_or_default(),",
        replacement: "Form::Inline { octets, .. } => octets.as_slice(),",
        integration: &["binary::binary_strings_compare_8_2"],
        unit: &["value::binary::tests::a_binary_string_is_held_inline_up_to_the_capacity"],
    },
];
