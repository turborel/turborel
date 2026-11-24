//! [ISO/IEC 9075-2:2023 §9.18 SR4, SR5d, SR5h i 2] The `<named argument
//! specification>`s of a `<routine invocation>` (Features T521 and T524):
//! all its arguments named or none, no name twice, and, of a candidate
//! routine, each argument put at its parameter's place. A function's
//! parameter has no default value (Feature T525), so a candidate function
//! is one whose every parameter is named; of a procedure, one whose every
//! parameter is named or has a default value (Features T522 and T523)
//! [SR9d ii].

use super::rule;
use crate::error::Result;

/// [§9.18 SR4] That the arguments of `names` are all named or none.
///
/// # Errors
///
/// SQLCODE -103 if some are and some are not.
pub(crate) fn all_or_none(names: &[Option<String>]) -> Result<()> {
    let given = names.iter().filter(|n| n.is_some()).count();
    if given == 0 || given == names.len() {
        return Ok(());
    }
    Err(rule("[ISO/IEC 9075-2:2023 §9.18 SR4] an <SQL argument list> that names one argument names every argument"))
}

/// [§9.18 SR5d, SR5h i] The `arguments`, named by `names`, in the order of
/// the `parameters` of a candidate routine: as written if none is named;
/// none if they are, and a parameter is named by no argument or an
/// argument names no parameter [SR5d i, ii].
pub(crate) fn arranged<'a, A>(parameters: &[&str], arguments: &'a [A], names: &[Option<String>]) -> Option<Vec<&'a A>> {
    if names.iter().all(Option::is_none) {
        return Some(arguments.iter().collect());
    }
    if parameters.len() != arguments.len() {
        return None;
    }
    let at = |p: &str| names.iter().position(|n| n.as_deref() == Some(p));
    parameters.iter().map(|p| at(p).and_then(|k| arguments.get(k))).collect()
}

/// [§9.18 SR9b, SR9d, SR9h i] Of each of the `parameters` of a procedure,
/// each its name and whether it has a default value, the place of its
/// argument among those `names` names: as written if none is named, else
/// the one naming it. None, as DEFAULT, of a parameter past those written,
/// or named by none [SR9h i 1 B, 2 B]. None if the procedure is no
/// candidate: it has fewer parameters than there are arguments [SR9b i],
/// a parameter of no default value takes DEFAULT so [SR9b ii, SR9d ii],
/// or an argument names none of them [SR9d i].
pub(crate) fn placed(parameters: &[(&str, bool)], names: &[Option<String>]) -> Option<Vec<Option<usize>>> {
    if parameters.len() < names.len() {
        return None;
    }
    let named = names.iter().any(Option::is_some);
    if named && names.iter().flatten().any(|n| parameters.iter().all(|(p, _)| p != n)) {
        return None;
    }
    let at = |(k, (p, _)): (usize, &(&str, bool))| if named { names.iter().position(|n| n.as_deref() == Some(*p)) } else { (k < names.len()).then_some(k) };
    let places: Vec<Option<usize>> = parameters.iter().enumerate().map(at).collect();
    places.iter().zip(parameters).all(|(place, (_, defaulted))| place.is_some() || *defaulted).then_some(places)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [ISO/IEC 9075-2:2023 §9.18 SR4, SR5d] arguments all
    /// named are put at their parameters' places, those of none as
    /// written; a list naming some is refused (SQLCODE -103), and one
    /// naming a parameter the routine lacks, or missing one, fits none.
    /// [§9.18 SR9b, SR9d] a procedure's parameter of a default value may
    /// be named by none, or left past those written; one of none may not.
    #[test]
    fn named_arguments_take_their_parameters_places() {
        let named = |n: &[&str]| n.iter().map(|s| Some((*s).to_owned())).collect::<Vec<_>>();
        assert_eq!(arranged(&["A", "B"], &["a", "b"], &named(&["B", "A"])), Some(vec![&"b", &"a"]));
        assert_eq!(arranged(&["A", "B"], &["a", "b"], &[None, None]), Some(vec![&"a", &"b"]));
        assert_eq!(arranged(&["A", "B"], &["a", "b"], &named(&["A", "C"])), None);
        assert_eq!(arranged(&["A", "B", "C"], &["a", "b"], &named(&["A", "B"])), None);
        assert!(all_or_none(&named(&["A", "B"])).is_ok());
        let (a, b) = (("A", false), ("B", true));
        assert_eq!(placed(&[a, b], &named(&["B", "A"])), Some(vec![Some(1), Some(0)]));
        assert_eq!(placed(&[a, b], &named(&["A"])), Some(vec![Some(0), None]));
        assert_eq!(placed(&[a, b], &[None]), Some(vec![Some(0), None]));
        assert_eq!(placed(&[a, b], &named(&["B"])), None);
        assert_eq!(placed(&[b, a], &[None]), None);
        assert_eq!(placed(&[a, b], &named(&["A", "C"])), None);
        assert_eq!(placed(&[a], &[None, None]), None);
        assert_eq!(all_or_none(&[Some("A".to_owned()), None]).unwrap_err().sqlcode(), -103_i32);
    }
}
