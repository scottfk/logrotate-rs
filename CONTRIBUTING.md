# Contributing

Thanks for stopping by. This is a personal project, a Rust port of
[logrotate](https://github.com/logrotate/logrotate) that I tinker with in my spare time,
but contributions are welcome. Here's how things work.

## What this project is

logrotate-rs aims to be a **drop-in replacement** for C logrotate. Given the same
arguments, configuration, logs, state file and clock, it should produce the same output,
exit status, state file and rotated files, down to the byte, the owner, the mode and the
timestamp. It is not a redesign: if upstream has an odd edge case, we copy the odd edge
case and list it in QUIRKS.md.

The upstream release we track is pinned in `tools/REFERENCE_TAG`, built the way Gentoo
builds it (see the README). Behavior is checked against a build of that tag, not against
the man pages or against what seems sensible.

The layout follows upstream's sources (`config.c`, `logrotate.c`, `log.c`); the README
describes it as it grows.

## What's useful to contribute

- **Behavior differences.** If logrotate-rs does something different from C logrotate at
  the pinned tag, that's a bug. Please send the configuration, the command, the files
  involved, the time it happened (or the clock setting), and both outcomes. A new
  differential test case is even better.
- **Quirk fixes, as separate changes.** Fixing something listed in QUIRKS.md is welcome,
  but as its own PR that says which quirk it fixes. Ideally it gets fixed upstream too.
- **Reference bumps.** Moving `tools/REFERENCE_TAG` to a newer upstream release, plus the
  fixes needed so the tests pass again.
- **Documentation.** Corrections or additions, especially install notes for different
  distros and schedulers (cron, systemd timers).

## What doesn't fit

- **"Improvements" that change behavior quietly.** New directives, changed defaults and
  nicer messages belong upstream in logrotate first. Once they ship in a tagged release,
  we'll match them.
- **New dependencies** without a strong reason. The only crate is `libc`, and I'd like to
  keep it that way. popt and libacl are the same system libraries upstream links.
- **`unsafe` outside the `sys` module.** If you need a new syscall, add a safe wrapper
  there.

## Building and testing

You need Rust 1.88 or newer (edition 2024), a C compiler, and the popt and libacl headers
and libraries. For the tests you also need the tools to build upstream logrotate (`git`,
`autoconf`, `automake`, `pkg-config`, and Gentoo's `ignore-hidden` patch, which the
reference build takes from the ::gentoo tree), unprivileged user namespaces, and a
subordinate id range of at least 65536 for your user in `/etc/subuid` and `/etc/subgid`
(with `newuidmap`/`newgidmap` from shadow).

```sh
tools/build-reference.sh   # build the C reference at the pinned tag into ../logrotate
```

The `make` targets (`make`, `make check`, `make clippy`, `make fmt`) arrive with the
first code. **Before sending a PR, please make sure `make check`, `make clippy` and
`make fmt` are clean.**

### Differential tests

Most testing is done by running the C reference and logrotate-rs side by side on the
same input, the way [metalog-rs](https://github.com/scottfk/metalog-rs) and
[cronie-rs](https://github.com/scottfk/cronie-rs) do it:

- Each run is in a private user and mount namespace with real users mapped from your
  subordinate ids, and its own copy of the case's tree.
- The clock comes from an LD_PRELOAD shim, so "does this log rotate today" and the
  `dateext` names are deterministic.
- The exit status, stdout, stderr, the state file, mail, scripts run, and every file
  left behind (contents, owner, mode, ACLs, times) are compared.

Upstream's own test suite is run against the port as well.

When you fix a difference or change behavior, add cases that cover it, including error
paths and odd input. Unit tests are fine for internal helpers, but a differential test is
what shows the behavior matches.

## Code conventions

- **Port, don't paraphrase.** Each function's doc comment names the upstream function it
  ports (e.g. `` `findNeedRotating()` ``). Keep the structure close enough to the C that
  someone can read the two side by side.
- **Bytes, not strings.** Configuration lines, paths, user names and script text are
  `Vec<u8>`/`&[u8]`, as they are in C. Don't round-trip through `String` where non-UTF-8
  input could show up.
- **Mark quirks.** Reproduced upstream bugs get a `QUIRK:` comment in the code and a row
  in QUIRKS.md.
- **Every `unsafe` block gets a `// SAFETY:` comment.** The lints enforce this
  (`undocumented_unsafe_blocks`, `unsafe_op_in_unsafe_fn`).
- **US-English spelling** in comments, docs and identifiers, except where a name or
  string must match upstream.
- **Script headers.** Shell scripts, C shims and the Makefile have a changelog comment at
  the top, one line per change: `YYYYMMDD - <description> - <author>`. Please add a line
  when you change one.

## How to submit changes

The repo lives at <https://github.com/scottfk/logrotate-rs>.

1. Fork the repo and create a branch.
2. Make your change, with tests.
3. In the PR description, explain **what** changed and **why**. For a behavior fix,
   include the input that showed the difference.
4. If you bumped or tested against a different upstream tag, say which one.

There's no CLA. Contributions are accepted under the project's license (see
[LICENSE](LICENSE)), the same as upstream logrotate's.

## Issues

You can open issues for questions, not just bugs. If something about the build or the
reference setup is confusing, ask. I can't promise a fast response, but I do read them.

## Code of conduct

Be straightforward and respectful. That's the whole policy. See
[CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md).
