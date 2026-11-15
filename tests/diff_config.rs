//! Differential tests for phase 1: the command line, the configuration
//! parser, the `-l` log, and the state file.
//!
//! Log definitions name directories (which logrotate skips), so no file is
//! rotated: what's compared is the parse, its messages and the per-set
//! summary `-v` prints, the exit status, and the state file.

mod common;

use common::{ALICE, Case, check};

const CONF: &str = "logrotate.conf";

/// `logrotate -v -s @T@/state logrotate.conf` with `conf`, and directories
/// `logs/d1` and `logs/d2` to name in definitions.
fn v(name: &str, conf: &str) -> Case {
    Case::new(name, &["-v", "-s", "@T@/state", CONF]).conf(CONF, conf).dir("logs/d1", 0o755).dir("logs/d2", 0o755)
}

/// The same, with a state file.
fn vs(name: &str, conf: &str, state: &str) -> Case {
    v(name, conf).file("state", state.as_bytes(), 0o640)
}

const STATE: &str = "logrotate state -- version 2\n\"/var/log/a\" 2025-11-10-6:0:0\n";

// ---------------------------------------------------------------------------
// command line

#[test]
fn version() {
    check(Case::new("version", &["--version"]));
}

#[test]
fn help() {
    check(Case::new("help", &["--help"]));
}

#[test]
fn usage() {
    check(Case::new("usage", &["--usage"]));
}

#[test]
fn no_arguments() {
    check(Case::new("no_arguments", &[]));
}

#[test]
fn bad_option() {
    check(Case::new("bad_option", &["--frobnicate", "x"]));
    check(Case::new("bad_short_option", &["-z", "x"]));
}

#[test]
fn missing_option_argument() {
    check(Case::new("missing_option_argument", &["x", "-s"]));
}

#[test]
fn lock_options_exclusive() {
    check(Case::new("lock_options_exclusive", &["--skip-state-lock", "--wait-for-state-lock", "x"]));
}

#[test]
fn debug_mode() {
    check(
        Case::new("debug_mode", &["-d", "-s", "@T@/state", CONF])
            .conf(CONF, "rotate 2\n@T@/logs/d1 {\n weekly\n}\n")
            .dir("logs/d1", 0o755),
    );
    check(
        Case::new("debug_mode_state", &["-d", "-s", "@T@/state", CONF])
            .conf(CONF, "@T@/logs/d1 {\n}\n")
            .dir("logs/d1", 0o755)
            .file("state", STATE.as_bytes(), 0o640),
    );
    check(
        Case::new("debug_mode_state_unreadable", &["-d", "-s", "@T@/state", CONF])
            .conf(CONF, "@T@/logs/d1 {\n}\n")
            .dir("logs/d1", 0o755)
            .dir("state", 0o755),
    );
}

#[test]
fn quiet_run() {
    check(
        Case::new("quiet_run", &["-s", "@T@/state", CONF])
            .conf(CONF, "rotate 2\nbogus\n@T@/logs/d1 {\n weekly\n}\n")
            .dir("logs/d1", 0o755),
    );
}

#[test]
fn force_flag() {
    check(
        Case::new("force_flag", &["-f", "-v", "-s", "@T@/state", CONF])
            .conf(CONF, "@T@/logs/d1 {\n weekly\n}\n")
            .dir("logs/d1", 0o755),
    );
}

#[test]
fn log_file() {
    check(
        Case::new("log_file", &["-l", "@T@/out.log", "-s", "@T@/state", CONF])
            .conf(CONF, "bogus\n@T@/logs/d1 {\n}\n")
            .dir("logs/d1", 0o755),
    );
    check(
        Case::new("log_file_verbose", &["-v", "-l", "@T@/out.log", "-s", "@T@/state", CONF])
            .conf(CONF, "@T@/logs/d1 {\n}\n")
            .dir("logs/d1", 0o755)
            .file("out.log", b"old contents\n", 0o600),
    );
    check(
        Case::new("log_file_unopenable", &["-l", "@T@/nodir/out.log", "-s", "@T@/state", CONF])
            .conf(CONF, "@T@/logs/d1 {\n}\n")
            .dir("logs/d1", 0o755),
    );
}

#[test]
fn log_to_syslog() {
    check(
        Case::new("log_to_syslog", &["-l", "syslog", "-s", "@T@/state", CONF])
            .conf(CONF, "bogus\n@T@/logs/d1 {\n}\n")
            .dir("logs/d1", 0o755),
    );
    check(
        Case::new("log_to_syslog_verbose", &["-v", "-l", "syslog", "-s", "@T@/state", CONF])
            .conf(CONF, "@T@/logs/d1 {\n}\n")
            .dir("logs/d1", 0o755),
    );
}

