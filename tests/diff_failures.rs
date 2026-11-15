//! Differential tests for phase 3: extended ACLs, failures midway, and the
//! permission paths of `su` and of running as an unprivileged user.
//!
//! Every case runs `logrotate -v -s @T@/state logrotate.conf` (unless it
//! says otherwise) over logs in `T/logs`, with a state entry saying
//! `app.log` was rotated yesterday, so `daily` rotates it.  The time is
//! 2025-11-11 14:00 UTC.  ACL specs use numeric ids: 1001 is alice, 1002
//! bob, 4 adm.

mod common;

use common::{ALICE, Case, check};

const CONF: &str = "logrotate.conf";
const LOG: &str = "@T@/logs/app.log";

fn state_text(logs: &[&str]) -> String {
    let mut s = String::from("logrotate state -- version 2\n");
    for f in logs {
        s += &format!("\"{f}\" 2025-11-10-6:0:0\n");
    }
    s
}

/// A case with `conf`, a root-owned logs directory, `-v`, and a state file
/// saying `app.log` was rotated yesterday.
fn r(name: &str, conf: &str) -> Case {
    Case::new(name, &["-v", "-s", "@T@/state", CONF]).conf(CONF, conf).dir("logs", 0o755).file(
        "state",
        state_text(&[LOG]).as_bytes(),
        0o640,
    )
}

/// `r` with `logs/app.log` (root, 0640).
fn rl(name: &str, conf: &str) -> Case {
    r(name, conf).file("logs/app.log", b"line 1\nline 2\n", 0o640)
}

/// A daily definition for app.log with extra directives.
fn daily(directives: &str) -> String {
    format!("@T@/logs/app.log {{\n daily\n rotate 1\n{directives}}}\n")
}

/// Changes to a case's command line.
trait Args {
    /// `-d` instead of `-v`.
    fn args_debug(self) -> Self;
    /// Another state file.
    fn args_state(self, path: &str) -> Self;
}

impl Args for Case {
    fn args_debug(mut self) -> Case {
        for a in &mut self.args {
            if a == "-v" {
                *a = "-d".into();
            }
        }
        self
    }

    fn args_state(mut self, path: &str) -> Case {
        let i = self.args.iter().position(|a| a == "-s").unwrap();
        self.args[i + 1] = path.into();
        self
    }
}

/// Bytes gzip can't shrink much (a fixed pseudo-random sequence).
fn noise(n: usize) -> Vec<u8> {
    let mut x: u32 = 0x1234_5678;
    (0..n)
        .map(|_| {
            x = x.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (x >> 16) as u8
        })
        .collect()
}

// ---------------------------------------------------------------------------
// ACLs

#[test]
fn acl_create() {
    let a = "u:1001:r";
    // No mode given: the new log gets the old one's ACL.
    check(rl("acl_create", &daily(" create\n")).acl("logs/app.log", a));
    check(rl("acl_create_owner_only", &daily(" create 0640 alice adm\n")).acl("logs/app.log", a));
    // A mode: chmod only, no ACL.
    check(rl("acl_create_mode", &daily(" create 0600\n")).acl("logs/app.log", a));
    check(rl("acl_create_mode_same", &daily(" create 0640\n")).acl("logs/app.log", a));
    check(rl("acl_mask", &daily(" create\n")).acl("logs/app.log", "u:1001:rw,g:4:rw,m::r"));
    check(rl("acl_group_entries", &daily(" create\n")).acl("logs/app.log", "g:4:r,g:100:rw"));
    check(rl("acl_nocreate", &daily("")).acl("logs/app.log", a));
    check(rl("acl_create_dateext", &daily(" create\n dateext\n")).acl("logs/app.log", a));
    check(rl("acl_create_dry_run", &daily(" create\n")).acl("logs/app.log", a).args_debug());
}

