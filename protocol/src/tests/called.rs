//! A Prepared's parameter modes, of a CALL, in its tail (RULES.md D978).

use turborel::catalog::{ParameterMode, QualifiedName};
use turborel::params::{DynamicParameter, ParameterType};
use turborel::value::{DataType, Spelled};

use super::{FAILED, kind_of, reply_round_trip};
use crate::{Called, Column, Reply};

/// The body of `reply`, encoded.
fn body(reply: &Reply) -> Vec<u8> {
    let mut bytes = Vec::new();
    reply.encode(&mut bytes);
    bytes.split_off(crate::HEADER)
}

/// A Prepared of `CALL S.P(?, ?, ?)`, of an IN, an OUT and an INOUT
/// parameter, told of its `?`s' modes if `told.0`, its columns' declared
/// types if `told.1`.
fn prepared(told: (bool, bool)) -> Reply {
    let (told, declared) = told;
    let integer = DataType::Exact { precision: 10, scale: 0, spelled: Spelled::Integer };
    let of = |mode, ordinal| DynamicParameter { data_type: ParameterType::Of(integer.clone()), mode, ordinal: Some(ordinal) };
    let column = |name: &str| {
        let c = Column { name: Some(name.into()), data_type: integer.clone(), not_null: false, declared: None };
        if declared { c.with_declared() } else { c }
    };
    let parameters = vec![of(ParameterMode::In, 1), of(ParameterMode::Out, 2), of(ParameterMode::InOut, 3)];
    let called = told.then(|| Called { procedure: QualifiedName { auth: "S".into(), ident: "P_1".into() }, parameters });
    let inputs = vec![ParameterType::Of(integer.clone()); 2];
    Reply::Prepared { statement: 1, parameters: inputs, columns: vec![column("B"), column("C")], called }
}

/// Verifies: [DESIGN] [ISO/IEC 9075-2:2023 §20.10 GR7e] RULES.md D978: a
/// Prepared untold of its modes is its first form, byte for byte; told,
/// the same bytes then tag 20, after a declared types' tag 19 if there is
/// one, the procedure's specific name and each `?` with its mode's code
/// and its parameter's ordinal position; each reads back as written.
#[test]
fn parameter_modes_follow_the_first_form() {
    for (told, declared) in [(false, false), (true, false), (false, true), (true, true)] {
        reply_round_trip(&prepared((told, declared)));
    }
    let (plain, told) = (body(&prepared((false, false))), body(&prepared((true, false))));
    assert!(told.starts_with(&plain));
    let tail = &told[plain.len()..];
    assert_eq!(tail[..12], [20, 1, 0, 0, 0, b'S', 3, 0, 0, 0, b'P', b'_']);
    let Reply::Prepared { called: Some(called), .. } = Reply::decode(b'P', &told).unwrap() else { panic!("no modes") };
    let modes: Vec<(u8, Option<u32>)> = called.parameters.iter().map(|p| (p.mode.code(), p.ordinal)).collect();
    assert_eq!(modes, [(1, Some(1)), (4, Some(2)), (2, Some(3))]);
    let both = body(&prepared((true, true)));
    let Reply::Prepared { columns, called: Some(_), .. } = Reply::decode(b'P', &both).unwrap() else { panic!("no modes") };
    assert!(columns[0].declared.is_some(), "declared types beside it");
}

/// Verifies: [DESIGN] RULES.md D978, D42: a mode of no code of Table 33, an
/// ordinal position of 0, and the modes' tag before the declared types'
/// fail the connection.
#[test]
fn parameter_modes_malformed_are_refused() {
    let (plain, told) = (body(&prepared((false, false))), body(&prepared((true, false))));
    let tail = told[plain.len()..].to_vec();
    // Past tag 20, the name "S" and "P_1", and the count: the first mode.
    let first = 1 + 5 + 7 + 4;
    let refused = |b: &[u8]| Reply::decode(b'P', b).map_err(|e| kind_of(&e)).err();
    let mut bad_mode = told.clone();
    bad_mode[plain.len() + first] = 3;
    assert_eq!(refused(&bad_mode), Some(FAILED), "no mode's code");
    let mut no_ordinal = told.clone();
    no_ordinal[plain.len() + first + 1] = 0;
    assert_eq!(refused(&no_ordinal), Some(FAILED), "an ordinal position of 0");
    let declared = body(&prepared((false, true)));
    let swapped = [&declared[..plain.len()], &tail, &declared[plain.len()..]].concat();
    assert_eq!(refused(&swapped), Some(FAILED), "tag 20 before tag 19");
}