#[test]
fn several_config_files() {
    check(
        Case::new("several_config_files", &["-v", "-s", "@T@/state", "a.conf", "b.conf", "missing.conf"])
            .conf("a.conf", "rotate 3\n@T@/logs/d1 {\n}\n")
            .conf("b.conf", "@T@/logs/d2 {\n}\n")
            .dir("logs/d1", 0o755)
            .dir("logs/d2", 0o755),
    );
}

#[test]
fn popt_alias() {
    check(
        Case::new("popt_alias", &["--quick", CONF])
            .conf(CONF, "@T@/logs/d1 {\n}\n")
            .dir("logs/d1", 0o755)
            .env("HOME", "@T@/home/root")
            .file("home/root/.popt", b"logrotate alias --quick -v -s @T@/state\n", 0o644),
    );
}

// ---------------------------------------------------------------------------
// the state file

#[test]
fn state_default_path() {
    check(Case::new("state_default_path", &["-v", CONF]).conf(CONF, "@T@/logs/d1 {\n}\n").dir("logs/d1", 0o755));
}

#[test]
fn state_round_trip() {
    let state = "logrotate state -- version 2\n\
                 \"/var/log/a\" 2025-11-10-6:0:0\n\
                 \"/var/log/old\" 2023-1-1-0:0:0\n\
                 \"/var/log/b\\\\c\" 2025-6-1-0:0:0\n\
                 \"/var/log/q\\\"uote\" 2025-6-1-0:0:0\n\
                 \"/var/log/nl\\nx\" 2025-6-1-0:0:0\n\
                 \"/var/log/other\\x\" 2025-6-1-0:0:0\n\
                 /var/log/unquoted 2025-6-1\n\
                 '/var/log/single quoted' 2025-6-1-23:59:59\n\
                 \n\
                 \"/var/log/day0\" 2025-6-0-0:0:0\n\
                 \"/var/log/y1900\" 1900-1-1-0:0:0\n\
                 \"/var/log/a\" 2025-11-11-1:2:3\n";
    check(vs("state_round_trip", "@T@/logs/d1 {\n}\n", state));
}

#[test]
fn state_version_1() {
    check(vs("state_version_1", "", "logrotate state -- version 1\n\"/var/log/a\" 2025-11-1\n"));
}

#[test]
fn state_empty() {
    check(vs("state_empty", "", ""));
}

#[test]
fn state_bad_top_line() {
    check(vs("state_bad_top_line", "", "logrotate state -- version 3\n"));
    check(vs("state_no_newline", "", "logrotate state -- version 2"));
}

#[test]
fn state_bad_lines() {
    let top = "logrotate state -- version 2\n";
    for (name, line) in [
        ("state_bad_fields", "\"/a\"\n"),
        ("state_three_fields", "\"/a\" 2025-1-1 x\n"),
        ("state_bad_date", "\"/a\" 2025-1\n"),
        ("state_bad_quote", "\"/a 2025-1-1\n"),
        ("state_bad_year", "\"/a\" 1969-1-1\n"),
        ("state_bad_year_high", "\"/a\" 2101-1-1\n"),
        ("state_bad_month", "\"/a\" 2025-13-1\n"),
        ("state_bad_day", "\"/a\" 2025-1-32\n"),
        ("state_bad_hour", "\"/a\" 2025-1-1-24:0:0\n"),
        ("state_bad_minute", "\"/a\" 2025-1-1-0:60:0\n"),
        ("state_bad_second", "\"/a\" 2025-1-1-0:0:60\n"),
        ("state_no_final_newline", "\"/a\" 2025-1-1"),
        ("state_nul", "\0\"/a\" 2025-1-1\n"),
        ("state_nul_inside", "\"/a\" 2025\0-1-1\n"),
    ] {
        check(vs(name, "", &format!("{top}\"/ok\" 2025-1-1\n{line}\"/after\" 2025-1-1\n")));
    }
    let long = format!("{top}\"/{}\" 2025-1-1\n", "x".repeat(9000));
    check(vs("state_line_too_long", "", &long));
    let fits = format!("{top}\"/{}\" 2025-1-1\n", "x".repeat(8100));
    check(vs("state_long_line", "", &fits));
}