#[test]
fn acl_copies_and_compression() {
    let a = "u:1001:rw,g:4:r";
    check(rl("acl_compress", &daily(" compress\n")).acl("logs/app.log", a));
    check(rl("acl_compress_create", &daily(" compress\n create\n")).acl("logs/app.log", a));
    check(
        rl("acl_delaycompress", &daily(" rotate 2\n compress\n delaycompress\n"))
            .file("logs/app.log.1", b"older\n", 0o600)
            .acl("logs/app.log.1", "u:1002:r"),
    );
    check(rl("acl_copy", &daily(" copy\n")).acl("logs/app.log", a));
    check(rl("acl_copytruncate", &daily(" copytruncate\n")).acl("logs/app.log", a));
    check(rl("acl_copytruncate_compress", &daily(" copytruncate\n compress\n")).acl("logs/app.log", a));
    check(rl("acl_renamecopy", &daily(" renamecopy\n olddir @T@/old\n")).dir("old", 0o755).acl("logs/app.log", a));
    check(rl("acl_olddir_compress", &daily(" olddir @T@/old\n compress\n")).dir("old", 0o755).acl("logs/app.log", a));
}

#[test]
fn acl_default_directories() {
    // Files made in a directory with a default ACL inherit it, before
    // logrotate sets (or doesn't set) the old file's ACL.
    let d = "d:u:1002:r";
    check(rl("acl_dir_default_create", &daily(" create\n")).acl("logs", d));
    check(rl("acl_dir_default_create_mode", &daily(" create 0600\n")).acl("logs", d));
    check(rl("acl_dir_default_compress", &daily(" compress\n")).acl("logs", d));
    check(rl("acl_dir_default_copy", &daily(" copy\n")).acl("logs", d));
    check(rl("acl_dir_default_both", &daily(" create\n compress\n")).acl("logs", d).acl("logs/app.log", "u:1001:r"));
    check(rl("acl_olddir_default", &daily(" olddir @T@/old\n compress\n")).dir("old", 0o755).acl("old", "d:g:4:rw"));
    check(
        rl("acl_createolddir_default", &daily(" olddir @T@/old/sub\n createolddir 0750 root adm\n"))
            .dir("old", 0o755)
            .acl("old", "d:u:1001:rwx"),
    );
}

#[test]
fn acl_state_file() {
    // The new state file gets the old one's ACL, unless the old one was
    // world-readable (then the mode is forced, without the bit).
    check(rl("acl_state", &daily("")).acl("state", "u:1001:r"));
    check(
        rl("acl_state_world_readable", &daily(""))
            .no_file("state")
            .file("state", state_text(&[LOG]).as_bytes(), 0o644)
            .acl("state", "u:1001:r"),
    );
    check(
        rl("acl_state_dir_default", &daily("")).dir("var", 0o755).acl("var", "d:u:1001:r").args_state("@T@/var/state"),
    );
}

#[test]
fn acl_as_users() {
    let alice_log = |c: Case| {
        c.file_owned("logs/app.log", b"alice's\n", 0o640, 1001, 1001)
            .dir_owned("logs", 0o750, 1001, 1001)
            .acl("logs/app.log", "u:1002:r")
    };
    check(alice_log(r("acl_su_create", &daily(" su alice alice\n create\n compress\n"))));
    check(alice_log(r("acl_su_copytruncate", &daily(" su alice alice\n copytruncate\n"))));
    check(
        alice_log(r("acl_as_alice", &daily(" create\n compress\n")))
            .no_file("state")
            .no_file(CONF)
            .file_owned("home/alice/state", state_text(&[LOG]).as_bytes(), 0o640, 1001, 1001)
            .args_state("@T@/home/alice/state")
            .file_owned(CONF, daily(" create\n compress\n").as_bytes(), 0o644, 1001, 1001)
            .run_as(ALICE),
    );
}

// ---------------------------------------------------------------------------
// writes that fail (a file size limit standing in for a full disk)

#[test]
fn write_failures() {
    let big = noise(4000);
    let f = |c: Case| c.file("logs/app.log", &big, 0o640).fsize(1000);
    check(f(r("fsize_copytruncate", &daily(" copytruncate\n"))));
    check(f(r("fsize_copy", &daily(" copy\n"))));
    check(f(r("fsize_compress", &daily(" compress\n"))));
    check(f(r("fsize_compress_delay", &daily(" rotate 2\n compress\n delaycompress\n")).file(
        "logs/app.log.1",
        &big,
        0o640,
    )));
    check(f(r("fsize_renamecopy", &daily(" renamecopy\n olddir @T@/old\n")).dir("old", 0o755)));
    // Rename-only rotation writes nothing but the state file.
    check(rl("fsize_state", &daily("")).fsize(40));
    check(rl("fsize_state_zero", &daily("")).fsize(0));
}

