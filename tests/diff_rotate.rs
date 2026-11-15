//! Differential tests for phase 2: rotating logs.
//!
//! Every case runs `logrotate -v -s @T@/state -m @T@/bin/mail logrotate.conf`
//! (unless it says otherwise) over logs in `T/logs`, whose files all have an
//! mtime an hour before the case's time (2025-11-11 14:00 UTC, a Tuesday)
//! unless set.  The fake mailer appends its arguments and input to
//! `T/mail.log`.

mod common;

use common::{ALICE, Case, DEFAULT_TIME, check};

const CONF: &str = "logrotate.conf";
const NOW: i64 = DEFAULT_TIME.0;
const DAY: i64 = 86400;
const MAILER: &str = "#!/bin/sh\n{ echo \"== mail $*\"; cat; echo \"== end\"; } >> @T@/mail.log\n";

/// A case with `conf`, the logs directory, the fake mailer and `-v`.
fn r(name: &str, conf: &str) -> Case {
    Case::new(name, &["-v", "-s", "@T@/state", "-m", "@T@/bin/mail", CONF]).conf(CONF, conf).dir("logs", 0o755).file(
        "bin/mail",
        MAILER.as_bytes(),
        0o755,
    )
}

/// A state file saying each log was last rotated at the given date
/// ("2025-11-10-6:0:0").
fn state(entries: &[(&str, &str)]) -> String {
    let mut s = String::from("logrotate state -- version 2\n");
    for (f, d) in entries {
        s += &format!("\"{f}\" {d}\n");
    }
    s
}

/// `r` plus a state file.
fn rs(name: &str, conf: &str, entries: &[(&str, &str)]) -> Case {
    r(name, conf).file("state", state(entries).as_bytes(), 0o640)
}

const YESTERDAY: &str = "2025-11-10-6:0:0";
const LOG: &str = "@T@/logs/app.log";

fn log_lines(n: usize) -> Vec<u8> {
    (0..n).map(|i| format!("line {i}\n")).collect::<String>().into_bytes()
}

// ---------------------------------------------------------------------------
// what needs rotating

#[test]
fn size_criterium() {
    let conf = "@T@/logs/app.log {\n size 100\n rotate 3\n}\n";
    check(r("size_over", conf).file("logs/app.log", &log_lines(30), 0o640));
    check(r("size_under", conf).file("logs/app.log", b"short\n", 0o640));
    check(r("size_equal", "@T@/logs/app.log {\n size 6\n}\n").file("logs/app.log", b"short\n", 0o640));
    check(
        r("size_with_old", conf)
            .file("logs/app.log", &log_lines(30), 0o640)
            .file("logs/app.log.1", b"one\n", 0o640)
            .file("logs/app.log.2", b"two\n", 0o640)
            .file("logs/app.log.3", b"three\n", 0o640)
            .file("logs/app.log.4", b"four\n", 0o600),
    );
}

#[test]
fn time_criteria() {
    for (crit, last) in [
        ("daily", YESTERDAY),
        ("daily", "2025-11-11-1:0:0"),
        ("daily", "2025-11-11-14:0:0"),
        ("hourly", "2025-11-11-13:0:0"),
        ("hourly", "2025-11-11-14:0:0"),
        ("weekly", "2025-11-4-14:0:0"),
        ("weekly", "2025-11-9-14:0:0"),
        ("weekly 2", "2025-11-10-14:0:0"),
        ("weekly 3", "2025-11-10-14:0:0"),
        ("weekly 7", "2025-11-5-14:0:0"),
        ("weekly 0", "2025-11-5-14:0:0"),
        ("monthly", "2025-10-31-23:0:0"),
        ("monthly", "2025-11-1-0:0:0"),
        ("yearly", "2024-12-31-0:0:0"),
        ("yearly", "2025-1-1-0:0:0"),
        ("daily", "2025-11-13-0:0:0"),
        ("daily", "2025-11-12-12:0:0"),
    ] {
        let name = format!("crit_{}_{}", crit.replace(' ', "_"), last.replace([':', '-'], "_"));
        check(rs(&name, &format!("@T@/logs/app.log {{\n {crit}\n rotate 2\n}}\n"), &[(LOG, last)]).file(
            "logs/app.log",
            b"data\n",
            0o640,
        ));
    }
}

