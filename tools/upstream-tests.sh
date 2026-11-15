#!/bin/bash
# upstream-tests.sh - run upstream logrotate's own test suite against logrotate-rs
#
# 20261115 - Created by Claude (claude.ai, Anthropic) - scottfk
#
# Usage: tools/upstream-tests.sh [REFERENCE_DIR]
#
# Upstream's test/Makefile hard-codes LOGROTATE=$(top_builddir)/logrotate,
# so the suite runs in a copy of the reference build tree
# (target/upstream-tests/) with its logrotate replaced by
# target/release/logrotate.  The copy's binary is newer than its sources,
# so make doesn't rebuild it; the script checks that it is still ours
# afterwards.  Exits nonzero if any test fails.

set -euo pipefail

here=$(cd "$(dirname "$0")/.." && pwd)
ref=${1:-${LOGROTATE_REFERENCE:-$here/../logrotate}}
work=$here/target/upstream-tests
bin=$here/target/release/logrotate

[[ -x $bin ]] || { echo "upstream-tests.sh: build $bin first (make build)" >&2; exit 2; }
[[ -f $ref/test/Makefile ]] || { echo "upstream-tests.sh: no configured reference tree at $ref" >&2; exit 2; }

mkdir -p "$work"
rsync -a --delete --exclude .git "$ref/" "$work/"
cp "$bin" "$work/logrotate"
touch "$work/logrotate"

rc=0
make -C "$work" -s check -j"$(nproc)" >/dev/null 2>&1 || rc=$?

if ! cmp -s "$bin" "$work/logrotate"; then
	echo "upstream-tests.sh: make rebuilt logrotate; the results are not for logrotate-rs" >&2
	exit 2
fi
grep -E '^# (TOTAL|PASS|SKIP|XFAIL|FAIL|XPASS|ERROR):' "$work/test/test-suite.log"
grep -E '^(FAIL|ERROR|XPASS): ' "$work/test/test-suite.log" || true
exit "$rc"