#[test]
fn state_timezones() {
    let state = "logrotate state -- version 2\n\"/a\" 2025-3-30-2:30:0\n\"/b\" 2025-10-26-2:30:0\n\
                 \"/c\" 2024-11-12-3:0:0\n\"/d\" 2024-11-11-15:0:0\n";
    for (name, tz) in [("state_tz_berlin", "Europe/Berlin"), ("state_tz_ny", "America/New_York")] {
        check(vs(name, "", state).env("TZ", tz));
    }
}

#[test]
fn state_not_regular() {
    check(v("state_is_dir", "").dir("state", 0o755));
    check(v("state_is_fifo", "").fifo("state", 0o640));
}

#[test]
fn state_world_readable() {
    check(vs("state_world_readable", "", STATE).file("state", STATE.as_bytes(), 0o644));
}

#[test]
fn state_modes_and_owner() {
    check(vs("state_mode_600", "", STATE).file("state", STATE.as_bytes(), 0o600));
    check(vs("state_owned_by_alice", "", STATE).file_owned("state", STATE.as_bytes(), 0o660, 1001, 4));
}

#[test]
fn state_leftover_tmp() {
    check(vs("state_leftover_tmp", "", STATE).file("state.tmp", b"junk\n", 0o600));
}

#[test]
fn state_dev_null() {
    check(
        Case::new("state_dev_null", &["-v", "-s", "/dev/null", CONF])
            .conf(CONF, "@T@/logs/d1 {\n}\n")
            .dir("logs/d1", 0o755),
    );
}

#[test]
fn state_skip_and_wait_lock() {
    check(Case::new("state_skip_lock", &["-v", "--skip-state-lock", "-s", "@T@/state", CONF]).conf(CONF, ""));
    check(Case::new("state_wait_lock", &["-v", "--wait-for-state-lock", "-s", "@T@/state", CONF]).conf(CONF, ""));
}

#[test]
fn state_unwritable_dir() {
    check(
        Case::new("state_unwritable_dir", &["-v", "-s", "@T@/ro/state", CONF])
            .conf(CONF, "")
            .run_as(ALICE)
            .dir("ro", 0o755),
    );
    check(
        Case::new("state_tmp_unwritable", &["-v", "-s", "@T@/ro/state", CONF])
            .conf(CONF, "")
            .run_as(ALICE)
            .file_owned("ro/state", STATE.as_bytes(), 0o640, 1001, 1001)
            .dir("ro", 0o755),
    );
}

#[test]
fn state_backup_of_existing_tmp() {
    // A state.tmp that unlink() can't remove (a directory).
    check(vs("state_tmp_is_dir", "", STATE).dir("state.tmp", 0o755));
}

// ---------------------------------------------------------------------------
// config files

#[test]
fn config_missing() {
    check(Case::new("config_missing", &["-v", "-s", "@T@/state", "nope.conf"]));
}

#[test]
fn config_empty() {
    check(v("config_empty", ""));
}

#[test]
fn config_not_regular() {
    // (A FIFO would block C logrotate in open().)
    check(
        Case::new("config_device_in_dir", &["-v", "-s", "@T@/state", "d"])
            .symlink("d/null", "/dev/null")
            .conf("d/a", ""),
    );
    check(Case::new("config_device", &["-v", "-s", "@T@/state", "/dev/null"]));
}

#[test]
fn config_permissions() {
    check(v("config_dangerous_mode", "rotate 1\n").file(CONF, b"rotate 1\n", 0o755));
    check(v("config_group_writable", "rotate 1\n").file(CONF, b"rotate 1\n", 0o664));
    check(v("config_wrong_owner", "rotate 1\n").file_owned(CONF, b"rotate 1\n", 0o644, 1001, 0));
    check(v("config_mode_400", "").file(CONF, b"@T@/logs/d1 {\n}\n", 0o400));
    check(
        v("config_as_user", "@T@/logs/d1 {\n}\n")
            .file_owned(CONF, b"@T@/logs/d1 {\n}\n", 0o666, 1001, 1001)
            .run_as(ALICE),
    );
}

#[test]
fn config_unreadable() {
    check(v("config_unreadable", "").file(CONF, b"rotate 1\n", 0o000).run_as(ALICE));
}

// ---------------------------------------------------------------------------
// directives

#[test]
fn all_flags() {
    let conf = "compress\nnocompress\ndelaycompress\nnodelaycompress\nshred\nnoshred\nallowhardlink\n\
                noallowhardlink\nsharedscripts\nnosharedscripts\ncopytruncate\nnocopytruncate\nrenamecopy\n\
                norenamecopy\ncopy\nnocopy\nifempty\nnotifempty\ndateext\nnodateext\ndateyesterday\n\
                datehourago\nmailfirst\nmaillast\nmissingok\nnomissingok\nignoreduplicates\nnocreate\n\
                nocreateolddir\nnoolddir\nnomail\n@T@/logs/d1 {\n}\n";
    check(v("all_flags", conf));
}