// ---------------------------------------------------------------------------
// files that change under logrotate

#[test]
fn vanishing_files() {
    check(rl("prerotate_removes_log", &daily(" prerotate\n  rm @T@/logs/app.log\n endscript\n")));
    check(rl("prerotate_removes_log_create", &daily(" create\n prerotate\n  rm @T@/logs/app.log\n endscript\n")));
    check(rl(
        "prerotate_removes_log_copytruncate",
        &daily(" copytruncate\n prerotate\n  rm @T@/logs/app.log\n endscript\n"),
    ));
    check(rl(
        "prerotate_symlinks_log_copytruncate",
        &daily(
            " copytruncate\n prerotate\n  mv @T@/logs/app.log @T@/logs/real; ln -s real @T@/logs/app.log\n endscript\n",
        ),
    ));
    check(rl(
        "prerotate_log_becomes_dir",
        &daily(" create\n prerotate\n  rm @T@/logs/app.log; mkdir @T@/logs/app.log\n endscript\n"),
    ));
    check(rl("firstaction_removes_log", &daily(" sharedscripts\n firstaction\n  rm @T@/logs/app.log\n endscript\n")));
    check(rl(
        "postrotate_removes_rotated_compress",
        &daily(" compress\n postrotate\n  rm @T@/logs/app.log.1\n endscript\n"),
    ));
    check(
        rl(
            "prerotate_removes_older_delaycompress",
            &daily(" rotate 3\n compress\n delaycompress\n prerotate\n  rm @T@/logs/app.log.1\n endscript\n"),
        )
        .file("logs/app.log.1", b"older\n", 0o640),
    );
    check(
        rl("prerotate_removes_olddir", &daily(" olddir @T@/old\n prerotate\n  rmdir @T@/old\n endscript\n"))
            .dir("old", 0o755),
    );
    check(rl("prerotate_fills_target", &daily(" prerotate\n  mkdir @T@/logs/app.log.1\n endscript\n")));
    check(rl(
        "prerotate_creates_compressed",
        &daily(" compress\n prerotate\n  echo x > @T@/logs/app.log.1.gz\n endscript\n"),
    ));
    check(rl("lastaction_removes_state", &daily(" sharedscripts\n lastaction\n  rm @T@/state\n endscript\n")));
    check(rl(
        "lastaction_state_becomes_dir",
        &daily(" sharedscripts\n lastaction\n  rm @T@/state; mkdir @T@/state\n endscript\n"),
    ));
    check(rl("state_tmp_is_dir", &daily("")).dir("state.tmp", 0o755));
    check(rl("state_is_dir", &daily("")).no_file("state").dir("state", 0o755));
    check(rl("state_dir_missing", &daily("")).args_state("@T@/none/state"));
}

// ---------------------------------------------------------------------------
// unwritable directories, running as alice

#[test]
fn unwritable_as_user() {
    let alice = |c: Case, conf: &str| {
        // The state file in alice's home: T itself is root's.
        c.no_file("state")
            .no_file(CONF)
            .file_owned("home/alice/state", state_text(&[LOG]).as_bytes(), 0o640, 1001, 1001)
            .args_state("@T@/home/alice/state")
            .file_owned(CONF, conf.as_bytes(), 0o644, 1001, 1001)
            .file_owned("logs/app.log", b"alice's\n", 0o644, 1001, 1001)
            .run_as(ALICE)
    };
    let conf = daily("");
    check(alice(r("user_dir_root_owned", &conf), &conf));
    let conf = daily(" copytruncate\n");
    check(alice(r("user_dir_root_owned_copytruncate", &conf), &conf));
    let conf = daily(" olddir @T@/old\n");
    check(alice(r("user_olddir_root_owned", &conf).dir_owned("logs", 0o755, 1001, 1001).dir("old", 0o755), &conf));
    let conf = daily(" olddir @T@/old/sub\n createolddir\n");
    check(alice(r("user_createolddir_denied", &conf).dir_owned("logs", 0o755, 1001, 1001).dir("old", 0o755), &conf));
    let conf = daily(" create 0640 root root\n");
    check(alice(r("user_create_chown_ignored", &conf).dir_owned("logs", 0o755, 1001, 1001), &conf));
    let conf = daily(" compress\n");
    check(alice(
        r("user_rotates_root_owned_file", &conf).dir_owned("logs", 0o755, 1001, 1001).file(
            "logs/app.log.1.gz",
            b"",
            0o644,
        ),
        &conf,
    ));
    let conf = daily("");
    check(
        alice(r("user_state_dir_root_owned", &conf).dir_owned("logs", 0o755, 1001, 1001), &conf)
            .dir("var", 0o755)
            .args_state("@T@/var/state"),
    );
    let conf = daily("");
    check(
        alice(r("user_state_root_owned", &conf).dir_owned("logs", 0o755, 1001, 1001), &conf)
            .no_file("home/alice/state")
            .file("home/alice/state", state_text(&[LOG]).as_bytes(), 0o644),
    );
}