#[test]
fn no_state_entry() {
    // A new state is "now": nothing rotates until next time.
    check(r("no_state_entry", "@T@/logs/app.log {\n daily\n}\n").file("logs/app.log", b"data\n", 0o640));
}

#[test]
fn ifempty() {
    for (name, d) in [("notifempty_empty", "notifempty"), ("ifempty_empty", "ifempty")] {
        check(rs(name, &format!("@T@/logs/app.log {{\n daily\n {d}\n}}\n"), &[(LOG, YESTERDAY)]).file(
            "logs/app.log",
            b"",
            0o640,
        ));
    }
}

#[test]
fn min_max_size_and_age() {
    let s = &[(LOG, YESTERDAY)];
    check(rs("minsize_small", "@T@/logs/app.log {\n daily\n minsize 1k\n}\n", s).file("logs/app.log", b"x\n", 0o640));
    check(rs("minsize_big", "@T@/logs/app.log {\n daily\n minsize 10\n}\n", s).file(
        "logs/app.log",
        &log_lines(20),
        0o640,
    ));
    check(rs("maxsize_early", "@T@/logs/app.log {\n weekly\n maxsize 10\n}\n", &[(LOG, "2025-11-11-10:0:0")]).file(
        "logs/app.log",
        &log_lines(20),
        0o640,
    ));
    check(
        rs("minage_young", "@T@/logs/app.log {\n daily\n minage 3\n}\n", s)
            .file("logs/app.log", b"x\n", 0o640)
            .mtime("logs/app.log", NOW - 2 * DAY),
    );
    check(
        rs("minage_old", "@T@/logs/app.log {\n daily\n minage 3\n}\n", s)
            .file("logs/app.log", b"x\n", 0o640)
            .mtime("logs/app.log", NOW - 4 * DAY),
    );
    check(
        rs("maxage_numbered", "@T@/logs/app.log {\n daily\n rotate 5\n maxage 2\n}\n", s)
            .file("logs/app.log", b"x\n", 0o640)
            .file("logs/app.log.1", b"1\n", 0o640)
            .file("logs/app.log.2", b"2\n", 0o640)
            .file("logs/app.log.3", b"3\n", 0o640)
            .mtime("logs/app.log.2", NOW - 3 * DAY)
            .mtime("logs/app.log.3", NOW - 10 * DAY),
    );
    check(
        rs("maxage_mail", "@T@/logs/app.log {\n daily\n rotate 5\n maxage 2\n mail a@b\n}\n", s)
            .file("logs/app.log", b"x\n", 0o640)
            .file("logs/app.log.1", b"1\n", 0o640)
            .mtime("logs/app.log.1", NOW - 3 * DAY),
    );
}

#[test]
fn force_and_debug() {
    let conf =
        "@T@/logs/app.log {\n weekly\n rotate 2\n compress\n postrotate\n  echo post >> @T@/out\n endscript\n}\n";
    let files = |c: Case| c.file("logs/app.log", b"data\n", 0o640).file("logs/app.log.1.gz", b"", 0o640);
    check(files(Case::new("force", &["-f", "-v", "-s", "@T@/state", CONF]).conf(CONF, conf).dir("logs", 0o755)));
    check(files(Case::new("dry_run", &["-d", "-f", "-s", "@T@/state", CONF]).conf(CONF, conf).dir("logs", 0o755)));
    check(files(Case::new("quiet", &["-f", "-s", "@T@/state", CONF]).conf(CONF, conf).dir("logs", 0o755)));
}

