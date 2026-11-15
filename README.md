# logrotate-rs

A drop-in port/reimplementation of logrotate in rust. Currently just something fun for me to tinker with in Claude.

## Goals

- **Drop-in compatible with logrotate 3.22.0** (Gentoo's `app-admin/logrotate`):
  the same options, messages and exit statuses, the same configuration
  syntax, the same state file, and logs rotated, compressed, mailed and
  removed at the same times, with the same names, owners, modes and ACLs.
  Upstream quirks are reproduced on purpose and listed in
  [QUIRKS.md](QUIRKS.md). Improvements come later, as deliberate changes.
- **Linux only.**
- **Small and boring:** `libc` as the only crate dependency, the same system
  libraries upstream links (popt for the command line, libacl), no async,
  and `unsafe` confined to one `sys` module.

## Status

Just started: project docs and the reference build. No code yet.

The reference is upstream's tag `3.22.0`, built as Gentoo builds it
([tools/build-reference.sh](tools/build-reference.sh)):

- USE="acl cron -selinux";
- Gentoo's `ignore-hidden` patch: in an `include`d directory, files whose
  names start with `.` are skipped (upstream skips only `.` and `..`);
- the state file at `/var/lib/misc/logrotate.status`;
- the defaults `/bin/mail`, `/bin/gzip`, `/bin/gunzip` and `.gz`.

The build reports itself as `logrotate 3.22.0`, like the release tarball,
whose sources are the same as the tag's. The C reference passes upstream's
own test suite (106 tests pass and 1 is skipped).

## Planned layout

Upstream is three C files (about 6,700 lines), and the port follows them:

| Path | What |
|---|---|
| `src/config.rs` | `config.c`: the configuration parser, `include`, globs, taboo extensions, per-log defaults |
| `src/state.rs` | `readState()`/`writeState()` and the state lock |
| `src/rotate.rs` | `logrotate.c`: deciding what needs rotating, renaming, `dateext`, `olddir`, `create`/`copy`/`copytruncate`, compression, scripts, mail |
| `src/log.rs` | `log.c`: messages to stderr, `-l` files and syslog |
| `src/sys.rs` | every libc, popt and libacl call: the only `unsafe` code |
| `src/main.rs` | `main()`: options, the state file, then each log set |
| `tests/` | differential tests and the harness |
| `contrib/gentoo/` | an `app-admin/logrotate` live ebuild, and how to switch and roll back |
| `tools/` | the reference build |

## Testing (planned)

`make check` will build the C reference into `../logrotate` if it isn't
there, then run two suites:

1. **Differential tests.** Each case runs C logrotate and the port on
   copies of the same tree (config, logs, state file) and compares the exit
   status, stdout, stderr, the state file, syslog messages, mail and every
   file left behind: names, contents, owners, modes, ACLs and times.
   - Each run is in a private user and mount namespace, with ids mapped from
     your subordinate ranges, so there is a real root and real other users
     (`su`, `create 0640 alice adm` and so on), as in
     [cronie-rs](https://github.com/scottfk/cronie-rs).
   - The clock is an LD_PRELOAD shim, so `daily`, `weekly`, `maxage`,
     `dateext` and the state file's dates are deterministic, DST included.
   - `gzip`, `mail` and scripts are real programs in the sandbox, so what
     they were run with and on is compared too.
   - `-d` (debug) prints the whole parsed configuration, which makes the
     parser easy to compare on its own.
2. **Upstream's test suite**, run against the port with
   `LOGROTATE=target/release/logrotate`.

## Roadmap

1. **Configuration and the command line:** `config.c`, popt's options and
   messages, `-d` output, and reading and writing the state file.
2. **Rotation:** what needs rotating and when (`daily` ... `yearly`,
   `size`, `minsize`, `maxsize`, `maxage`), the rename chain, `dateext`
   and `dateformat`, `olddir`, `create`, `copy`, `copytruncate`,
   compression, `shred`, scripts (`sharedscripts`, `firstaction`,
   `lastaction`, `preremove`), mail and the `-l` log.
3. **Users, ownership and ACLs:** `su`, `create` with an owner and group,
   the ACL copies, and the permission checks on config files and
   directories, tested with real users in the sandbox. Upstream's test
   suite passes against the port.
4. **Packaging:** a Gentoo ebuild (an `app-admin/logrotate` drop-in), with
   a VM rehearsal and a rollback through a binary package of the C build.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md), [SECURITY.md](SECURITY.md) and
[CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

## License

The same license as upstream logrotate. [LICENSE](LICENSE) is upstream's
`COPYING` (the GNU GPL, version 2), unchanged. Upstream's source files don't
name a version, and its RPM spec declares `GPLv2+`, so the project is
GPL-2.0-or-later.
