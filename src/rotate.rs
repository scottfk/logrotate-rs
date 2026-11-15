//! Port of logrotate.c's rotation code.
//!
//! Phase 1 has what the configuration and the state file need: switching
//! the effective user, `createOutputFile()`, and the start of
//! `rotateLogSet()` (its description of the log set, and the "no logs"
//! case).  Rotating files is phase 2.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};

use crate::config::{Criterium, LOG_FLAG_IFEMPTY, LogInfo};
use crate::log::{MESS_DEBUG, MESS_ERROR, message};
use crate::msg;
use crate::sys;

const ROOT_UID: u32 = 0;

static DEBUG: AtomicBool = AtomicBool::new(false);
static NOW_SECS: AtomicI64 = AtomicI64::new(0);
static SAVE_EUID: AtomicU32 = AtomicU32::new(0);
static SAVE_EGID: AtomicU32 = AtomicU32::new(0);

/// logrotate.c's `debug`.
pub fn debug() -> bool {
    DEBUG.load(Ordering::Relaxed)
}

pub fn set_debug() {
    DEBUG.store(true, Ordering::Relaxed);
}

/// logrotate.c's `nowSecs`.
pub fn now_secs() -> i64 {
    NOW_SECS.load(Ordering::Relaxed)
}

pub fn set_now_secs(t: i64) {
    NOW_SECS.store(t, Ordering::Relaxed);
}

/// `switch_user()`: 0, or 1 after an error message.
pub fn switch_user(user: u32, group: u32) -> i32 {
    let egid = sys::getegid();
    let euid = sys::geteuid();
    SAVE_EGID.store(egid, Ordering::Relaxed);
    SAVE_EUID.store(euid, Ordering::Relaxed);
    if euid == user && egid == group {
        return 0;
    }
    let pid = sys::getpid();
    msg!(MESS_DEBUG, format!("switching euid from {euid} to {user} and egid from {egid} to {group} (pid {pid})\n"));
    if sys::setegid(group) != 0 || sys::seteuid(user) != 0 {
        msg!(
            MESS_ERROR,
            format!("error switching euid from {euid} to {user} and egid from {egid} to {group} (pid {pid}): "),
            sys::strerror(sys::errno()),
            "\n"
        );
        return 1;
    }
    0
}

/// `switch_user_back()`.
pub fn switch_user_back() -> i32 {
    switch_user(SAVE_EUID.load(Ordering::Relaxed), SAVE_EGID.load(Ordering::Relaxed))
}

/// `is_acl_well_supported()`.
fn is_acl_well_supported(err: i32) -> bool {
    !matches!(err, libc::ENOTSUP | libc::EINVAL | libc::ENOSYS | libc::EBUSY)
}

/// `createOutputFile()`: the new file's fd, or -1 after an error message.
pub fn create_output_file(
    file_name: &[u8],
    flags: i32,
    sb: &sys::Stat,
    acl: Option<&sys::Acl>,
    force_mode: bool,
) -> i32 {
    let mut fd = -1;
    for _ in 0..2 {
        fd = sys::open(
            file_name,
            flags | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW,
            (libc::S_IRUSR | libc::S_IWUSR) & sb.st_mode,
        );
        if fd >= 0 || sys::errno() != libc::EEXIST {
            break;
        }
        // The destination exists, while it should not.
        let now = sys::localtime(now_secs());
        let mut backup = file_name.to_vec();
        backup.extend_from_slice(&sys::strftime(c"-%Y%m%d%H", &now));
        backup.extend_from_slice(b".backup");
        msg!(MESS_ERROR, "destination ", file_name, " already exists, renaming to ", backup, "\n");
        if sys::rename(file_name, &backup) != 0 {
            msg!(
                MESS_ERROR,
                "error renaming already existing output file ",
                file_name,
                " to ",
                backup,
                ": ",
                sys::strerror(sys::errno()),
                "\n"
            );
            return -1;
        }
    }
    if fd < 0 {
        msg!(MESS_ERROR, "error creating output file ", file_name, ": ", sys::strerror(sys::errno()), "\n");
        return -1;
    }
    if sys::fchmod(fd, (libc::S_IRUSR | libc::S_IWUSR) & sb.st_mode) != 0 {
        msg!(MESS_ERROR, "error setting mode of ", file_name, ": ", sys::strerror(sys::errno()), "\n");
        sys::close(fd);
        return -1;
    }
    let sb_create = match sys::fstat(fd) {
        Ok(s) => s,
        Err(e) => {
            msg!(MESS_ERROR, "fstat of ", file_name, " failed: ", sys::strerror(e), "\n");
            sys::close(fd);
            return -1;
        }
    };
    // Only attempt to set user/group if running as root.
    if sys::geteuid() == ROOT_UID
        && (sb_create.st_uid != sb.st_uid || sb_create.st_gid != sb.st_gid)
        && sys::fchown(fd, sb.st_uid, sb.st_gid) != 0
    {
        msg!(
            MESS_ERROR,
            "error setting owner of ",
            file_name,
            format!(" to uid {} and gid {}: ", sb.st_uid, sb.st_gid),
            sys::strerror(sys::errno()),
            "\n"
        );
        sys::close(fd);
        return -1;
    }
    let mut acl_set = false;
    if !force_mode && let Some(acl) = acl {
        if acl.set_fd(fd) == -1 {
            let e = sys::errno();
            if is_acl_well_supported(e) {
                msg!(MESS_ERROR, "setting ACL for ", file_name, ": ", sys::strerror(e), "\n");
                sys::close(fd);
                return -1;
            }
        } else {
            acl_set = true;
        }
    }
    if (!acl_set || force_mode) && sys::fchmod(fd, sb.st_mode) != 0 {
        msg!(MESS_ERROR, "error setting mode of ", file_name, ": ", sys::strerror(sys::errno()), "\n");
        sys::close(fd);
        return -1;
    }
    fd
}

