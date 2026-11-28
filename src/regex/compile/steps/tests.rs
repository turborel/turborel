//! Unit tests of what a program knows of each instruction's step
//! (RULES.md D1311): where two ways can meet, the characters of ASCII an
//! instruction matches, and the octet that ends a loop run through.

use super::super::LOOP_TAKES;
use super::*;
use crate::regex::Regex;
use crate::regex::bounded::tests::PATTERNS;
use crate::regex::class::CharSet;

/// Verifies: [DESIGN] (RULES.md D1311) two ways can meet at a program's
/// first instruction, at each one a fork or a jump leads to, and at the
/// one after the loop that takes the rest of a text; at no other, which
/// a way reaches from the instruction before it alone. Of `ab|ac` between
/// its two positions kept, those are the first, both branches and where
/// they join.
#[test]
fn two_ways_meet_where_a_fork_or_a_jump_leads() {
    let insts = [
        Inst::Save(0),
        Inst::Split(2, 5),
        Inst::Char('a'),
        Inst::Char('b'),
        Inst::Jump(7),
        Inst::Char('a'),
        Inst::Char('c'),
        Inst::Save(1),
        Inst::Match,
    ];
    assert_eq!(meets(&insts, None), [true, false, true, false, false, true, false, true, false]);
    assert_eq!(meets(&insts, Some((5, 8))), [true, false, true, false, false, true, false, true, true]);
    assert_eq!(meets(&[Inst::Jump(9), Inst::Match], Some((0, 7))), [true, false], "a place past the program is none");
    for (pattern, flag) in PATTERNS {
        let program = Regex::new(pattern, flag).unwrap().program;
        assert_eq!(program.passed.meets.len(), program.insts.len(), "{pattern:?}");
        assert!(program.passed.meets[0], "{pattern:?}");
    }
}

/// Verifies: [DESIGN] (RULES.md D1311) an instruction's table holds
/// exactly the characters of ASCII it matches, of a character, of a
/// class, of a class negated and of one of no character of ASCII; and
/// holds none of an instruction that matches no character by what it is
/// alone, the wildcard that looks for an LF after a CR among them.
#[test]
fn a_table_holds_the_ascii_characters_an_instruction_matches() {
    let sets = [
        CharSet::single('/'),
        CharSet::single('/').complement(),
        CharSet::chars(&['a', 'z', '\u{7f}', '\0', '?', '@']),
        CharSet::single('é'),
    ];
    let mut insts: Vec<Inst> = sets.iter().cloned().map(Inst::Set).collect();
    insts.extend([Inst::Char('~'), Inst::AnyButCrlf, Inst::Save(0), Inst::Match]);
    let made = tables(&insts);
    assert_eq!(made.len(), insts.len());
    for (inst, table) in insts.iter().zip(&made) {
        for octet in 0..128_u8 {
            let wanted = taken_by(inst).is_some_and(|set| set.contains(char::from(octet)));
            assert_eq!(holds(table, octet), wanted, "{inst:?} of {octet}");
        }
    }
    assert_eq!(made[1], [!(1_u64 << 47_u32), u64::MAX], "every character of ASCII but the solidus");
    assert_eq!((made[3], made[5]), ([0, 0], [0, 0]));
}

/// Verifies: [DESIGN] (RULES.md D1311) a loop the program runs through
/// is ended by one octet where its instruction matches every character
/// but one of ASCII, as `[^/]+/` and `[^\n]*\n` are; not where it leaves
/// out two characters, apart or one after another, or one not of ASCII,
/// or matches one character or a few; and no instruction that is no fork
/// of such a loop has one.
#[test]
fn a_loop_of_every_character_but_one_of_ascii_is_ended_by_its_octet() {
    let ends_of = |pattern: &str| -> Vec<u8> {
        let program = Regex::new(pattern, "").unwrap().program;
        assert_eq!(program.passed.ends.len(), program.insts.len());
        for (pc, end) in program.passed.ends.iter().enumerate() {
            assert!(end.is_none() || program.passed.runs[pc], "{pattern:?}: an end of a fork alone");
        }
        program.passed.ends.iter().flatten().copied().collect()
    };
    assert_eq!(ends_of("^https?://(www\\.)?([^/]+)/.*$"), [b'/']);
    assert_eq!(ends_of("[^\\n]*\\n"), [b'\n']);
    assert_eq!(ends_of("[^/]+/x[^,]*,"), [b'/', b',']);
    for pattern in ["[^/,]+/", "[^a-c]+a", "[^é]+é", "a*b", "[a-c]+d", "[^/]+", "[^/]+?/"] {
        assert_eq!(ends_of(pattern), [], "{pattern:?}");
    }
    assert_eq!(ends(&[Inst::Match], &[true], LOOP_TAKES), [None], "a fork with no instruction after it");
}

/// Verifies: [DESIGN] (RULES.md D1312) a straight run is of instructions
/// one after another that each match one given character of ASCII, and
/// ends before one two ways can meet at, which begins a run of its own;
/// a class, a character not of ASCII and an instruction that reads none
/// are of no run. Of `ab(c|cd)é` the first two are a run of two and each
/// branch's characters a run of their own.
#[test]
fn a_straight_run_ends_where_two_ways_can_meet() {
    let insts = [
        Inst::Save(0),
        Inst::Char('a'),
        Inst::Char('b'),
        Inst::Split(4, 6),
        Inst::Char('c'),
        Inst::Jump(8),
        Inst::Char('c'),
        Inst::Char('d'),
        Inst::Char('é'),
        Inst::Set(CharSet::single('x')),
        Inst::Char('y'),
        Inst::Match,
    ];
    let (octets, runs) = straight(&insts, &meets(&insts, None));
    assert_eq!(octets, [0, b'a', b'b', 0, b'c', 0, b'c', b'd', 0, 0, b'y', 0]);
    assert_eq!(runs, [0, 2, 1, 0, 1, 0, 2, 1, 0, 0, 1, 0]);
    let joined = [Inst::Char('a'), Inst::Char('b'), Inst::Char('c'), Inst::Jump(1)];
    assert_eq!(straight(&joined, &meets(&joined, None)).1, [1, 2, 1, 0], "a jump leads to the second, so the first ends before it");
}