#[test]
fn summary_variants() {
    let conf = "@T@/logs/d1 {\n hourly\n rotate -1\n}\n\
                @T@/logs/d2 {\n daily\n rotate -1\n maxage 3\n mail a@b\n minage 2\n}\n\
                @T@/logs/d1/x {\n monthly\n rotate 0\n mail a@b\n notifempty\n olddir /tmp\n minsize 5\n maxsize 7\n}\n\
                @T@/logs/d2/x {\n yearly\n rotate 5\n}\n";
    check(v("summary_variants", conf).dir("logs/d1/x", 0o755).dir("logs/d2/x", 0o755));
}

#[test]
fn criterium_overrides() {
    let conf = "weekly\nmonthly\n@T@/logs/d1 {\n daily\n hourly\n size 5\n size 6\n}\n\
                @T@/logs/d2 {\n daily\n}\n";
    check(v("criterium_overrides", conf));
}

#[test]
fn sizes() {
    for (name, val) in [
        ("size_plain", "100"),
        ("size_k", "10k"),
        ("size_upper_k", "10K"),
        ("size_m", "2M"),
        ("size_g", "3G"),
        ("size_hex", "0x10"),
        ("size_octal", "010"),
        ("size_only_unit", "k"),
        ("size_bad_unit", "10m"),
        ("size_bad_digits", "1x0"),
        ("size_negative", "-5"),
        ("size_negative_zero", "-0"),
        ("size_huge", "18446744073709551615"),
        ("size_overflow_g", "9000000000G"),
        ("size_equals", "= 7"),
    ] {
        check(v(name, &format!("@T@/logs/d1 {{\n size {val}\n minsize {val}\n maxsize {val}\n}}\n")));
        check(v(&format!("{name}_global"), &format!("size {val}\n@T@/logs/d1 {{\n}}\n")));
    }
}

#[test]
fn size_without_value() {
    // QUIRK: "size\n" skips the line count; later line numbers are off.
    check(v("size_without_value", "size\nbogus\n@T@/logs/d1 {\n size\n minsize\n bogus\n}\n"));
    check(v("size_without_value_blank", "size   \nbogus\n"));
    check(v("size_at_eof", "@T@/logs/d1 {\n}\nsize"));
    check(v("size_equals_nothing", "size =\nbogus\n"));
}

#[test]
fn counts() {
    for (name, d, val) in [
        ("rotate_5", "rotate", "5"),
        ("rotate_minus_1", "rotate", "-1"),
        ("rotate_minus_2", "rotate", "-2"),
        ("rotate_hex", "rotate", "0x20"),
        ("rotate_junk", "rotate", "5x"),
        ("rotate_huge", "rotate", "99999999999"),
        ("start_0", "start", "0"),
        ("start_bad", "start", "-1"),
        ("start_junk", "start", "a"),
        ("shredcycles_3", "shredcycles", "3"),
        ("shredcycles_bad", "shredcycles", "3x"),
        ("shredcycles_negative", "shredcycles", "-3"),
        ("minage_5", "minage", "5"),
        ("maxage_5", "maxage", "5"),
        ("minage_bad", "minage", "5x"),
        ("maxage_bad", "maxage", "x"),
        ("maxage_huge", "maxage", "4294967296"),
    ] {
        check(v(name, &format!("@T@/logs/d1 {{\n {d} {val}\n}}\n# after\n@T@/logs/d2 {{\n}}\n")));
        check(v(&format!("{name}_global"), &format!("{d} {val}\n@T@/logs/d1 {{\n}}\n")));
    }
    check(v("rotate_missing", "@T@/logs/d1 {\n rotate\n}\n"));
}

#[test]
fn weekly_arguments() {
    for (name, arg) in [
        ("weekly_none", ""),
        ("weekly_0", " 0"),
        ("weekly_6", " 6"),
        ("weekly_7", " 7"),
        ("weekly_8", " 8"),
        ("weekly_word", " monday"),
        ("weekly_trailing", " 3 x"),
        ("weekly_plus", " +3"),
        ("weekly_negative", " -1"),
        ("weekly_equals", " = 2"),
    ] {
        check(v(name, &format!("@T@/logs/d1 {{\n weekly{arg}\n}}\n@T@/logs/d2 {{\n}}\n")));
    }
    check(v("weekly_at_eof", "@T@/logs/d1 {\n}\nweekly 3   "));
}

