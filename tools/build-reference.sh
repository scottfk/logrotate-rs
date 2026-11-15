#!/bin/bash
# build-reference.sh - build upstream C logrotate at the pinned tag, for the differential tests
#
# 20261115 - Created by Claude (claude.ai, Anthropic) - scottfk
#
# Usage: tools/build-reference.sh [DIR]
#
# DIR defaults to ../logrotate (next to this checkout), which is where the
# tests will look (override with LOGROTATE_REFERENCE=/path/to/logrotate).
# Builds the way Gentoo's app-admin/logrotate-3.22.0-r1 does with
# USE="acl cron -selinux": Gentoo's ignore-hidden patch (files in an
# included directory whose names start with '.' are skipped), and the
# ebuild's configure flags and state file path.
#
# The checkout gets a .tarball-version, so the version is "3.22.0" as in
# the release tarball, not git-version-gen's "3.22.0_dirty".

set -euo pipefail

here=$(cd "$(dirname "$0")/.." && pwd)
dir=${1:-$here/../logrotate}
tag=$(cat "$here/tools/REFERENCE_TAG")
url=https://github.com/logrotate/logrotate.git
patch=/var/db/repos/gentoo/app-admin/logrotate/files/logrotate-3.15.0-ignore-hidden.patch

if [[ ! -d $dir/.git ]]; then
	git clone "$url" "$dir"
fi
cd "$dir"
if [[ $(git describe --tags --exact-match 2>/dev/null || true) != "$tag" ]]; then
	git fetch --tags origin
	git checkout -q "$tag"
fi
echo "$tag" > .tarball-version
if [[ ! -f $patch ]]; then
	echo "build-reference.sh: $patch not found (needs a Gentoo ::gentoo tree)" >&2
	exit 1
fi
if patch -p1 -s -N --dry-run < "$patch" >/dev/null 2>&1; then
	patch -p1 -s -N --no-backup-if-mismatch < "$patch"
fi
if [[ ! -x configure ]]; then
	./autogen.sh
fi
if [[ ! -f Makefile ]]; then
	./configure -q --prefix=/usr --sysconfdir=/etc --localstatedir=/var \
		--with-acl --without-selinux \
		--with-state-file-path=/var/lib/misc/logrotate.status
fi
make -s
echo "reference: $dir ($(./logrotate --version 2>&1 | head -1))"