#[test]
fn missing_and_special_logs() {
    check(r("missing_log", "@T@/logs/app.log {\n rotate 1\n}\n"));
    check(r("missing_log_missingok", "@T@/logs/app.log {\n missingok\n}\n"));
    check(r("missing_dir_missingok", "@T@/logs/none/app.log {\n missingok\n}\n"));
    check(r("missing_dir", "@T@/logs/none/app.log {\n}\n"));
    check(
        r("symlink_log", "@T@/logs/app.log {\n size 1\n}\n")
            .file("logs/real", b"x\n", 0o640)
            .symlink("logs/app.log", "real"),
    );
    check(
        r("hardlink_log", "@T@/logs/app.log {\n size 1\n}\n")
            .file("logs/app.log", b"data\n", 0o640)
            .hardlink("logs/other", "logs/app.log"),
    );
    check(
        r("hardlink_allowed", "@T@/logs/app.log {\n size 1\n allowhardlink\n}\n")
            .file("logs/app.log", b"data\n", 0o640)
            .hardlink("logs/other", "logs/app.log"),
    );
    check(r("fifo_log", "@T@/logs/app.log {\n size 1\n}\n").fifo("logs/app.log", 0o640));
}

#[test]
fn insecure_parent() {
    let conf = "@T@/logs/app.log {\n size 1\n}\n";
    check(r("parent_group_writable", conf).file("logs/app.log", b"x\n", 0o640).dir_owned("logs", 0o775, 0, 4));
    check(r("parent_root_group_writable", conf).file("logs/app.log", b"x\n", 0o640).dir_owned("logs", 0o775, 0, 0));
    check(r("parent_world_writable", conf).file("logs/app.log", b"x\n", 0o640).dir("logs", 0o1777));
    check(
        r("parent_insecure_su", "@T@/logs/app.log {\n size 1\n su root adm\n}\n")
            .file("logs/app.log", b"x\n", 0o640)
            .dir_owned("logs", 0o775, 0, 4),
    );
}

// ---------------------------------------------------------------------------
// names

#[test]
fn numbered_names() {
    let s = &[(LOG, YESTERDAY)];
    let old = |c: Case| {
        c.file("logs/app.log", b"new\n", 0o640)
            .file("logs/app.log.1", b"1\n", 0o640)
            .file("logs/app.log.2", b"2\n", 0o640)
            .file("logs/app.log.5", b"5\n", 0o640)
            .file("logs/app.log.x", b"x\n", 0o640)
    };
    check(old(rs("rotate_unlimited", "@T@/logs/app.log {\n daily\n rotate -1\n}\n", s)));
    check(old(rs("rotate_zero", "@T@/logs/app.log {\n daily\n rotate 0\n}\n", s)));
    check(old(rs("rotate_one", "@T@/logs/app.log {\n daily\n rotate 1\n}\n", s)));
    check(old(rs("start_zero", "@T@/logs/app.log {\n daily\n rotate 3\n start 0\n}\n", s)));
    check(old(rs("start_five", "@T@/logs/app.log {\n daily\n rotate 2\n start 5\n}\n", s)));
    check(
        old(rs(
            "rotate_unlimited_none",
            "@T@/logs/other.log {\n daily\n rotate -1\n}\n",
            &[("@T@/logs/other.log", YESTERDAY)],
        ))
        .file("logs/other.log", b"o\n", 0o640),
    );
}

#[test]
fn extensions() {
    let s = &[(LOG, YESTERDAY)];
    let f = |c: Case| c.file("logs/app.log", b"new\n", 0o640).file("logs/app.1.log", b"1\n", 0o640);
    check(f(rs("extension", "@T@/logs/app.log {\n daily\n rotate 2\n extension .log\n}\n", s)));
    check(f(rs("extension_nomatch", "@T@/logs/app.log {\n daily\n rotate 2\n extension .txt\n}\n", s)));
    check(f(rs("addextension", "@T@/logs/app.log {\n daily\n rotate 2\n addextension .log\n}\n", s)));
    check(f(rs("addextension_new", "@T@/logs/app.log {\n daily\n rotate 2\n addextension .old\n}\n", s)));
    // QUIRK: a global addextension never reaches the definition.
    check(f(rs("addextension_global", "addextension .old\n@T@/logs/app.log {\n daily\n rotate 2\n}\n", s)));
    check(f(rs("extension_compress", "@T@/logs/app.log {\n daily\n rotate 2\n extension .log\n compress\n}\n", s)));
}

