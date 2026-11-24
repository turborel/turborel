//! The mutations of a prepared CALL's parameter modes (RULES.md R-TEST-9,
//! D977, D978): each argument's `?`s kept by the parser, each `?` given its
//! parameter's mode and ordinal position, the inputs those not OUT alone,
//! DESCRIBE's PARAMETER items of input and output items, and the modes
//! sent to a client granted them and read by it.

use super::super::{Mutation, Package};

/// The test of each `?`'s mode through the API.
const MODES: &str = "call_modes::each_dynamic_parameter_of_a_call_has_its_mode_20_10";
/// The test of DESCRIBE's PARAMETER items.
const DESCRIBED: &str = "call_modes::describe_sets_each_parameters_mode_20_10";
/// The test of the modes over a connection.
const TOLD: &str = "modes::a_prepared_call_tells_its_parameters_modes";

/// Mechanisms whose removal must fail a prepared CALL's parameter modes.
pub const CALL_MODES: [Mutation; 9] = [
    Mutation {
        name: "call-argument-questions",
        package: Package::Core,
        breaks: "a CALL's argument keeps none of the `?`s read within it, so none has its parameter's mode",
        file: "src/parser/routines/call.rs",
        original: "            dynamic.push(self.questions_since(before));",
        replacement: "            dynamic.push(Vec::new());",
        integration: &[MODES, DESCRIBED],
        unit: &[],
    },
    Mutation {
        name: "call-mode-ordinal",
        package: Package::Core,
        breaks: "a `?`'s ordinal position is its parameter's from 0, not from 1",
        file: "src/db/prepared/outputs.rs",
        original: "            let ordinal = u32::try_from(k.saturating_add(1)).unwrap_or(u32::MAX);",
        replacement: "            let ordinal = u32::try_from(*k).unwrap_or(u32::MAX);",
        integration: &[MODES, DESCRIBED],
        unit: &[],
    },
    Mutation {
        name: "call-mode-given",
        package: Package::Core,
        breaks: "each `?` of a CALL is described as IN, whatever its parameter's mode",
        file: "src/db/prepared/outputs.rs",
        original: "            Some((_, mode, ordinal)) => DynamicParameter { data_type, mode: *mode, ordinal: Some(*ordinal) },",
        replacement: "            Some((_, _, ordinal)) => DynamicParameter { data_type, mode: ParameterMode::In, ordinal: Some(*ordinal) },",
        integration: &[MODES, DESCRIBED],
        unit: &[],
    },
    Mutation {
        name: "call-inputs-not-out",
        package: Package::Core,
        breaks: "an INOUT parameter's `?` is no input, so an execution takes no value for it",
        file: "src/db/prepared.rs",
        original: "        let inputs = parameters.iter().filter(|p| p.mode != ParameterMode::Out)",
        replacement: "        let inputs = parameters.iter().filter(|p| p.mode == ParameterMode::In)",
        integration: &[MODES, "call_outputs::a_prepared_call_gives_back_its_outputs_20_7"],
        unit: &[],
    },
    Mutation {
        name: "describe-parameter-mode",
        package: Package::Core,
        breaks: "DESCRIBE leaves a CALL's items' PARAMETER_MODE 0",
        file: "src/db/dynamic/descriptor.rs",
        original: "        self.set_number(DescriptorItem::ParameterMode, i64::from(mode));",
        replacement: "        let _unused = mode;",
        integration: &[DESCRIBED],
        unit: &[],
    },
    Mutation {
        name: "describe-output-parameters",
        package: Package::Core,
        breaks: "DESCRIBE OUTPUT of a CALL sets no item's PARAMETER fields",
        file: "src/db/dynamic/statements.rs",
        original: "            called(&mut top, prepared.output_parameter(at));",
        replacement: "            called(&mut top, None);",
        integration: &[DESCRIBED],
        unit: &[],
    },
    Mutation {
        name: "modes-granted-only",
        package: Package::Server,
        breaks: "a Prepared of a CALL carries tag 20 to a client not granted `parameter-modes`",
        file: "server/src/native/requests.rs",
        original: "        .filter(|_| granted.has(PARAMETER_MODES))",
        replacement: "        .filter(|_| true)",
        integration: &[TOLD],
        unit: &[],
    },
    Mutation {
        name: "modes-code-sent",
        package: Package::Server,
        breaks: "tag 20 sends every `?` as IN",
        file: "protocol/src/called.rs",
        original: "        b.put_u8(p.mode.code());",
        replacement: "        b.put_u8(1);",
        integration: &[TOLD],
        unit: &[],
    },
    Mutation {
        name: "modes-read-by-client",
        package: Package::Server,
        breaks: "a client told the modes gives the inputs alone, each IN",
        file: "client/src/statements.rs",
        original: "            Some(called) => called.parameters.clone(),",
        replacement: "            Some(_) => self.inputs.iter().cloned().map(DynamicParameter::input).collect(),",
        integration: &[TOLD],
        unit: &[],
    },
];
