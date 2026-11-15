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

All four phases are done: the command line, the configuration parser, the
`-l` log and syslog, the state file, rotation itself (every criterion,
numbered and dated names, `olddir`, `create`, `copy`/`copytruncate`/
`renamecopy`, compression, `shred`, scripts, mail and `su`), extended
ACLs, what happens when a step fails partway, and a Gentoo package. The
differential suite and upstream's own test suite both pass, and the package
was rehearsed on a test VM, rollback included
([contrib/gentoo/README.md](contrib/gentoo/README.md)).

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

## Layout

Upstream is three C files (about 6,700 lines), and the port follows them:

| Path | What |
|---|---|
| `src/config.rs` | `config.c`: the configuration parser, `include`, globs, taboo extensions, per-log defaults |
| `src/state.rs` | `logrotate.c`'s state code: the hash table, `readState()`, `lockState()`, `writeState()` |
| `src/rotate.rs` | `logrotate.c`'s rotation code: switching users, what needs rotating, renaming, `dateext`, `olddir`, `create`/`copy`/`copytruncate`, compression, `shred`, scripts, mail |
| `src/log.rs` | `log.c`: messages to stderr, `-l` files and syslog |
| `src/sys.rs` | every libc, popt and libacl call: the only `unsafe` code |
| `src/main.rs` | `main()`: options (through popt), the state file, then each log set |
| `src/bytes.rs` | byte-string helpers, copied from cronie-rs |
| `tests/` | differential tests and the harness ([tests/common/mod.rs](tests/common/mod.rs)) |
| `contrib/gentoo/` | an `app-admin/logrotate` live ebuild, and how to switch and roll back |
| `tools/` | the reference build, the runner for upstream's test suite, and the VM rehearsal check |

## Testing

```sh
make check
```

This builds the C reference into `../logrotate` if it isn't there, then
runs two suites:

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
   - Files the harness makes have fixed modification times, the fake
     `-m` mailer records what it's sent, and compressed files are compared
     decompressed (gzip headers hold times).
   - Each run's directory is bound onto the same path inside its sandbox,
     so both programs see identical paths (the state file's order depends
     on them).
   - Writes that fail partway (a full disk) are made with a file size
     limit (`RLIMIT_FSIZE`, with `SIGXFSZ` ignored): they fail with
     `EFBIG` instead of `ENOSPC`, through the same code.
   - 511 cases: 274 for the command line, parser and state file
     ([tests/diff_config.rs](tests/diff_config.rs)), whose definitions name
     directories so nothing rotates; 160 for rotation
     ([tests/diff_rotate.rs](tests/diff_rotate.rs)); and 77 for ACLs,
     failures midway, unwritable directories and `su`
     ([tests/diff_failures.rs](tests/diff_failures.rs)).
2. **Upstream's test suite** ([tools/upstream-tests.sh](tools/upstream-tests.sh)):
   107 tests, run in a copy of the reference build tree with its
   `logrotate` swapped for the port. 106 pass and the SELinux test is
   skipped, the same as for C logrotate.

To see what each case produced:
`LOGROTATE_DIFF_SHOW=1 cargo test -- --nocapture`. Each run's files stay in
`target/difftest/<case>/{c,r}/`.

## Roadmap

1. **Configuration and the command line (done, branch `phase1-config`):**
   `config.c`, popt's options and messages, the `-l` log, and reading,
   locking and writing the state file.
2. **Rotation (done, branch `phase2-rotate`):** what needs rotating and
   when (`daily` ... `yearly`, `size`, `minsize`, `maxsize`, `maxage`),
   the rename chain, `dateext` and `dateformat`, `olddir`, `create`,
   `copy`, `copytruncate`, compression, `shred`, scripts (`sharedscripts`,
   `firstaction`, `lastaction`, `preremove`), mail and `su`. Upstream's
   test suite passes against the port.
3. **ACLs and the remaining permission paths (done, branch
   `phase3-acl`):** extended ACLs copied to new, rotated, compressed and
   state files (and default ACLs of directories), failures midway (full
   disks, unwritable directories, files that vanish or change type under a
   script), and more `su` cases.
4. **Packaging (done, branch `phase4-ebuild`):** a Gentoo ebuild (an
   `app-admin/logrotate` drop-in), with a VM rehearsal and a rollback
   through a binary package of the C build.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md), [SECURITY.md](SECURITY.md) and
[CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).

## License

The same license as upstream logrotate. [LICENSE](LICENSE) is upstream's
`COPYING` (the GNU GPL, version 2), unchanged. Upstream's source files don't
name a version, and its RPM spec declares `GPLv2+`, so the project is
GPL-2.0-or-later.
