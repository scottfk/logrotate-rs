//! Port of log.c: messages to stderr, to the `-l` file, and to syslog.

use std::cell::Cell;

use crate::sys;

pub const MESS_REALDEBUG: i32 = 1;
pub const MESS_DEBUG: i32 = 2;
pub const MESS_WARN: i32 = 4;
pub const MESS_ERROR: i32 = 5;
pub const MESS_FATAL: i32 = 6;

thread_local! {
    static LOG_LEVEL: Cell<i32> = const { Cell::new(MESS_DEBUG) };
    static MESSAGE_FILE: Cell<Option<sys::Fd>> = const { Cell::new(None) };
    static TO_SYSLOG: Cell<bool> = const { Cell::new(false) };
}

/// `logSetLevel()`.
pub fn set_level(level: i32) {
    LOG_LEVEL.with(|l| l.set(level));
}

/// `logSetMessageFile()`.
pub fn set_message_file(fd: sys::Fd) {
    MESSAGE_FILE.with(|f| f.set(Some(fd)));
}

/// `logToSyslog(1)`.
pub fn to_syslog() {
    TO_SYSLOG.with(|s| s.set(true));
    sys::openlog_logrotate();
}

/// `log_once()`: the level's prefix, then the message.
fn log_once(fd: sys::Fd, level: i32, msg: &[u8]) {
    let prefix: &[u8] = match level {
        MESS_DEBUG => b"",
        MESS_WARN => b"warning: ",
        // QUIRK: MESS_REALDEBUG falls into the default case too.
        _ => b"error: ",
    };
    let mut out = prefix.to_vec();
    out.extend_from_slice(msg);
    let saved = sys::errno();
    sys::write_all(fd, &out);
    sys::set_errno(saved);
}

/// `message()`.  The `-l` file gets every message, whatever the level.
pub fn message(level: i32, msg: &[u8]) {
    if level >= LOG_LEVEL.with(Cell::get) {
        log_once(2, level, msg);
    }
    if let Some(fd) = MESSAGE_FILE.with(Cell::get) {
        log_once(fd, level, msg);
    }
    if TO_SYSLOG.with(Cell::get) {
        let pri = libc::LOG_USER
            | match level {
                MESS_REALDEBUG => libc::LOG_DEBUG,
                MESS_DEBUG => libc::LOG_INFO,
                MESS_WARN => libc::LOG_WARNING,
                MESS_ERROR => libc::LOG_ERR,
                MESS_FATAL => libc::LOG_CRIT,
                _ => libc::LOG_INFO,
            };
        let saved = sys::errno();
        sys::syslog(pri, msg);
        sys::set_errno(saved);
    }
    if level == MESS_FATAL {
        sys::exit(1);
    }
}

/// `message(level, ...)` with the pieces concatenated (see `bcat!`).
#[macro_export]
macro_rules! msg {
    ($level:expr, $($e:expr),* $(,)?) => {
        $crate::log::message($level, &$crate::bcat!($($e),*))
    };
}
