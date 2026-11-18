//! Tests of how deeply what reading a schema and validating by it
//! recurses through nests (RULES.md R-CF-6, D18): the deepest schema of
//! each shape reading recurses through, read, its content models compiled
//! and checked and its non-determinism found, within half of a thread's
//! default stack, one level more SQLCODE -900; and a document nested as
//! deeply as a tree may be, validated by a recursive declaration, which
//! nothing recurses through.

use super::*;
use crate::xml::{MAX_NESTING, Node};

/// Half of a thread's default stack of 2 MiB, which the deepest case of
/// each shape runs within.
const HALF_DEFAULT_STACK: usize = 1024 * 1024;

/// The value of `run` on a thread of `HALF_DEFAULT_STACK`.
fn on_half_stack<T: Send + 'static>(run: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new().name("schema-depth".to_owned()).stack_size(HALF_DEFAULT_STACK).spawn(run).unwrap().join().unwrap()
}

/// `n` local element declarations, each of an anonymous complex type of a
/// sequence holding the next, in a global one.
fn local_elements(n: usize) -> String {
    let open = r#"<xs:element name="e"><xs:complexType><xs:sequence>"#;
    let close = "</xs:sequence></xs:complexType></xs:element>";
    [open.repeat(n), r#"<xs:element name="e" type="xs:string"/>"#.to_owned(), close.repeat(n)].concat()
}

/// `n` model groups, sequences and choices in turn, each holding the
/// next, the content model of a complex type.
fn model_groups(n: usize) -> String {
    let compositor = |i: usize| if i.is_multiple_of(2) { "sequence" } else { "choice" };
    let open = (0..n).map(|i| format!("<xs:{}>", compositor(i))).collect::<Vec<_>>().concat();
    let close = (0..n).rev().map(|i| format!("</xs:{}>", compositor(i))).collect::<Vec<_>>().concat();
    format!(r#"<xs:complexType name="t">{open}<xs:element name="e" type="xs:string"/>{close}</xs:complexType>"#)
}

/// `n` complex types, each extending the next by an element of its own.
fn derivations(n: usize) -> String {
    let derived = (0..n)
        .map(|i| {
            let base = i.saturating_add(1);
            format!(
                r#"<xs:complexType name="c{i}"><xs:complexContent><xs:extension base="c{base}"><xs:sequence><xs:element name="e{i}" type="xs:string"/></xs:sequence></xs:extension></xs:complexContent></xs:complexType>"#
            )
        })
        .collect::<Vec<_>>().concat();
    format!(
        r#"{derived}<xs:complexType name="c{n}"><xs:sequence><xs:element name="e{n}" type="xs:string"/></xs:sequence></xs:complexType>"#
    )
}

/// `n` model group definitions, each referring to the next, the content
/// model of a complex type.
fn group_references(n: usize) -> String {
    let groups = (0..n)
        .map(|i| format!(r#"<xs:group name="g{i}"><xs:sequence><xs:group ref="g{}"/></xs:sequence></xs:group>"#, i.saturating_add(1)))
        .collect::<Vec<_>>()
        .concat();
    format!(
        r#"{groups}<xs:group name="g{n}"><xs:sequence><xs:element name="e" type="xs:string"/></xs:sequence></xs:group><xs:complexType name="t"><xs:group ref="g0"/></xs:complexType>"#
    )
}

/// `n` attribute group definitions, each referring to the next, the
/// attributes of a complex type.
fn attribute_group_references(n: usize) -> String {
    let groups = (0..n)
        .map(|i| format!(r#"<xs:attributeGroup name="a{i}"><xs:attributeGroup ref="a{}"/></xs:attributeGroup>"#, i.saturating_add(1)))
        .collect::<Vec<_>>()
        .concat();
    format!(
        r#"{groups}<xs:attributeGroup name="a{n}"><xs:attribute name="x"/></xs:attributeGroup><xs:complexType name="t"><xs:attributeGroup ref="a0"/></xs:complexType>"#
    )
}

/// A shape of schema, of the levels given.
type Shape = fn(usize) -> String;

/// The most levels of `shape` a schema is read of, on a thread of half a
/// default stack: each read, and its non-determinism found, until one
/// level more is SQLCODE -900.
fn deepest(shape: Shape) -> usize {
    on_half_stack(move || {
        let mut n = 1_usize;
        loop {
            match schema(&shape(n)) {
                Ok(s) => assert!(!s.is_non_deterministic()),
                Err(e) => {
                    assert_eq!(e.sqlcode(), -900_i32, "{e}");
                    return n.saturating_sub(1);
                }
            }
            n = n.saturating_add(1);
        }
    })
}

/// Verifies: [DESIGN] RULES.md R-CF-6, D18, D106: of each shape reading a
/// schema recurses through, local declarations in anonymous types (three
/// levels of `MAX_DEPTH` each: the element, its type and its group),
/// model groups, derivations, model group references (two each: the
/// definition and its group) and attribute group references, the deepest
/// schema accepted, within four of its levels of `MAX_DEPTH`, is read,
/// its content models compiled and checked and its non-determinism found
/// within half of a thread's default stack, and one level more is SQLCODE
/// -900, never a stack overflow.
#[test]
fn reading_nests_within_half_a_stack() {
    let shapes: [(&str, Shape, usize); 5] = [
        ("local elements", local_elements, 3),
        ("model groups", model_groups, 1),
        ("derivations", derivations, 1),
        ("group references", group_references, 2),
        ("attribute group references", attribute_group_references, 1),
    ];
    for (name, shape, levels) in shapes {
        let n = deepest(shape);
        assert!(n.saturating_mul(levels) <= MAX_DEPTH, "{name}: {n}");
        assert!(n.saturating_add(4).saturating_mul(levels) > MAX_DEPTH, "{name}: {n}");
    }
}

/// The variable whose presence makes `bases_are_read_first` read its
/// schema in this process: the child process it started.
const CHILD: &str = "TURBOREL_SCHEMA_DEPTH_CHILD";

/// Verifies: [DESIGN] RULES.md R-CF-6, D106: a complex type's base is read
/// before the type, not within the frames of reading it, so the deepest
/// chain of derivations accepted is read within half of a thread's
/// default stack. The chain is read in a child process this test starts,
/// as a stack overflow aborts the process it happens in: an overflow
/// fails this test rather than the run of every test.
#[test]
fn bases_are_read_first() {
    if std::env::var_os(CHILD).is_some() {
        assert!(deepest(derivations) > 0);
        return;
    }
    let path = module_path!().split_once("::").map_or("", |(_, path)| path);
    let name = format!("{path}::bases_are_read_first");
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name.as_str(), "--test-threads=1"])
        .env(CHILD, "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "reading the deepest chain of derivations ended {status}");
}

/// `n` elements `e` nested, each of a distinct ID `k`.
fn nested_document(n: usize) -> String {
    let open = (1..=n).map(|i| format!(r#"<e k="k{i}">"#)).collect::<Vec<_>>().concat();
    [open, "</e>".repeat(n)].concat()
}

/// Verifies: [DESIGN] RULES.md R-CF-6, D106: a document of elements nested
/// `MAX_NESTING` deep, each declared by one recursive declaration of an
/// ID and a unique constraint, is validated as a document node, its
/// identity constraints and IDs checked, within half of a thread's default
/// stack, as nothing recurses: valid, and with two IDs equal not.
#[test]
fn validation_nests_within_half_a_stack() {
    let body = r#"<xs:element name="e"><xs:complexType><xs:sequence><xs:element ref="e" minOccurs="0"/></xs:sequence>
        <xs:attribute name="k" type="xs:ID"/></xs:complexType>
        <xs:unique name="u"><xs:selector xpath="e"/><xs:field xpath="@k"/></xs:unique></xs:element>"#;
    let outcomes = on_half_stack(move || {
        let s = schema(body).unwrap();
        let mut outcomes = Vec::new();
        for document in [nested_document(MAX_NESTING), nested_document(MAX_NESTING).replacen(r#"k="k2""#, r#"k="k1""#, 1)] {
            let node = Node::root(parse(&document, Syntax::Document, Whitespace::Preserve).unwrap());
            outcomes.push(match validate(&s, &node, Mode::Strict).unwrap() {
                Ok(valid) => valid.tree().descendants(valid.id()).count(),
                Err(failure) => {
                    assert_eq!(failure.code, INVALID, "{failure:?}");
                    0
                }
            });
        }
        outcomes
    });
    assert_eq!(outcomes, [MAX_NESTING, 0]);
}