#[test]
fn dateext() {
    let s = &[(LOG, YESTERDAY)];
    let f = |c: Case| c.file("logs/app.log", b"new\n", 0o640);
    for (name, extra) in [
        ("dateext_default", ""),
        ("dateext_yesterday", " dateyesterday\n"),
        ("dateext_hourago", " datehourago\n"),
        ("dateext_format", " dateformat -%Y-%m-%d\n"),
        ("dateext_format_seconds", " dateformat .%s\n"),
        ("dateext_format_hour", " dateformat -%Y%m%d%H%M%S\n"),
        ("dateext_format_unsupported", " dateformat -%a-%d\n"),
        ("dateext_format_spaces", " dateformat    -%d\n"),
        ("dateext_format_percent_end", " dateformat -%d%\n"),
        ("dateext_format_z", " dateformat -%Y%z\n"),
        ("dateext_format_v", " dateformat -%V\n"),
        ("dateext_format_too_long", " dateformat -%Y%Y%Y%Y%Y%Y%Y\n"),
        ("dateext_format_no_fit", " dateformat -%Y%m%d%Y%m%d%Y%m%d%Y%m%d%Y%m%d%Y%m%d%Y%m%d%Y\n"),
        ("dateext_format_empty_result", " dateformat %p\n"),
        ("dateext_compress", " compress\n"),
        ("dateext_delaycompress", " compress\n delaycompress\n"),
    ] {
        check(f(rs(name, &format!("@T@/logs/app.log {{\n daily\n rotate 2\n dateext\n{extra}}}\n"), s)));
    }
    check(f(rs(
        "dateext_hourly",
        "@T@/logs/app.log {\n hourly\n rotate 2\n dateext\n}\n",
        &[(LOG, "2025-11-11-13:0:0")],
    )));
    check(f(rs("dateext_exists", "@T@/logs/app.log {\n daily\n dateext\n}\n", s)).file(
        "logs/app.log-20251111",
        b"already\n",
        0o640,
    ));
    check(
        f(rs("dateext_old_removed", "@T@/logs/app.log {\n daily\n rotate 2\n dateext\n mail a@b\n}\n", s))
            .file("logs/app.log-20251101", b"a\n", 0o640)
            .file("logs/app.log-20251105", b"b\n", 0o640)
            .file("logs/app.log-20251001", b"c\n", 0o640)
            .file("logs/app.log-2025110", b"short\n", 0o640),
    );
    check(
        f(rs(
            "dateext_format_sorted",
            "@T@/logs/app.log {\n daily\n rotate 2\n dateext\n dateformat -%d-%m-%Y\n}\n",
            s,
        ))
        .file("logs/app.log-01-12-2024", b"a\n", 0o640)
        .file("logs/app.log-05-11-2025", b"b\n", 0o640)
        .file("logs/app.log-31-01-2025", b"c\n", 0o640),
    );
    check(
        f(rs("dateext_maxage", "@T@/logs/app.log {\n daily\n rotate 9\n dateext\n maxage 5\n}\n", s))
            .file("logs/app.log-20251101", b"a\n", 0o640)
            .file("logs/app.log-20251109", b"b\n", 0o640)
            .mtime("logs/app.log-20251101", NOW - 10 * DAY),
    );
    check(
        f(rs(
            "dateext_delaycompress_old",
            "@T@/logs/app.log {\n daily\n rotate 3\n dateext\n compress\n delaycompress\n}\n",
            s,
        ))
        .file("logs/app.log-20251109", b"a\n", 0o640)
        .file("logs/app.log-20251110", b"b\n", 0o640)
        .file("logs/app.log-20251108.gz", b"", 0o640),
    );
    check(f(rs("dateext_unlimited", "@T@/logs/app.log {\n daily\n rotate -1\n dateext\n}\n", s)).file(
        "logs/app.log-20251101",
        b"a\n",
        0o640,
    ));
}