#[test]
fn create_arguments() {
    for (name, arg) in [
        ("create_none", ""),
        ("create_mode", " 0640"),
        ("create_mode_user_group", " 0640 alice users"),
        ("create_user_group", " alice users"),
        ("create_numeric", " 0600 1002 100"),
        ("create_quoted", " 0600 \"alice\" 'users'"),
        ("create_unterminated_quote", " 0600 \"alice users"),
        ("create_bad_mode", " 0648"),
        ("create_big_mode", " 7777777777"),
        ("create_unknown_user", " 0600 mallory users"),
        ("create_unknown_group", " 0600 alice wheel0"),
        ("create_unknown_uid", " 0600 4242 users"),
        ("create_extra", " 0600 alice users more"),
        ("create_blank_values", " \"\" \"\""),
    ] {
        check(v(name, &format!("@T@/logs/d1 {{\n create{arg}\n}}\n@T@/logs/d2 {{\n}}\n")));
    }
    check(v("create_at_eof_trailing", "@T@/logs/d1 {\n}\ncreate 0640 alice "));
}

#[test]
fn su_arguments() {
    for (name, arg) in [
        ("su_user_group", " alice users"),
        ("su_user_only", " alice"),
        ("su_none", ""),
        ("su_extra", " alice users x"),
        ("su_unknown", " mallory users"),
        ("su_numeric", " 1001 1001"),
    ] {
        check(v(name, &format!("@T@/logs/d1 {{\n su{arg}\n}}\n@T@/logs/d2 {{\n}}\n")));
        check(v(&format!("{name}_global"), &format!("su{arg}\n@T@/logs/d1 {{\n}}\n")));
    }
}

#[test]
fn createolddir_arguments() {
    for (name, arg) in [
        ("createolddir_none", ""),
        ("createolddir_mode", " 0700"),
        ("createolddir_full", " 0750 alice users"),
        ("createolddir_bad", " 0999"),
    ] {
        check(v(name, &format!("@T@/logs/d1 {{\n createolddir{arg}\n}}\n@T@/logs/d2 {{\n}}\n")));
    }
}

#[test]
fn mail_addresses() {
    check(v("mail_good", "mail root@example.com\n@T@/logs/d1 {\n mail x@y\n}\n"));
    // QUIRK: the message shows the rest of the file.
    check(v("mail_bad", "@T@/logs/d1 {\n mail a\tb\n}\n@T@/logs/d2 {\n}\n"));
    check(v("mail_bad_global", "mail a\x01b\n@T@/logs/d1 {\n}\n"));
    check(v("mail_missing", "@T@/logs/d1 {\n mail\n}\n"));
}

#[test]
fn errors_directive() {
    check(v("errors_directive", "errors\n@T@/logs/d1 {\n}\n"));
    check(v("errors_directive_address", "errors root@localhost\n@T@/logs/d1 {\n}\n"));
}

#[test]
fn unknown_and_malformed() {
    check(v("unknown_option", "frobnicate on\nrotate 2\n@T@/logs/d1 {\n frob\n}\n"));
    check(v("not_separated", "@T@/logs/d1 {\n compress;\n}\n@T@/logs/d2 {\n}\n"));
    check(v("not_separated_global", "rotate5\ncompress;\n@T@/logs/d1 {\n}\n"));
    check(v("not_separated_high_byte", "compress\u{e9}\n@T@/logs/d1 {\n}\n"));
    check(v("not_separated_nul", "@T@/logs/d1 {\n compress\0\n}\n"));
    check(v("keyword_equals", "rotate=3\n@T@/logs/d1 {\n rotate = 4\n}\n"));
    check(v("bad_line_start", "5 rotate\n@T@/logs/d1 {\n}\n"));
    check(v("bad_line_start_in_def", "@T@/logs/d1 {\n -rotate 3\n}\n@T@/logs/d2 {\n}\n"));
    check(v("crlf", "rotate 3\r\n@T@/logs/d1 {\r\n}\r\n"));
    check(v("comments", "# comment\n   # indented\n\t\n@T@/logs/d1 { # trailing\n # inside\n}\n"));
}

