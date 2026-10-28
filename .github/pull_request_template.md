<!--
This description is the series' cover letter (CONTRIBUTING.md, Pull
requests). Each commit message still stands alone, in Linux's form.
Base the series on main, keep it rebased, and on revising say what
changed in a comment, not here.
-->

## What and why

<!-- The problem and its impact, then the approach and the choices made.
Link the issue: "Closes #N". -->

## The series

<!-- One line per commit, in order: what it does. Each passes
`cargo xtask verify` on its own. -->

1.

## Testing

<!-- The tests added and what each verifies. For a change that touches data,
the crash and recovery, isolation, or result checks done. -->

- [ ] `cargo xtask verify --coverage` passes

## Compatibility

<!-- What happens to existing files, the log, clients of the protocol,
settings and SQL behaviour; "none" if nothing changes. -->

## Performance

<!-- If the change claims any: `cargo xtask bench NAME --compare before`,
with the numbers. Otherwise "none claimed". -->

## Checklist (RULES.md §7)

- [ ] One logical change per commit, each passing `cargo xtask verify`
- [ ] Requirements cited in the code (R-DOC-4) and in the tests (`/// Verifies:`, R-TRACE-1)
- [ ] Normal and robustness tests (R-TEST-2), MC/DC pairs for new decisions (R-TEST-8), a failing-first test for a fix (R-TEST-7)
- [ ] No `unsafe`, no panics, no new dependency without a decision (R-SAFE-1, R-ERR, R-SAFE-4)
- [ ] CONFORMANCE.md, RULES.md and TODO.md updated where they change