#[test]
fn dateext_timezones() {
    for tz in ["Europe/Berlin", "America/New_York", "Pacific/Kiritimati"] {
        let name = format!("dateext_tz_{}", tz.replace('/', "_"));
        check(
            rs(&name, "@T@/logs/app.log {\n daily\n dateext\n dateyesterday\n}\n", &[(LOG, YESTERDAY)])
                .file("logs/app.log", b"new\n", 0o640)
                .env("TZ", tz),
        );
    }
    // Across the end of DST in New York (2025-11-02 06:00 UTC).
    check(
        rs("dst_end_daily", "@T@/logs/app.log {\n daily\n dateext\n}\n", &[(LOG, "2025-11-1-1:30:0")])
            .file("logs/app.log", b"new\n", 0o640)
            .env("TZ", "America/New_York")
            .time(1_762_065_000),
    );
}

#[test]
fn olddir() {
    let s = &[(LOG, YESTERDAY)];
    let f = |c: Case| c.file("logs/app.log", b"new\n", 0o640);
    check(
        f(rs("olddir_relative", "@T@/logs/app.log {\n daily\n rotate 2\n olddir old\n}\n", s)).dir("logs/old", 0o755),
    );
    check(
        f(rs("olddir_absolute", "@T@/logs/app.log {\n daily\n rotate 2\n olddir @T@/archive\n}\n", s))
            .dir("archive", 0o700),
    );
    check(f(rs(
        "createolddir",
        "@T@/logs/app.log {\n daily\n rotate 2\n olddir old/deep\n createolddir 0750 alice users\n}\n",
        s,
    )));
    check(f(rs("createolddir_default", "@T@/logs/app.log {\n daily\n olddir @T@/archive/x\n createolddir\n}\n", s)));
    check(
        f(rs("olddir_compress", "@T@/logs/app.log {\n daily\n rotate 2\n olddir old\n compress\n dateext\n}\n", s))
            .dir("logs/old", 0o755),
    );
}

// ---------------------------------------------------------------------------
// create, copy, copytruncate

#[test]
fn create() {
    let s = &[(LOG, YESTERDAY)];
    for (name, d) in [
        ("create_default", "create"),
        ("create_mode", "create 0600"),
        ("create_full", "create 0644 alice users"),
        ("create_group_only", "create alice adm"),
        ("nocreate", "nocreate"),
    ] {
        check(rs(name, &format!("@T@/logs/app.log {{\n daily\n {d}\n}}\n"), s).file_owned(
            "logs/app.log",
            b"old\n",
            0o604,
            1002,
            4,
        ));
    }
    check(
        rs(
            "create_exists",
            "@T@/logs/app.log {\n daily\n create\n postrotate\n  echo again > @T@/logs/app.log\n endscript\n}\n",
            s,
        )
        .file("logs/app.log", b"old\n", 0o640),
    );
}

#[test]
fn copy_modes() {
    let s = &[(LOG, YESTERDAY)];
    for (name, d) in [
        ("copytruncate", "copytruncate"),
        ("copy", "copy"),
        ("renamecopy", "renamecopy"),
        ("copytruncate_create", "copytruncate\n create 0600"),
        ("copytruncate_rotate0", "copytruncate\n rotate 0"),
        ("copytruncate_rotate0_mail", "copytruncate\n rotate 0\n mail x@y"),
        ("copytruncate_compress", "copytruncate\n compress"),
        ("copytruncate_compress_rotate0", "copytruncate\n compress\n rotate 0"),
        ("renamecopy_compress", "renamecopy\n compress"),
    ] {
        check(rs(name, &format!("@T@/logs/app.log {{\n daily\n rotate 2\n {d}\n}}\n"), s).file_owned(
            "logs/app.log",
            &log_lines(5),
            0o604,
            1002,
            4,
        ));
    }
    let mut sparse = vec![0u8; 20000];
    sparse.extend_from_slice(b"end\n");
    check(rs("copy_zeros", "@T@/logs/app.log {\n daily\n copytruncate\n}\n", s).file("logs/app.log", &sparse, 0o640));
}

// ---------------------------------------------------------------------------
// compression

