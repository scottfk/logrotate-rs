# Copyright 1999-2026 Gentoo Authors
# Distributed under the terms of the GNU General Public License v2
# 20261115 - Created by Claude (claude.ai, Anthropic) - scottfk

# logrotate 3.22.0 with the program from logrotate-rs (a strict-compatibility
# Rust port, built live from git).  Upstream's release, configured exactly
# as app-admin/logrotate-3.22.0-r1 does it, supplies the man page, the cron
# script and the systemd units; logrotate.conf and the tmpfiles entry are
# Gentoo's.  Pin the release with a package.mask of
# >app-admin/logrotate-3.22.0_p9999: logrotate-rs ports exactly 3.22.0.

EAPI=8

LOGROTATE_PV="3.22.0"
# logrotate-rs's Cargo.lock: update together.
CRATES="libc@0.2.191"
RUST_MIN_VER="1.88.0"

VERIFY_SIG_OPENPGP_KEY_PATH=/usr/share/openpgp-keys/cgzones.asc
inherit cargo git-r3 systemd tmpfiles verify-sig

DESCRIPTION="Rotates, compresses, and mails system logs (logrotate-rs program)"
HOMEPAGE="https://github.com/scottfk/logrotate-rs https://github.com/logrotate/logrotate"
EGIT_REPO_URI="https://github.com/scottfk/logrotate-rs.git"
SRC_URI="
	https://github.com/logrotate/logrotate/releases/download/${LOGROTATE_PV}/logrotate-${LOGROTATE_PV}.tar.xz
	verify-sig? ( https://github.com/logrotate/logrotate/releases/download/${LOGROTATE_PV}/logrotate-${LOGROTATE_PV}.tar.xz.asc )
	${CARGO_CRATE_URIS}
"
# cargo builds in ${S}: the logrotate-rs checkout.
S="${WORKDIR}/logrotate-rs"
EGIT_CHECKOUT_DIR="${S}"
# Upstream's release.
UP="${WORKDIR}/logrotate-${LOGROTATE_PV}"

LICENSE="GPL-2+"
# Dependent crate licenses
LICENSE+=" || ( Apache-2.0 MIT )"
SLOT="0"
# logrotate-rs ports the build with ACLs and without SELinux, so those
# flags are fixed.
IUSE="+cron"
# The differential tests need upstream's C build and user namespaces.
RESTRICT="test"

DEPEND="
	>=dev-libs/popt-1.5
	virtual/acl
"
RDEPEND="
	${DEPEND}
	cron? ( virtual/cron )
"
BDEPEND="verify-sig? ( sec-keys/openpgp-keys-cgzones )"

STATEFILE="${EPREFIX}/var/lib/misc/logrotate.status"
OLDSTATEFILE="${EPREFIX}/var/lib/logrotate.status"

move_old_state_file() {
	elog "logrotate state file is now located at ${STATEFILE}"
	elog "See bug #357275"
	if [[ -e "${OLDSTATEFILE}" ]] ; then
		elog "Moving your current state file to new location: ${STATEFILE}"
		mv -n "${OLDSTATEFILE}" "${STATEFILE}" || die
	fi
}

src_unpack() {
	if use verify-sig ; then
		verify-sig_verify_detached "${DISTDIR}"/logrotate-${LOGROTATE_PV}.tar.xz{,.asc}
	fi
	cargo_src_unpack
	git-r3_src_unpack
}

src_prepare() {
	default
	pushd "${UP}" >/dev/null || die
	sed -i -e 's#/usr/sbin/logrotate#/usr/bin/logrotate#' examples/logrotate.{cron,service} || die
	popd >/dev/null || die
}

src_configure() {
	# Upstream's configure, as Gentoo runs it: it writes logrotate.8 (the
	# state file path and mail command substituted).  Nothing of the C
	# program is compiled.
	pushd "${UP}" >/dev/null || die
	econf \
		--with-acl \
		--without-selinux \
		--with-state-file-path="${STATEFILE}"
	popd >/dev/null || die

	cargo_src_configure
}

src_install() {
	dobin "$(cargo_target_dir)"/logrotate
	doman "${UP}"/logrotate.8
	dodoc "${UP}"/ChangeLog.md
	docinto logrotate-rs
	dodoc README.md QUIRKS.md

	insinto /etc
	doins "${FILESDIR}"/logrotate.conf

	if use cron ; then
		exeinto /etc/cron.daily
		newexe "${UP}"/examples/logrotate.cron logrotate
	fi

	systemd_dounit "${UP}"/examples/logrotate.{service,timer}
	newtmpfiles "${FILESDIR}"/logrotate.tmpfiles logrotate.conf

	keepdir /etc/logrotate.d
}

pkg_postinst() {
	move_old_state_file

	tmpfiles_process logrotate.conf

	elog "logrotate is now logrotate-rs's (${EROOT}/usr/bin/logrotate)."
	elog "Its known differences from C are in ${EROOT}/usr/share/doc/${PF}/logrotate-rs/QUIRKS.md*."
	elog "To go back, emerge the C release: emerge -1 =app-admin/logrotate-${LOGROTATE_PV}-r1"
}
