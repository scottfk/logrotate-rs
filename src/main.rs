//! logrotate-rs: a drop-in port of logrotate 3.22.0.
//!
//! Upstream license: GPL-2.0-or-later (see LICENSE).  The modules follow
//! upstream's files: config.c ([`config`]), logrotate.c ([`rotate`],
//! [`state`] and `main()` here) and log.c ([`log`]).

mod bytes;
mod config;
mod log;
mod rotate;
mod state;
mod sys;

use std::ffi::CStr;
use std::os::unix::ffi::OsStringExt;

use log::{MESS_DEBUG, MESS_ERROR, MESS_WARN, message};

const VERSION: &str = "3.22.0";
const DEFAULT_MAIL_COMMAND: &CStr = c"/bin/mail";
const STATEFILE: &CStr = c"/var/lib/misc/logrotate.status";

fn main() {
    sys::sigpipe_default();
    let args: Vec<Vec<u8>> = std::env::args_os().map(OsStringExt::into_vec).collect();
    std::process::exit(logrotate_main(&args));
}

/// Upstream's `main()`.
fn logrotate_main(args: &[Vec<u8>]) -> i32 {
    log::set_level(MESS_WARN);
    sys::setlocale_all();

    let mut popt = sys::Popt::new(args, DEFAULT_MAIL_COMMAND, STATEFILE);
    popt.read_default_config();
    popt.set_other_option_help(c"[OPTION...] <configfile>");

    let mut arg;
    loop {
        arg = popt.next_opt();
        if arg < 0 {
            break;
        }
        match u8::try_from(arg).unwrap_or(0) {
            b'd' | b'v' => {
                if arg == i32::from(b'd') {
                    rotate::set_debug();
                    message(
                        MESS_WARN,
                        b"logrotate in debug mode does nothing except printing debug messages!  \
                          Consider using verbose mode (-v) instead if this is not what you want.\n\n",
                    );
                }
                log::set_level(MESS_DEBUG);
            }
            b'l' => {
                let log_file = popt.log_file().unwrap_or_default();
                if log_file == b"syslog" {
                    log::to_syslog();
                } else {
                    // fopen(logFile, "w")
                    let fd = sys::open(&log_file, libc::O_WRONLY | libc::O_CREAT | libc::O_TRUNC, 0o666);
                    if fd < 0 {
                        msg!(MESS_ERROR, "error opening log file ", log_file, ": ", sys::strerror(sys::errno()), "\n");
                        continue;
                    }
                    log::set_message_file(fd);
                }
            }
            b'V' => {
                let text = format!(
                    "logrotate {VERSION}\n\n    Default mail command:       /bin/mail\n    \
                     Default compress command:   /bin/gzip\n    \
                     Default uncompress command: /bin/gunzip\n    \
                     Default compress extension: .gz\n    \
                     Default state file path:    /var/lib/misc/logrotate.status\n    \
                     ACL support:                yes\n    \
                     SELinux support:            no\n"
                );
                sys::write_all(1, text.as_bytes());
                popt.free();
                sys::exit(0);
            }
            _ => {}
        }
    }

    if arg < -1 {
        // QUIRK: the error code passed to poptStrerror() is main()'s rc, 0.
        let text = bytes_cat(&[
            b"logrotate: bad argument ",
            &popt.bad_option(sys::POPT_BADOPTION_NOALIAS),
            b": ",
            &sys::Popt::strerror(0),
            b"\n",
        ]);
        sys::write_all(2, &text);
        popt.free();
        return 2;
    }

    let Some(files) = popt.get_args() else {
        sys::write_all(
            2,
            format!(
                "logrotate {VERSION} - Copyright (C) 1995-2001 Red Hat, Inc.\n\
                 This may be freely redistributed under the terms of the GNU General Public License\n\n"
            )
            .as_bytes(),
        );
        popt.print_usage_stderr();
        popt.free();
        sys::exit(1);
    };

    let skip_state_lock = popt.skip_state_lock();
    let wait_for_state_lock = popt.wait_for_state_lock();
    if skip_state_lock && wait_for_state_lock {
        sys::write_all(2, b"logrotate: options --skip-state-lock and --wait-for-state-lock are mutually exclusive\n");
        popt.free();
        sys::exit(1);
    }
    let force = popt.force();
    let state_file = popt.state_file();
    let _mail_command = popt.mail_command();

    let mut rc = 0;
    let (err, logs) = config::read_all_config_paths(&files);
    if err != 0 {
        rc = 1;
    }
    popt.free();
    rotate::set_now_secs(sys::time_now());
    sys::tzset();

    if !rotate::debug() && state::lock_state(&state_file, skip_state_lock, wait_for_state_lock) != 0 {
        sys::exit(3);
    }

    let mut states = state::States::default();
    if state::read_state(&mut states, &state_file) != 0 {
        rc = 1;
    }

    msg!(MESS_DEBUG, format!("\nHandling {} logs\n", logs.num_logs));

    for log in logs.iter() {
        rc |= rotate::rotate_log_set(log, force);
    }

    if !rotate::debug() {
        rc |= state::write_state(&mut states, &state_file);
    }

    i32::from(rc != 0)
}

fn bytes_cat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}