#[test]
fn compression() {
    let s = &[(LOG, YESTERDAY)];
    let f = |c: Case| {
        c.file("logs/app.log", &log_lines(10), 0o640).file("logs/app.log.1", b"one\n", 0o640).file(
            "logs/app.log.1.gz",
            b"",
            0o640,
        )
    };
    for (name, extra) in [
        ("compress", " compress\n"),
        ("delaycompress", " compress\n delaycompress\n"),
        ("delaycompress_without_compress", " delaycompress\n"),
        ("compress_xz", " compress\n compresscmd /usr/bin/xz\n"),
        ("compress_options", " compress\n compressoptions -1 --no-name\n"),
        ("compress_ext", " compress\n compressext .gzip\n"),
        ("compress_fails", " compress\n compresscmd /bin/false\n"),
        ("compress_missing", " compress\n compresscmd /nonexistent/zip\n"),
        ("compress_stderr", " compress\n compresscmd @T@/bin/noisy\n"),
        ("compress_env", " compress\n compresscmd @T@/bin/envzip\n compressext .env\n"),
    ] {
        check(
            f(rs(name, &format!("@T@/logs/app.log {{\n daily\n rotate 3\n{extra}}}\n"), s))
                .file(
                    "bin/noisy",
                    b"#!/bin/sh\necho 'warning: from the compressor' >&2\nprintf 'a\\0b' >&2\ncat\n",
                    0o755,
                )
                .file("bin/envzip", b"#!/bin/sh\necho \"$LOGROTATE_COMPRESSED_FILENAME\"\ncat\n", 0o755),
        );
    }
    check(
        rs("compress_dest_exists", "@T@/logs/app.log {\n daily\n compress\n}\n", s)
            .file("logs/app.log", b"x\n", 0o640)
            .file("logs/app.log.1.gz", b"stale\n", 0o640),
    );
}

// ---------------------------------------------------------------------------
// scripts

#[test]
fn scripts() {
    let rec = |what: &str| format!("  echo \"{what} $# [$1] [$2]\" >> @T@/out\n");
    let all = format!(
        " firstaction\n{}  endscript\n prerotate\n{}  endscript\n postrotate\n{}  endscript\n lastaction\n{}  endscript\n preremove\n{}  endscript\n",
        rec("first"),
        rec("pre"),
        rec("post"),
        rec("last"),
        rec("preremove")
    );
    let files = |c: Case| {
        c.file("logs/a.log", b"a\n", 0o640).file("logs/b.log", b"b\n", 0o640).file("logs/a.log.1", b"a1\n", 0o640).file(
            "logs/b.log.1",
            b"b1\n",
            0o640,
        )
    };
    let s = &[("@T@/logs/a.log", YESTERDAY), ("@T@/logs/b.log", YESTERDAY)];
    check(files(rs("scripts_unshared", &format!("@T@/logs/*.log {{\n daily\n rotate 1\n{all}}}\n"), s)));
    check(files(rs("scripts_shared", &format!("@T@/logs/*.log {{\n daily\n rotate 1\n sharedscripts\n{all}}}\n"), s)));
    check(files(rs(
        "scripts_none_rotated",
        &format!("@T@/logs/*.log {{\n daily\n rotate 1\n{all}}}\n"),
        &[("@T@/logs/a.log", "2025-11-11-14:0:0"), ("@T@/logs/b.log", "2025-11-11-14:0:0")],
    )));
    check(files(rs(
        "scripts_one_rotated",
        &format!("@T@/logs/*.log {{\n daily\n rotate 1\n{all}}}\n"),
        &[("@T@/logs/a.log", YESTERDAY), ("@T@/logs/b.log", "2025-11-11-14:0:0")],
    )));
    for (name, which, shared) in [
        ("fail_first", "firstaction", false),
        ("fail_pre", "prerotate", false),
        ("fail_pre_shared", "prerotate", true),
        ("fail_post", "postrotate", false),
        ("fail_post_shared", "postrotate", true),
        ("fail_last", "lastaction", false),
        ("fail_preremove", "preremove", false),
    ] {
        let conf = format!(
            "@T@/logs/*.log {{\n daily\n rotate 1\n{}{all} {which}\n  echo failing {which} >> @T@/out\n  exit 3\n endscript\n}}\n",
            if shared { " sharedscripts\n" } else { "" }
        );
        check(files(rs(name, &conf, s)));
    }
    check(files(rs(
        "script_stdout",
        "@T@/logs/a.log {\n daily\n postrotate\n  echo to stdout; echo to stderr >&2\n endscript\n}\n",
        s,
    )));
    check(files(rs("script_killed", "@T@/logs/a.log {\n daily\n postrotate\n  kill -9 $$\n endscript\n}\n", s)));
    check(files(
        Case::new("scripts_dry_run", &["-d", "-s", "@T@/state", CONF])
            .conf(CONF, &format!("@T@/logs/*.log {{\n daily\n rotate 1\n{all}}}\n"))
            .dir("logs", 0o755)
            .file("state", state(s).as_bytes(), 0o640),
    ));
}

