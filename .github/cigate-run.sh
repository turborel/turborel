#!/bin/bash
set -o pipefail
cargo fmt --all --check && cargo run -q -p xtask -- file-length && cargo run -q -p xtask -- trace >/dev/null && cargo clippy -q --workspace --all-targets --features turborel-test-support/replication -- -D warnings && cargo test -p xtask 2>&1 | grep -E 'test result|FAILED|failed|panicked' | head -12 && cargo test -p turborel --lib -- exec::table exec::parallel::grouped exec::batch engine::rows engine::columns 2>&1 | grep -E 'test result|FAILED|failed|panicked' | head -12 && cargo test -p turborel --test integration -- grouped_parts regrouped_parts crossed_parts ungrouped_parts spilled_groups pair_top distinct_aggregates column_storage 2>&1 | grep -E 'test result|FAILED|failed|panicked' | head -12 && python3 - xtask/src/mutate/list/reads.rs header-of-a-pass-held <<'PY'
# mut.py LIST.rs [NAME...]: run in a COPY of the tree (never the worktree). Applies each mutation of the list file in turn (or
# those named), runs its unit tests, says CAUGHT if a test failed and MISSED if all passed, and restores the file.
import re, subprocess, sys
src = open(sys.argv[1]).read(); only = set(sys.argv[2:])
consts = dict(re.findall(r'const (\w+): &str =\s*"([^"]+)";', src))
def lit(t):
    t = t.strip()
    return consts[t] if t in consts else bytes(t.strip('"'), 'utf-8').decode('unicode_escape')
for m in re.finditer(r'name: "([^"]+)",.*?file: ([^,]+),\s*original: ("(?:[^"\\]|\\.)*"),\s*replacement: ("(?:[^"\\]|\\.)*"),.*?unit: &\[([^\]]*)\]', src, re.S):
    name, path, original, replacement = m.group(1), lit(m.group(2)), lit(m.group(3)), lit(m.group(4))
    tests = [lit(t) for t in m.group(5).split(',') if t.strip()]
    if only and name not in only: continue
    text = open(path).read()
    if text.count(original) != 1:
        print(f"MUTATION {name}: original occurs {text.count(original)} times, NOT APPLIED"); continue
    open(path, "w").write(text.replace(original, replacement))
    verdicts = []
    for test in tests:
        run = subprocess.run(["cargo", "test", "-p", "turborel", "--lib", "--", test, "--exact"], capture_output=True, text=True)
        out = run.stdout + run.stderr
        verdicts.append("CAUGHT" if "test result: FAILED" in out else ("MISSED (test passed)" if "test result: ok. 1 passed" in out else "UNCLEAR " + out[-300:]))
    print(f"MUTATION {name}: {'; '.join(verdicts)}"); sys.stdout.flush()
    open(path, "w").write(text)
print("MUT-DONE")
PY