/// The start of `rotateLogSet()`: the description of the log set, then
/// the "no logs" case.  Returns the set's error status.
pub fn rotate_log_set(log: &LogInfo, force: bool) -> i32 {
    // QUIRK: a definition whose include failed has lost its pattern, and
    // printf prints a NULL %s as "(null)".
    let pattern: &[u8] = log.pattern.as_deref().unwrap_or(b"(null)");
    msg!(MESS_DEBUG, "\nrotating pattern: ", pattern, " ");
    if force {
        message(MESS_DEBUG, b"forced from command line ");
    } else {
        match log.criterium {
            Criterium::Hourly => message(MESS_DEBUG, b"hourly "),
            Criterium::Days => msg!(MESS_DEBUG, format!("after {} days ", log.threshold)),
            Criterium::Weekly => message(MESS_DEBUG, b"weekly "),
            Criterium::Monthly => message(MESS_DEBUG, b"monthly "),
            Criterium::Yearly => message(MESS_DEBUG, b"yearly "),
            Criterium::Size => msg!(MESS_DEBUG, format!("{} bytes ", log.threshold)),
        }
    }
    if let Some(d) = &log.old_dir {
        msg!(MESS_DEBUG, "olddir is ", d, ", ");
    }
    if log.has(LOG_FLAG_IFEMPTY) {
        message(MESS_DEBUG, b"empty log files are rotated, ");
    } else {
        message(MESS_DEBUG, b"empty log files are not rotated, ");
    }
    if log.minsize != 0 {
        msg!(MESS_DEBUG, format!("only log files >= {} bytes are rotated, ", log.minsize));
    }
    if log.maxsize != 0 {
        msg!(MESS_DEBUG, format!("log files >= {} are rotated earlier, ", log.maxsize));
    }
    if log.rotate_min_age != 0 {
        msg!(MESS_DEBUG, format!("only log files older than {} days are rotated, ", log.rotate_min_age));
    }
    if log.rotate_count == -1 && log.rotate_age == 0 {
        message(MESS_DEBUG, b"old logs are kept forever\n");
    } else {
        if let Some(a) = &log.log_address {
            msg!(MESS_DEBUG, "old logs mailed to ", a, ", ");
        }
        if log.rotate_count == 0 {
            message(MESS_DEBUG, b"no old logs will be kept\n");
        } else {
            if log.rotate_count == -1 {
                message(MESS_DEBUG, b"(unlimited rotations), ");
            } else {
                msg!(MESS_DEBUG, format!("({} rotations), ", log.rotate_count));
            }
            message(MESS_DEBUG, b"old logs are removed");
            if log.rotate_age > 0 {
                msg!(MESS_DEBUG, format!(" after {} days", log.rotate_age));
            }
            message(MESS_DEBUG, b"\n");
        }
    }
    if log.files.is_empty() {
        message(MESS_DEBUG, b"No logs found. Rotation not needed.\n");
        return 0;
    }
    message(MESS_ERROR, b"logrotate-rs: rotating log files is not implemented yet\n");
    1
}