// ---------------------------------------------------------------------------
// mail

#[test]
fn mail() {
    let s = &[(LOG, YESTERDAY)];
    let f = |c: Case| {
        c.file("logs/app.log", b"current\n", 0o640).file("logs/app.log.1", b"one\n", 0o640).file(
            "logs/app.log.2",
            b"two\n",
            0o640,
        )
    };
    for (name, extra) in [
        ("mail_last", ""),
        ("mail_first", " mailfirst\n"),
        ("mail_compress", " compress\n"),
        ("mail_compress_first", " compress\n mailfirst\n"),
        ("mail_delaycompress_first", " compress\n delaycompress\n mailfirst\n"),
        ("mail_rotate0", " rotate 0\n"),
        ("mail_dateext", " dateext\n"),
    ] {
        check(f(rs(name, &format!("@T@/logs/app.log {{\n daily\n rotate 2\n mail root@example.com\n{extra}}}\n"), s)));
    }
    check(f(rs("mail_command_fails", "@T@/logs/app.log {\n daily\n rotate 1\n mail x@y\n}\n", s)).file(
        "bin/mail",
        b"#!/bin/sh\ncat >/dev/null\nexit 1\n",
        0o755,
    ));
    check(
        f(rs("mail_command_missing", "@T@/logs/app.log {\n daily\n rotate 1\n mail x@y\n}\n", s)).no_file("bin/mail"),
    );
    check(f(rs("mail_compress_gz", "@T@/logs/app.log {\n daily\n rotate 1\n mail x@y\n compress\n}\n", s)).file(
        "logs/app.log.1.gz",
        b"not really gzip\n",
        0o640,
    ));
}

// ---------------------------------------------------------------------------
// users

#[test]
fn su() {
    let s = &[(LOG, YESTERDAY)];
    let f =
        |c: Case| c.file_owned("logs/app.log", b"alice's\n", 0o640, 1001, 1001).dir_owned("logs", 0o750, 1001, 1001);
    check(f(rs("su_alice", "@T@/logs/app.log {\n daily\n rotate 2\n su alice alice\n create\n compress\n}\n", s)));
    check(f(rs(
        "su_alice_scripts",
        "@T@/logs/app.log {\n daily\n su alice alice\n postrotate\n  id -u > @T@/logs/post.uid\n endscript\n}\n",
        s,
    )));
    check(
        f(rs("su_alice_mail", "@T@/logs/app.log {\n daily\n rotate 1\n su alice alice\n mail a@b\n}\n", s)).file_owned(
            "logs/app.log.1",
            b"old\n",
            0o640,
            1001,
            1001,
        ),
    );
    check(f(rs("su_bob_denied", "@T@/logs/app.log {\n daily\n su bob bob\n}\n", s)));
    check(
        rs("as_user", "@T@/logs/app.log {\n daily\n create\n}\n", s)
            .file_owned("logs/app.log", b"x\n", 0o644, 1001, 1001)
            .dir_owned("logs", 0o755, 1001, 1001)
            .file_owned("state", state(s).as_bytes(), 0o640, 1001, 1001)
            .file_owned(CONF, b"@T@/logs/app.log {\n daily\n create 0600 root root\n}\n", 0o644, 1001, 1001)
            .run_as(ALICE),
    );
}

