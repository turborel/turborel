#!/bin/bash
# Scratch, never merged: the plans D1323 changes. Every plan the in-repo OLTP
# bench builds (both procedure sets, the eight scripts, one thread, short runs)
# and the 22 decision-support plans on both stores, with the branch's
# transitivity.rs (new) and main's (base). Prints per workload the plannings,
# the plans made with slots as sets, and their differences.
set -u -o pipefail
BASE=$1
perl -0pi -e 's/(        let plan = super::estimate::valued\(self.values, \|\| PhysicalPlanner \{ est: Some\(&estimates\), ..\*self \}.build\(logical, &\[\]\).plan\);\n        )self.pruned\(plan\)/$1let plan = self.pruned(plan);\n        if std::env::var_os("FMO_PLANS").is_some() {\n            eprintln!("FMOPLAN {plan:?}");\n        }\n        plan/' src/physical/cardinality.rs
[ "$(grep -c FMOPLAN src/physical/cardinality.rs)" = 1 ] || { echo NO-PRINT; exit 5; }
O=/tmp/st; mkdir -p $O
norm() { sed -E 's/Decimal \{ units: -?[0-9]+, scale: [0-9]+ \}/D/g; s/[0-9]+\.[0-9]+(e-?[0-9]+)?/F/g; s/rows: [0-9]+/rows: N/g; s/Char\("[^"]*"\)/C/g; s/Approx\(F\)/A/g'; }
CASES="tpcc:old tpcc:new point_select:- read_only:- read_write:- write_only:- update_index:- update_non_index:- insert:- delete:-"
dump() { # dump TAG
  local tag=$1 c w p
  cargo bench -q --bench oltp --no-run 2>&1 | grep -E "^error" -A5 | head -20
  for c in $CASES; do
    w=${c%%:*}; p=${c#*:}; [ "$p" = - ] && p=old
    FMO_PLANS=1 TURBOREL_OLTP_WORKLOADS=$w TURBOREL_OLTP_PROCEDURES=$p TURBOREL_OLTP_THREADS=1 TURBOREL_OLTP_SECS=0.5 \
      TURBOREL_OLTP_TABLE_SIZE=10000 TURBOREL_OLTP_SYNC=off TURBOREL_OLTP_POOL_MB=512 \
      timeout 900 cargo bench -q --bench oltp > $O/$tag-$c.out 2> $O/$tag-$c.err || { echo "BENCH-FAILED $tag $c"; grep -v FMOPLAN $O/$tag-$c.err | tail -20; }
    grep FMOPLAN $O/$tag-$c.err | grep "Dynamic" | norm | sort | uniq -c | sed -E 's/^ +//' > $O/$tag-$c.slotted
    grep FMOPLAN $O/$tag-$c.err | grep -v "Dynamic" | norm | sort | uniq -c | sed -E 's/^ +//' > $O/$tag-$c.valued
    grep FMOPLAN $O/$tag-$c.err | grep -F 'COUNT(DISTINCT STOCK.S_I_ID)' | awk '/Dynamic/ && s < 3 { s++; print "SLOT " $0; next } !/Dynamic/ && v < 3 { v++; print "VALUE " $0 }' \
      | while read -r kind line; do echo "EST $tag $c $kind $(echo "$line" | grep -oE '^FMOPLAN [A-Za-z]+|[A-Z][A-Za-z]*(Join|Loop|Scan) \{|est: [0-9.e-]+|rows: [0-9]+|ident: \"[A-Z_]+\"' | tr '\n' ' ')"; done
    tail -3 $O/$tag-$c.out | sed "s/^/OUT $tag $c /"
    echo "PLANNINGS $tag $c all $(grep -c FMOPLAN $O/$tag-$c.err) slotted $(grep FMOPLAN $O/$tag-$c.err | grep -c Dynamic) shapes $(wc -l < $O/$tag-$c.slotted) txns $(grep -oE '[0-9.]+ tps|tps[^,]*' $O/$tag-$c.out | head -1)"
  done
  cargo test -q --test integration --no-run 2>&1 | grep -E "^error" -A5 | head -20
  for u in "USING columnstore:c" ":r"; do
    FMK_TPCH=$PWD/scratch/fmk-tpch FMK_USING="${u%%:*}" cargo test -q --test integration -- --ignored --exact fmk_tpch::fmk_tpch_plans --nocapture \
      2>/dev/null | grep -v "test result" | sed -E 's/cost=[0-9.]+//g; s/est\. rows[^)]*//g' > $O/$tag-tpch-${u#*:}.plans
    echo "TPCH $tag ${u#*:} plans $(grep -c '^===' $O/$tag-tpch-${u#*:}.plans) errors $(grep -c 'ERROR' $O/$tag-tpch-${u#*:}.plans) lines $(wc -l < $O/$tag-tpch-${u#*:}.plans)"
  done
}
dump new
git checkout $BASE -- src/optimizer/transitivity.rs
grep -c "Held::Place" src/optimizer/transitivity.rs
dump base
for c in $CASES; do
  for k in slotted valued; do
    echo "=== $k $c: base shapes $(wc -l < $O/base-$c.$k) plannings $(awk '{n+=$1} END {print n+0}' $O/base-$c.$k), new shapes $(wc -l < $O/new-$c.$k) plannings $(awk '{n+=$1} END {print n+0}' $O/new-$c.$k)"
    diff <(sed -E 's/^[0-9]+ //' $O/base-$c.$k | sort -u) <(sed -E 's/^[0-9]+ //' $O/new-$c.$k | sort -u) > $O/d.txt
    grep -c '^[<>]' $O/d.txt | sed 's/^/DIFFLINES /'
    grep '^[<>]' $O/d.txt | while read -r side line; do echo "$side $(grep -cF "$line" $O/base-$c.$k)/$(grep -cF "$line" $O/new-$c.$k) n=$(grep -F "$line" $O/$( [ "$side" = "<" ] && echo base || echo new)-$c.$k | head -1 | cut -d' ' -f1) $line"; done | cut -c1-12000
  done
done
for s in c r; do
  if cmp -s $O/base-tpch-$s.plans $O/new-tpch-$s.plans; then echo "TPCH SAME $s"; else echo "TPCH CHANGED $s"; diff $O/base-tpch-$s.plans $O/new-tpch-$s.plans | head -40; fi
done
echo ST-PLANS-END
