# Gentoo: app-admin/logrotate from logrotate-rs

`app-admin/logrotate/logrotate-3.22.0_p9999.ebuild` is a live ebuild. It
installs logrotate 3.22.0 with `/usr/bin/logrotate` built from this
repository's `main`. Everything else is the same as Gentoo's
`app-admin/logrotate-3.22.0-r1`:

- Upstream's release is configured exactly as Gentoo does it: the same
  distfile and hashes (signature checked with USE=verify-sig) and the same
  `econf` arguments. Configure writes the man page (`logrotate.8`, with the
  state file path filled in). Nothing of the C program is compiled.
- The release's `examples/` supply `/etc/cron.daily/logrotate` and the
  systemd service and timer, with Gentoo's `/usr/sbin` to `/usr/bin` fix.
- `/etc/logrotate.conf` and the tmpfiles entry are Gentoo's, copied into
  `files/`. `/etc/logrotate.d` is kept, and the old state file is moved as
  Gentoo's `pkg_postinst` does.

Notes:

- It keeps the package name `app-admin/logrotate`, so anything that
  depends on logrotate is satisfied as before.
- `cron` is the only USE flag. logrotate-rs ports the build with ACLs and
  without SELinux, so the `acl` and `selinux` flags are gone; popt and
  libacl are always linked.
- The only crate is `libc`, fetched like a normal distfile (`CRATES`), so no
  `-network-sandbox` is needed. If logrotate-rs's `Cargo.lock` changes,
  update `CRATES` and regenerate the Manifest.
- Nothing runs logrotate as a daemon: the next cron.daily run (or `logrotate`
  by hand) uses the new program. No service restart is needed.

It was rehearsed on the test VM (snapshot `logrotate-base`).
[tools/vm-logrotate-check.sh](../../tools/vm-logrotate-check.sh) makes a
test tree once (`setup`): logs with older rotations, a state file, a
config using real users and `su`, ACLs, gzip and xz, `dateext`, `olddir`,
`renamecopy`, `shred`, `maxage`, shared scripts, mail and an `include`d
directory (with a hidden and a taboo file). Each `capture` restores that
tree and records:

- `--version`, `--usage`, `--help` and a bad option;
- a run on the state file, the state it leaves, then a forced run;
- every file left: type, mode, owner, group, ACLs, contents (decompressed);
- a dry run (`-d -f`) of the system's `/etc/logrotate.conf`;
- a real `/etc/cron.daily/logrotate` run and the system state file.

The rehearsal went:

1. Install Gentoo's C build (from the binhost, USE="acl cron -selinux"),
   `quickpkg` it, and capture twice. The captures match, so the check
   repeats.
2. Switch to this ebuild and capture.
3. Roll back to C from the binary package only (`--usepkgonly`), and capture.
4. Reinstall this ebuild and capture.

All five captures were the same apart from the line that says which program
ran. The files the two packages install were also compared (checksums,
modes and owners): the man page, the cron script, the systemd units,
`/etc/logrotate.conf` and the tmpfiles entry are identical. Only
`/usr/bin/logrotate` and the doc directory (`${PF}`, plus `logrotate-rs/`)
differ.

## Setup (once)

Put the package in a local overlay (here `localrepo`), then generate the
Manifest:

```sh
cp -r contrib/gentoo/app-admin/logrotate /var/db/repos/localrepo/app-admin/
ebuild /var/db/repos/localrepo/app-admin/logrotate/logrotate-3.22.0_p9999.ebuild manifest
```

Then, in `/etc/portage`, *add* these lines and keep the ones you already
have:

```sh
# package.accept_keywords: it's a live ebuild
=app-admin/logrotate-3.22.0_p9999 **
# package.mask: logrotate-rs ports exactly 3.22.0, so nothing newer may replace it
>app-admin/logrotate-3.22.0_p9999
```

## Before switching: a binary package of the C logrotate

```sh
quickpkg --include-config=y app-admin/logrotate
```

If quickpkg fails with "binpkg appears unsigned", Portage is set to require
signed binary packages (`binpkg-request-signature`, common with Gentoo's
binhost). Run it as `FEATURES=-binpkg-request-signature quickpkg ...`, and
do the same for the `--usepkgonly` emerge below. hydra doesn't set that
feature.

## Switching

```sh
emerge -1 =app-admin/logrotate-3.22.0_p9999
logrotate -d /etc/logrotate.conf     # a dry run: reads the config and state, changes nothing
```

The state file (`/var/lib/misc/logrotate.status`), `/etc/logrotate.conf`
and `/etc/logrotate.d` are read as they are: logrotate-rs uses the same
formats.

## Going back

```sh
emerge -1 --usepkgonly =app-admin/logrotate-3.22.0-r1   # or without --usepkgonly, to build it
```

The differences from C that logrotate-rs keeps on purpose (upstream bugs
ported as they are) are in QUIRKS.md, installed in
`/usr/share/doc/logrotate-3.22.0_p9999/logrotate-rs/`.