// ---------------------------------------------------------------------------
// shred

#[test]
fn shred() {
    let s = &[(LOG, YESTERDAY)];
    let f = |c: Case| c.file("logs/app.log", b"new\n", 0o640).file("logs/app.log.1", b"secret\n", 0o640);
    check(f(rs("shred", "@T@/logs/app.log {\n daily\n rotate 1\n shred\n}\n", s)));
    check(f(rs("shred_cycles", "@T@/logs/app.log {\n daily\n rotate 1\n shred\n shredcycles 2\n}\n", s)));
    check(f(rs("shred_compress", "@T@/logs/app.log {\n daily\n rotate 1\n shred\n compress\n}\n", s)));
    check(
        f(rs("shred_hardlink", "@T@/logs/app.log {\n daily\n rotate 1\n shred\n}\n", s))
            .hardlink("logs/keep", "logs/app.log.1"),
    );
}

// ---------------------------------------------------------------------------
// sets, globs and the state file

#[test]
fn globs_and_duplicates() {
    let s = &[("@T@/logs/a.log", YESTERDAY), ("@T@/logs/b.log", YESTERDAY)];
    let f = |c: Case| c.file("logs/a.log", b"a\n", 0o640).file("logs/b.log", b"b\n", 0o640).dir("logs/c.log", 0o755);
    check(f(rs("glob_set", "@T@/logs/*.log {\n daily\n}\n", s)));
    check(f(rs("glob_duplicate", "@T@/logs/*.log {\n daily\n}\n@T@/logs/a.log {\n weekly\n}\n", s)));
    check(f(rs(
        "glob_ignoreduplicates",
        "@T@/logs/*.log {\n daily\n ignoreduplicates\n}\n@T@/logs/a.log {\n weekly\n}\n",
        s,
    )));
    check(f(rs("glob_duplicate_in_set", "@T@/logs/a.log @T@/logs/*.log {\n daily\n}\n", s)));
    check(f(rs("glob_brace_set", "@T@/logs/{a,b}.log {\n daily\n}\n", s)));
    check(f(rs("glob_nomatch_literal", "@T@/logs/*.txt {\n daily\n}\n", s)));
}

#[test]
fn state_updates() {
    let s = &[(LOG, YESTERDAY), ("@T@/logs/gone.log", "2024-1-1-0:0:0"), ("@T@/logs/recent.log", "2025-6-1-0:0:0")];
    check(rs("state_updates", "@T@/logs/app.log @T@/logs/missing.log {\n daily\n missingok\n}\n", s).file(
        "logs/app.log",
        b"x\n",
        0o640,
    ));
    check(rs("state_future", "@T@/logs/app.log {\n daily\n}\n", &[(LOG, "2025-11-20-0:0:0")]).file(
        "logs/app.log",
        b"x\n",
        0o640,
    ));
}

#[test]
fn log_file_option() {
    check(
        Case::new("log_file_rotation", &["-l", "@T@/run.log", "-s", "@T@/state", CONF])
            .conf(CONF, "@T@/logs/app.log {\n size 1\n compress\n compresscmd @T@/bin/noisy\n postrotate\n  echo hi\n endscript\n}\n")
            .dir("logs", 0o755)
            .file("logs/app.log", b"x\n", 0o640)
            .file("bin/noisy", b"#!/bin/sh\necho noise >&2\ncat\n", 0o755),
    );
}

#[test]
fn several_sets() {
    let s = &[("@T@/logs/a.log", YESTERDAY), ("@T@/logs/b.log", YESTERDAY)];
    check(
        rs(
            "several_sets",
            "compress\n@T@/logs/a.log {\n daily\n rotate 2\n}\n@T@/logs/b.log {\n weekly\n nocompress\n}\n\
             @T@/logs/c.log {\n size 1\n copytruncate\n}\n",
            s,
        )
        .file("logs/a.log", b"a\n", 0o640)
        .file("logs/b.log", b"b\n", 0o640)
        .file("logs/c.log", b"c\n", 0o640),
    );
}