#[test]
fn braces() {
    check(v("unexpected_close", "}\n@T@/logs/d1 {\n}\n"));
    check(v("missing_open", "@T@/logs/d1\nrotate 1\n"));
    check(v("close_in_glob", "@T@/logs/d1 }\n"));
    check(v("text_after_close", "@T@/logs/d1 {\n} rotate 3\n@T@/logs/d2 {\n}\n"));
    check(v("nested_definition", "@T@/logs/d1 {\n @T@/logs/d2 {\n }\n}\nrotate 9\n"));
    check(v("one_line_definition", "@T@/logs/d1 { rotate 3 }\n"));
    check(v("open_on_next_line", "@T@/logs/d1\n{\n rotate 3\n}\n"));
    check(v("multi_line_patterns", "@T@/logs/d1\n# comment { }\n@T@/logs/d2\n{\n}\nbogus\n"));
    check(v("unterminated_definition", "@T@/logs/d1 {\n rotate 3\n"));
    // QUIRK: after an error, an indented "}" isn't seen.
    check(v("error_then_indented_close", "@T@/logs/d1 {\n rotate x\n    }\n@T@/logs/d2 {\n}\nrotate 4\n"));
    check(v("error_then_close", "@T@/logs/d1 {\n rotate x\n}\n@T@/logs/d2 {\n}\n"));
    check(v("error_then_scripts", "@T@/logs/d1 {\n rotate x\n postrotate\n  }\n endscript\n}\n@T@/logs/d2 {\n}\n"));
}

#[test]
fn patterns() {
    check(v("quoted_patterns", "\"@T@/logs/d1\" '@T@/logs/d2' {\n}\n"));
    check(v("bad_quote_pattern", "\"@T@/logs/d1 {\n}\n"));
    check(v("tilde_pattern", "~/x {\n}\n").env("HOME", "@T@/logs").dir("logs/x", 0o755));
    check(v("glob_dirs", "@T@/logs/d* {\n}\n"));
    let long = format!("@T@/logs/{} {{\n}}\n", "x".repeat(2100));
    check(v("glob_too_long", &long));
    check(
        v("glob_unreadable_dir", "@T@/logs/locked/*/x {\n}\n")
            .dir("logs/locked/sub", 0o755)
            .dir("logs/locked", 0o000)
            .run_as(ALICE),
    );
    check(
        v("glob_unreadable_dir_missingok", "@T@/logs/locked/*/x {\n missingok\n}\n")
            .dir("logs/locked/sub", 0o755)
            .dir("logs/locked", 0o000)
            .run_as(ALICE),
    );
    check(
        v("glob_unreadable_dir_missingok_late", "@T@/logs/locked/*/x {\n rotate 1\n missingok\n}\n")
            .dir("logs/locked/sub", 0o755)
            .dir("logs/locked", 0o000)
            .run_as(ALICE),
    );
    check(
        v("glob_unreadable_dir_global_missingok", "missingok\n@T@/logs/locked/*/x {\n}\n")
            .dir("logs/locked/sub", 0o755)
            .dir("logs/locked", 0o000)
            .run_as(ALICE),
    );
}

#[test]
fn scripts() {
    let conf = "@T@/logs/d1 {\n prerotate\n  echo pre\n endscript\n postrotate\n\techo post \"$1\"\n    endscript\n\
                firstaction\n echo first\nendscript\n lastaction\nendscript\n preremove\n  x\n  endscriptfoo\n  endscript\n}\n";
    check(v("scripts", conf));
    check(v("script_unterminated", "@T@/logs/d1 {\n postrotate\n  echo\n}\n"));
    check(v("script_global", "postrotate\n echo\nendscript\n@T@/logs/d1 {\n}\n"));
    check(v("script_same_line", "@T@/logs/d1 { postrotate endscript\n}\n"));
    check(v("script_endscript_digit", "@T@/logs/d1 {\n postrotate\n  x\n endscript1\n}\n"));
}

#[test]
fn taboo() {
    let setup = |c: Case| {
        c.conf("inc/a.conf", "rotate 1\n")
            .conf("inc/b.conf.rpmsave", "rotate 2\n")
            .conf("inc/c.conf~", "rotate 3\n")
            .conf("inc/d.foo", "rotate 4\n")
            .conf("inc/e.rhn-cfg-tmp-xyz", "rotate 5\n")
            .conf("inc/.hidden", "rotate 6\n")
            .conf("inc/f,v", "rotate 7\n")
            .conf("inc/g.bar", "rotate 8\n")
    };
    check(setup(v("taboo_default", "include inc\n")));
    check(setup(v("tabooext_add", "tabooext + .foo, .bar\ninclude inc\n")));
    check(setup(v("tabooext_replace", "tabooext .foo\ninclude inc\n")));
    check(setup(v("tabooext_empty_items", "tabooext + ,, .foo ,\ninclude inc\n")));
    check(setup(v("taboopat_add", "taboopat + *.f?o\ninclude inc\n")));
    check(setup(v("taboopat_replace", "taboopat a.*,,g*\ninclude inc\n")));
    check(setup(v("tabooext_in_definition", "@T@/logs/d1 {\n tabooext .x\n}\ninclude inc\n")));
    check(setup(v("taboopat_in_definition", "@T@/logs/d1 {\n taboopat .x\n}\ninclude inc\n")));
    check(setup(v("tabooext_missing", "tabooext\ninclude inc\n")));
}