// ---------------------------------------------------------------------------
// su

#[test]
fn su_permissions() {
    let alice_dir =
        |c: Case| c.file_owned("logs/app.log", b"alice's\n", 0o640, 1001, 1001).dir_owned("logs", 0o755, 1001, 1001);
    check(alice_dir(r("su_olddir_root_owned", &daily(" su alice alice\n olddir @T@/old\n")).dir("old", 0o755)));
    check(alice_dir(r(
        "su_createolddir",
        &daily(" su alice alice\n olddir @T@/logs/old\n createolddir 0700 alice alice\n"),
    )));
    check(alice_dir(
        r(
            "su_createolddir_root_parent",
            &daily(" su alice alice\n olddir @T@/old/sub\n createolddir 0700 alice alice\n"),
        )
        .dir("old", 0o755),
    ));
    check(alice_dir(r("su_create_root", &daily(" su alice alice\n create 0600 root root\n"))));
    check(alice_dir(r("su_create_other_group", &daily(" su alice alice\n create 0640 alice adm\n"))));
    check(alice_dir(r("su_compress", &daily(" su alice alice\n compress\n rotate 2\n"))).file_owned(
        "logs/app.log.1",
        b"older\n",
        0o640,
        1001,
        1001,
    ));
    check(
        alice_dir(r("su_dateext_maxage", &daily(" su alice alice\n dateext\n rotate 5\n maxage 3\n")))
            .file("logs/app.log-20251101", b"old root\n", 0o600)
            .mtime("logs/app.log-20251101", 1_761_955_200),
    );
    check(alice_dir(r("su_shred", &daily(" su alice alice\n shred\n"))).file_owned(
        "logs/app.log.1",
        b"secret\n",
        0o640,
        1001,
        1001,
    ));
    check(alice_dir(r(
        "su_scripts_uids",
        &daily(
            " su alice alice\n sharedscripts\n firstaction\n  id -u > @T@/first.uid\n endscript\n \
             prerotate\n  id -u > @T@/pre.uid\n endscript\n postrotate\n  id -u > @T@/post.uid\n endscript\n \
             lastaction\n  id -u > @T@/last.uid\n endscript\n",
        ),
    )));
    // The log itself unreadable to the su user.
    check(
        r("su_log_root_only_copytruncate", &daily(" su alice alice\n copytruncate\n"))
            .file("logs/app.log", b"root's\n", 0o600)
            .dir_owned("logs", 0o755, 1001, 1001),
    );
    check(
        r("su_log_root_only_compress", &daily(" su alice alice\n compress\n"))
            .file("logs/app.log", b"root's\n", 0o600)
            .dir_owned("logs", 0o755, 1001, 1001),
    );
    check(alice_dir(r("su_group_only", &daily(" su root adm\n create\n"))));
    check(alice_dir(r("su_numeric", &daily(" su 1001 1001\n create\n"))));
    check(alice_dir(r("su_user_only", &daily(" su alice\n create\n"))));
    check(alice_dir(r("su_unknown_user", &daily(" su nosuch nosuch\n"))));
    // Two definitions with different users in one run.
    check(
        alice_dir(r(
            "su_two_users",
            "@T@/logs/app.log {\n daily\n su alice alice\n create\n}\n@T@/bob/b.log {\n daily\n su bob bob\n create\n compress\n}\n",
        ))
        .no_file("state")
        .file("state", state_text(&[LOG, "@T@/bob/b.log"]).as_bytes(), 0o640)
        .file_owned("bob/b.log", b"bob's\n", 0o640, 1002, 1002)
        .dir_owned("bob", 0o755, 1002, 1002),
    );
}
