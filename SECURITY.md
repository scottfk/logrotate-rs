# Security

logrotate-rs is a reimplementation of logrotate. It usually runs as root from cron or a
systemd timer, and it works on directories that other users and daemons can write to:
it renames, creates, truncates, compresses and deletes files there, changes their owners
and modes, and runs scripts from its configuration. Security bugs here matter, even
though this is a hobby project.

## Status and supported versions

logrotate-rs is pre-release (0.x). There are no maintained release branches: only the
latest `main` gets fixes.

## What counts as a security issue

These are the kinds of bugs I most want to hear about:

- **Filesystem races and link tricks.** Ways for a user who can write to a log directory
  (or an `olddir`) to make logrotate follow a symlink or hard link, or win a race, so it
  creates, truncates, chowns, chmods or deletes a file it shouldn't.
- **Privilege boundaries.** `su` not taking effect, or not taking effect early enough;
  files created with the wrong owner, group, mode or ACL, even briefly; scripts, the
  compressor or the mailer running with the wrong ids or environment.
- **Configuration checks.** Skipping a check C logrotate does on a configuration file,
  an included directory or a log directory (ownership, permissions, `su` requirements).
- **Untrusted input.** Crashes, hangs or memory growth from log file names, the state
  file, or configuration that a package or a less-trusted admin can install.
- **Memory safety.** Unsoundness in the `unsafe` code, or anything reachable from safe
  code that causes undefined behavior.
- **Divergence from upstream that weakens security.** For example, a permission check,
  an `O_NOFOLLOW`, or a privilege change that C logrotate does and logrotate-rs skips.

Upstream behavior that is reproduced on purpose will be listed in QUIRKS.md. Reports
about it are still welcome, especially if the impact is worse than described. It will be
fixed as deliberate changes.

If the same bug is also present in **upstream C logrotate**, please report it to the
[logrotate project](https://github.com/logrotate/logrotate/security) as well. Upstream's
fix is what logrotate-rs will follow.

## How to report

For anything that could be exploited, **please don't put the details in a public issue.**
Email them to me instead:

    scone-void-baggage (at) duck (dot) com

(Replace `(at)` with `@` and `(dot)` with `.`, and remove the spaces.)

If you don't get a reply, open an issue on <https://github.com/scottfk/logrotate-rs/issues>
that only says you've sent a security report, without any details.

For low-risk problems, such as hardening suggestions or issues that need root already
(a malicious `/etc/logrotate.conf`, for instance), a normal public issue is fine.

Useful things to include:

- The commit you tested.
- The configuration, the files and directories involved (owners and modes), the user
  logrotate ran as, and the exact steps.
- What happened, and what C logrotate does at the pinned tag (`tools/REFERENCE_TAG`).

## What to expect

This is a spare-time project, so I can't promise a response time, but I'll acknowledge
security reports as soon as I can, fix confirmed bugs on `main`, and credit you in the
commit unless you'd rather I didn't.