#[test]
fn includes() {
    check(v("include_file", "include @T@/other.conf\n@T@/logs/d1 {\n}\n").conf("other.conf", "rotate 6\nweekly\n"));
    check(v("include_relative", "include other.conf\n").conf("other.conf", "@T@/logs/d1 {\n}\n"));
    check(v("include_missing", "include @T@/none\nrotate 5\n@T@/logs/d1 {\n}\n"));
    check(v("include_missing_arg", "include\nrotate 5\n@T@/logs/d1 {\n}\n"));
    check(v("include_self", "include @T@/logrotate.conf\n"));
    check(
        v("include_dir_order", "include @T@/inc\n@T@/logs/d1 {\n}\n")
            .conf("inc/b", "rotate 2\n")
            .conf("inc/a", "rotate 1\n")
            .conf("inc/C", "rotate 3\n")
            .conf("inc/_d", "rotate 4\n"),
    );
    check(
        v("include_dir_relative_names", "include inc\n")
            .conf("inc/a", "include sub\n")
            .conf("inc/sub/x", "@T@/logs/d1 {\n}\n"),
    );
    check(v("include_empty_dir", "include @T@/inc\nrotate 3\n@T@/logs/d1 {\n}\n").dir("inc", 0o755));
    check(v("include_unreadable_dir", "include @T@/inc\n").dir("inc", 0o000).run_as(ALICE));
    check(
        v("include_error_restores_defaults", "rotate 1\ninclude inc\n@T@/logs/d1 {\n}\n")
            .conf("inc/a", "rotate 5\nweekly\n@T@/logs/d2 {\n}\nbad line\n")
            .conf("inc/b", "compress\nmonthly\n}\n"),
    );
    check(
        v("include_file_error_restores", "rotate 1\ninclude @T@/x.conf\n@T@/logs/d1 {\n}\n")
            .conf("x.conf", "rotate 5\n}\n"),
    );
    check(
        v("include_in_definition", "@T@/logs/d1 {\n rotate 2\n include @T@/x.conf\n}\n")
            .conf("x.conf", "compress\nmonthly\n"),
    );
    check(
        v(
            "include_in_definition_error",
            "@T@/logs/d1 {\n rotate 2\n include @T@/x.conf\n}\nrotate 3\n@T@/logs/d2 {\n}\n",
        )
        .conf("x.conf", "compress\n}\n"),
    );
    check(
        v("include_in_definition_with_logs", "@T@/logs/d1 {\n include @T@/x.conf\n}\n@T@/logs/d2 {\n}\n")
            .conf("x.conf", "@T@/logs/d1/a {\n}\n@T@/logs/d1/b {\n}\nbad\n")
            .dir("logs/d1/a", 0o755)
            .dir("logs/d1/b", 0o755),
    );
    check(
        v("include_in_definition_with_logs_error", "@T@/logs/d1 {\n include @T@/x.conf\n}\n@T@/logs/d2 {\n}\n")
            .conf("x.conf", "@T@/logs/d1/a {\n}\n@T@/logs/d1/b {\n}\n}\n")
            .dir("logs/d1/a", 0o755)
            .dir("logs/d1/b", 0o755),
    );
    check(
        v("include_dir_in_definition_error", "@T@/logs/d1 {\n include @T@/inc\n}\n@T@/logs/d2 {\n}\n")
            .conf("inc/a", "@T@/logs/d1/a {\n}\n")
            .conf("inc/b", "rotate x\n")
            .dir("logs/d1/a", 0o755),
    );
}

#[test]
fn include_home() {
    check(v("include_home", "include ~/x.conf\n").env("HOME", "@T@/home/root").conf("home/root/x.conf", "rotate 3\n"));
    check(v("include_home_from_passwd", "include ~/x.conf\n").no_env("HOME").conf("home/root/x.conf", "rotate 3\n"));
    check(v("include_tilde_user", "include ~root/x.conf\n").conf("home/root/x.conf", "rotate 3\n"));
    check(v("olddir_home", "@T@/logs/d1 {\n olddir ~/old\n}\n").env("HOME", "@T@/home/root"));
}

#[test]
fn paths() {
    check(v("olddir_bad_path", "@T@/logs/d1 {\n olddir /a b\n}\n@T@/logs/d2 {\n}\n"));
    check(v("olddir_missing", "@T@/logs/d1 {\n olddir\n}\n"));
    check(v(
        "compresscmd",
        "compresscmd /usr/bin/bzip2\n@T@/logs/d1 {\n compresscmd xz\n}\n@T@/logs/d2 {\n compresscmd /usr/bin/\n}\n",
    ));
    check(v(
        "compresscmd_unknown",
        "compresscmd /opt/lz4\nuncompresscmd /opt/unlz4\ncompressext .lz4\n@T@/logs/d1 {\n}\n",
    ));
    check(v("compresscmd_bad", "compresscmd /a\u{7f}b\n@T@/logs/d1 {\n}\n"));
    check(v("compressoptions", "compressoptions -9 \"-x y\"\n@T@/logs/d1 {\n compressoptions\n}\n"));
    check(v("compressoptions_bad", "@T@/logs/d1 {\n compressoptions \"-9\n}\n"));
    check(v(
        "extensions",
        "extension .log\naddextension .x\n@T@/logs/d1 {\n extension\n}\n@T@/logs/d2 {\n addextension .y\n}\n",
    ));
    check(v("dateformat", "dateformat -%Y%m%d\n@T@/logs/d1 {\n dateformat\n}\n"));
}

#[test]
fn utf8_paths() {
    let conf = "@T@/logs/d1 {\n olddir /tmp/\u{e9}t\u{e9}\n}\n@T@/logs/d2 {\n olddir /tmp/\u{a0}x\n}\n\
                compresscmd /x/\u{3000}y\n";
    check(v("utf8_paths_c", conf));
    check(v("utf8_paths_utf8", conf).env("LC_ALL", "en_US.UTF-8"));
    check(
        v("latin1_path_utf8", "@T@/logs/d1 {\n}\ncompresscmd /x/\u{0}\n")
            .file(CONF, b"compresscmd /x/\xe9\n", 0o644)
            .env("LC_ALL", "en_US.UTF-8"),
    );
}

#[test]
fn olddir_checks() {
    // The checks run per file, and a failure drops the definition, so no
    // file is rotated.
    check(v("olddir_log_dir_missing", "@T@/logs/none/a.log {\n olddir old\n}\n"));
    check(v(
        "olddir_log_dir_missing_missingok",
        "@T@/logs/none/a.log @T@/logs/d1/a.log {\n olddir old\n missingok\n}\n@T@/logs/none/b.log {\n olddir /x\n}\n",
    ));
    check(v("olddir_target_missing", "@T@/logs/d1/a.log {\n olddir old\n}\n"));
    check(v("olddir_target_missing_abs", "@T@/logs/d1/a.log {\n olddir @T@/nowhere\n}\n"));
    check(v("olddir_different_device", "@T@/logs/d1/a.log {\n olddir /proc\n}\n"));
    check(v(
        "createolddir_then_error",
        "@T@/logs/d1/a.log @T@/logs/none/b.log {\n olddir old/sub\n createolddir 0750 alice users\n}\n",
    ));
    check(v(
        "createolddir_abs_then_error",
        "@T@/logs/d1/a.log @T@/logs/none/b.log {\n olddir @T@/archive/x/y\n createolddir\n}\n",
    ));
    check(
        v(
            "createolddir_on_file_then_error",
            "@T@/logs/d1/a.log @T@/logs/none/b.log {\n olddir f/x\n createolddir\n}\n",
        )
        .file("logs/d1/f", b"", 0o644),
    );
    check(
        v(
            "createolddir_su_then_error",
            "@T@/logs/d1/a.log @T@/logs/none/b.log {\n su alice users\n olddir old\n createolddir 0700\n}\n",
        )
        .dir_owned("logs/d1", 0o755, 1001, 1001),
    );
    check(v("createolddir_denied", "@T@/logs/d1/a.log {\n su alice users\n olddir old\n createolddir 0700\n}\n"));
}

#[test]
fn duplicate_and_missing_logs() {
    check(v("glob_missing_missingok_error", "@T@/logs/d1/*.log {\n missingok\n rotate x\n}\n"));
}

#[test]
fn locale_messages() {
    check(Case::new("locale_messages", &["-v", "-s", "@T@/state", "nope.conf"]).env("LC_ALL", "en_US.UTF-8"));
}
