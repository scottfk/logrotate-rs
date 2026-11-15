//! Port of logrotate.c's rotation code.
//!
//! Everything from switching users to `rotateLogSet()`: deciding what needs
//! rotating, renaming and creating, copying and truncating, compressing,
//! shredding, mailing, and running scripts.  Functions are named after the
//! C ones and keep their order of checks and messages.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};

use crate::bcat;
use crate::config::*;
use crate::log::{MESS_DEBUG, MESS_ERROR, message};
use crate::msg;
use crate::state::{LogState, States};
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
    // QUIRK: otherwise (under `su`, or run as a user) the owner and group
    // asked for by `create` are dropped without a message.
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

const DAY_SECONDS: i64 = 86400;
/// glibc's `BUFSIZ`.
const BUFSIZ: usize = 8192;

thread_local! {
    static MAIL_COMMAND: std::cell::RefCell<Vec<u8>> = std::cell::RefCell::new(b"/bin/mail".to_vec());
}

/// logrotate.c's `mailCommand` (`-m`).
pub fn set_mail_command(cmd: Vec<u8>) {
    MAIL_COMMAND.with(|m| *m.borrow_mut() = cmd);
}

fn mail_command() -> Vec<u8> {
    MAIL_COMMAND.with(|m| m.borrow().clone())
}

/// `struct logNames`.
#[derive(Debug, Default)]
struct LogNames {
    first_rotated: Option<Vec<u8>>,
    dispose_name: Option<Vec<u8>>,
    final_name: Option<Vec<u8>>,
    dir_name: Vec<u8>,
    base_name: Vec<u8>,
}

/// A C `%s` of a possibly-NULL string.
fn or_null(s: &Option<Vec<u8>>) -> &[u8] {
    s.as_deref().unwrap_or(b"(null)")
}

/// `switch_user_permanently()`: 0, or nonzero after an error message.
fn switch_user_permanently(log: &LogInfo) -> i32 {
    let group = sys::getegid();
    let user = sys::geteuid();
    if !log.has(LOG_FLAG_SU) {
        return 0;
    }
    let pid = sys::getpid();
    if user != log.su_uid {
        msg!(
            MESS_ERROR,
            format!("current euid ({user}) does not match uid of log configuration ({}) (pid {pid})\n", log.su_uid)
        );
        return 1;
    }
    if group != log.su_gid {
        msg!(
            MESS_ERROR,
            format!("current egid ({group}) does not match gid of log configuration ({}) (pid {pid})\n", log.su_gid)
        );
        return 1;
    }
    // Already the final user and group?
    if sys::getuid() == user && sys::getgid() == group {
        return 0;
    }
    // Switch to full root first.
    if sys::setgid(sys::getgid()) != 0 || sys::setuid(sys::getuid()) != 0 {
        msg!(MESS_ERROR, format!("error getting rid of euid != uid (pid {pid}): "), sys::strerror(sys::errno()), "\n");
        return 1;
    }
    msg!(MESS_DEBUG, format!("switching uid to {user} and gid to {group} permanently (pid {pid})\n"));
    if sys::setgid(group) != 0 || sys::setuid(user) != 0 {
        msg!(
            MESS_ERROR,
            format!("error switching uid to {user} and gid to {group} (pid {pid}): "),
            sys::strerror(sys::errno()),
            "\n"
        );
        return 1;
    }
    if user != ROOT_UID && (sys::setuid(ROOT_UID) != -1 || sys::seteuid(ROOT_UID) != -1) {
        msg!(MESS_ERROR, format!("failed to switch user permanently, able to switch back (pid {pid})\n"));
        return 1;
    }
    if sys::chdir(b"/") != 0 {
        msg!(MESS_ERROR, "failed to change current directory to root path: ", sys::strerror(sys::errno()), "\n");
        return -1;
    }
    0
}

/// `switch_user_back_permanently()` (used in children only).
fn switch_user_back_permanently() -> i32 {
    let (u, g) = (SAVE_EUID.load(Ordering::Relaxed), SAVE_EGID.load(Ordering::Relaxed));
    let ret = switch_user(u, g);
    SAVE_EUID.store(u, Ordering::Relaxed);
    SAVE_EGID.store(g, Ordering::Relaxed);
    ret
}

/// `open_logfile()`: the fd, or -1 with errno set (ENOTSUP for anything but
/// a regular file with one link, unless `allowhardlink`).
fn open_logfile(path: &[u8], log: &LogInfo, write_access: bool) -> sys::Fd {
    let fd = sys::open(
        path,
        libc::O_NOFOLLOW | libc::O_NOCTTY | libc::O_NONBLOCK | if write_access { libc::O_RDWR } else { libc::O_RDONLY },
        0,
    );
    if fd < 0 {
        return fd;
    }
    let sb = match sys::fstat(fd) {
        Ok(sb) => sb,
        Err(e) => {
            sys::close(fd);
            sys::set_errno(e);
            return -1;
        }
    };
    if !sys::s_isreg(sb.st_mode) || (sb.st_nlink != 1 && !log.has(LOG_FLAG_ALLOWHARDLINK)) {
        sys::close(fd);
        sys::set_errno(libc::ENOTSUP);
        return -1;
    }
    // O_NONBLOCK is unspecified for regular files by POSIX.
    if !sys::clear_nonblock(fd) {
        let e = sys::errno();
        sys::close(fd);
        sys::set_errno(e);
        return -1;
    }
    fd
}

/// `movefd()`: dup2, then close the old fd.  QUIRK: it closes it only when
/// dup2 returns 0, that is, when the new fd is stdin.
fn movefd(old: sys::Fd, new: sys::Fd) -> i32 {
    if old == new {
        return 0;
    }
    let rc = sys::dup2(old, new);
    if rc == 0 {
        sys::close(old);
    }
    rc
}

/// `runScript()`: the raw wait status (0 for success).
fn run_script(log: &LogInfo, logfn: Option<&[u8]>, logrotfn: Option<&[u8]>, script: &[u8]) -> i32 {
    if debug() {
        msg!(
            MESS_DEBUG,
            "running script with args ",
            logfn.unwrap_or(b"(null)"),
            " ",
            logrotfn.unwrap_or(b""),
            ": \"",
            script,
            "\"\n"
        );
        return 0;
    }
    let pid = sys::fork();
    if pid == -1 {
        msg!(MESS_ERROR, "cannot fork: ", sys::strerror(sys::errno()), "\n");
        return 1;
    }
    if pid == 0 {
        if log.has(LOG_FLAG_SU) && switch_user_back_permanently() != 0 {
            sys::exit(1);
        }
        // execl() stops at the first NULL argument.
        let mut args: Vec<&[u8]> = vec![b"sh", b"-c", script, b"logrotate_script"];
        if let Some(f) = logfn {
            args.push(f);
            if let Some(r) = logrotfn {
                args.push(r);
            }
        }
        sys::execv(b"/bin/sh", &args);
        msg!(MESS_ERROR, "cannot execute sub-shell: ", sys::strerror(sys::errno()), "\n");
        sys::exit(1);
    }
    sys::wait_any().1
}

/// `shred_file()`: unlink, or shred and unlink with `shred`.  0 on success.
fn shred_file(fd: sys::Fd, filename: &[u8], log: &LogInfo) -> i32 {
    if let Some(pre) = &log.preremove {
        message(MESS_DEBUG, b"running preremove script\n");
        if run_script(log, Some(filename), None, pre) != 0 {
            msg!(
                MESS_ERROR,
                "error running preremove script for ",
                filename,
                " of '",
                or_null(&log.pattern),
                "'. Not removing this file.\n"
            );
            return 1;
        }
    }
    if log.has(LOG_FLAG_SHRED) {
        if !log.has(LOG_FLAG_ALLOWHARDLINK) {
            match sys::fstat(fd) {
                Err(e) => {
                    msg!(MESS_ERROR, "cannot stat ", filename, ": ", sys::strerror(e), "\n");
                    return 1;
                }
                Ok(sb) if sb.st_nlink != 1 => {
                    msg!(
                        MESS_ERROR,
                        "failed to shred \"",
                        filename,
                        "\", because shredding files with multiple hard links is disabled for ",
                        or_null(&log.pattern),
                        ".\n"
                    );
                    return 1;
                }
                Ok(_) => {}
            }
        }
        msg!(MESS_DEBUG, "Using shred to remove the file ", filename, "\n");
        let count = log.shred_cycles.to_string();
        let mut cmd: Vec<&[u8]> = vec![b"shred", b"-u"];
        if log.shred_cycles != 0 {
            cmd.push(b"-n");
            cmd.push(count.as_bytes());
        }
        cmd.push(b"-");
        let pid = sys::fork();
        if pid == -1 {
            msg!(MESS_ERROR, "cannot fork: ", sys::strerror(sys::errno()), "\n");
            return 1;
        }
        if pid == 0 {
            movefd(fd, 1);
            if switch_user_permanently(log) != 0 {
                sys::exit(1);
            }
            sys::execvp(b"shred", &cmd);
            msg!(MESS_ERROR, "cannot execute shred command: ", sys::strerror(sys::errno()), "\n");
            sys::exit(1);
        }
        let (_, status) = sys::wait_any();
        if !sys::exited_ok(status) {
            msg!(MESS_ERROR, "Failed to shred ", filename, ", trying unlink\n");
            return sys::unlink(filename);
        }
        // shred doesn't remove the file itself.
    }
    if sys::unlink(filename) == 0 {
        return 0;
    }
    let e = sys::errno();
    if e != libc::ENOENT {
        return 1;
    }
    // Unlinking a log that no longer exists is not fatal.
    msg!(MESS_ERROR, "error unlinking log file ", filename, ": ", sys::strerror(e), "\n");
    0
}

/// `removeLogFile()`.
fn remove_log_file(name: &[u8], log: &LogInfo) -> i32 {
    msg!(MESS_DEBUG, "removing old log ", name, "\n");
    let mut fd = -1;
    if log.has(LOG_FLAG_SHRED) {
        fd = open_logfile(name, log, true);
        if fd < 0 {
            msg!(MESS_ERROR, "error opening ", name, ": ", sys::strerror(sys::errno()), "\n");
            return 1;
        }
    }
    let mut result = 0;
    if !debug() && shred_file(fd, name, log) != 0 {
        msg!(MESS_ERROR, "Failed to remove old log ", name, ": ", sys::strerror(sys::errno()), "\n");
        result = 1;
    }
    if fd != -1 {
        sys::close(fd);
    }
    result
}

/// An ACL from `acl_get_fd()`; Err after an error message when ACLs are
/// supported and reading one failed.
fn get_acl_fd(fd: sys::Fd, name: &[u8]) -> Result<Option<sys::Acl>, ()> {
    match sys::Acl::get_fd(fd) {
        Ok(a) => Ok(Some(a)),
        Err(e) if is_acl_well_supported(e) => {
            msg!(MESS_ERROR, "getting file ACL ", name, ": ", sys::strerror(e), "\n");
            Err(())
        }
        Err(_) => Ok(None),
    }
}

/// `compressLogFile()`.
fn compress_log_file(name: &[u8], log: &LogInfo, sb: &sys::Stat) -> i32 {
    msg!(MESS_DEBUG, "compressing log with: ", or_null(&log.compress_prog), "\n");
    if debug() {
        return 0;
    }
    let shred = log.has(LOG_FLAG_SHRED);
    let in_file = open_logfile(name, log, shred);
    if in_file < 0 {
        msg!(
            MESS_ERROR,
            "unable to open ",
            name,
            " (",
            if shred { "read-write" } else { "read-only" },
            ") for compression: ",
            sys::strerror(sys::errno()),
            "\n"
        );
        return 1;
    }
    let Ok(acl) = get_acl_fd(in_file, name) else {
        sys::close(in_file);
        return 1;
    };
    let compressed = bcat!(name, or_null(&log.compress_ext));
    let out_file = create_output_file(&compressed, libc::O_RDWR, sb, acl.as_ref(), false);
    drop(acl);
    if out_file < 0 {
        sys::close(in_file);
        return 1;
    }
    let (pr, pw) = match sys::pipe() {
        Ok(p) => p,
        Err(e) => {
            msg!(MESS_ERROR, "error opening pipe for compress: ", sys::strerror(e), "\n");
            sys::close(in_file);
            sys::close(out_file);
            return 1;
        }
    };
    let pid = sys::fork();
    if pid == -1 {
        msg!(MESS_ERROR, "cannot fork: ", sys::strerror(sys::errno()), "\n");
        for fd in [in_file, out_file, pw, pr] {
            sys::close(fd);
        }
        return 1;
    }
    if pid == 0 {
        sys::close(pr);
        movefd(in_file, 0);
        movefd(out_file, 1);
        if switch_user_permanently(log) != 0 {
            sys::exit(1);
        }
        movefd(pw, 2);
        // For custom compress scripts.
        sys::putenv(&bcat!("LOGROTATE_COMPRESSED_FILENAME=", name));
        let prog = log.compress_prog.clone().unwrap_or_default();
        let mut cmd: Vec<&[u8]> = vec![&prog];
        if let Some(opts) = &log.compress_options {
            cmd.extend(opts.iter().map(Vec::as_slice));
        }
        sys::execvp(&prog, &cmd);
        msg!(MESS_ERROR, "cannot execute compress command '", prog, "': ", sys::strerror(sys::errno()), "\n");
        sys::exit(1);
    }
    sys::close(pw);
    let mut error_printed = false;
    let mut buf = [0u8; 4096];
    loop {
        let n = sys::read(pr, &mut buf[..4095]);
        if n < 0 {
            if sys::errno() == libc::EINTR {
                continue;
            }
            break;
        }
        if n == 0 {
            break;
        }
        if !error_printed {
            error_printed = true;
            msg!(
                MESS_ERROR,
                "Compressing program wrote following message to stderr when compressing log ",
                name,
                ":\n"
            );
        }
        // fprintf(stderr, "%s", buff)
        sys::write_all(2, crate::bytes::c_str_prefix(&buf[..n as usize]));
    }
    sys::close(pr);
    let (_, status) = sys::wait_any();
    sys::fsync(out_file);
    if !sys::exited_ok(status) {
        msg!(MESS_ERROR, "failed to compress log ", name, "\n");
        sys::close(in_file);
        sys::close(out_file);
        sys::unlink(&compressed);
        return 1;
    }
    sys::futimens_from(out_file, sb);
    sys::close(out_file);
    if shred_file(in_file, name, log) != 0 {
        sys::close(in_file);
        return 1;
    }
    sys::close(in_file);
    0
}

/// `mailLog()`.
fn mail_log(
    log: &LogInfo,
    log_file: &[u8],
    mail_comm: &[u8],
    uncompress: Option<&[u8]>,
    address: &[u8],
    subject: &[u8],
) -> i32 {
    let mut mail_input = open_logfile(log_file, log, false);
    if mail_input < 0 {
        msg!(MESS_ERROR, "failed to open ", log_file, " for mailing: ", sys::strerror(sys::errno()), "\n");
        return 1;
    }
    let mut uncompress_child = 0;
    if let Some(uncompress) = uncompress {
        let (pr, pw) = match sys::pipe() {
            Ok(p) => p,
            Err(e) => {
                msg!(MESS_ERROR, "error opening pipe for uncompress: ", sys::strerror(e), "\n");
                sys::close(mail_input);
                return 1;
            }
        };
        uncompress_child = sys::fork();
        if uncompress_child == -1 {
            msg!(MESS_ERROR, "cannot fork: ", sys::strerror(sys::errno()), "\n");
            for fd in [mail_input, pw, pr] {
                sys::close(fd);
            }
            return 1;
        }
        if uncompress_child == 0 {
            sys::close(pr);
            movefd(mail_input, 0);
            movefd(pw, 1);
            if switch_user_permanently(log) != 0 {
                sys::exit(1);
            }
            sys::execvp(uncompress, &[uncompress]);
            msg!(MESS_ERROR, "cannot execute uncompress command: ", sys::strerror(sys::errno()), "\n");
            sys::exit(1);
        }
        sys::close(mail_input);
        mail_input = pr;
        sys::close(pw);
    }
    let mail_child = sys::fork();
    if mail_child == -1 {
        msg!(MESS_ERROR, "cannot fork: ", sys::strerror(sys::errno()), "\n");
        sys::close(mail_input);
        return 1;
    }
    if mail_child == 0 {
        movefd(mail_input, 0);
        sys::close(1);
        // The mail command runs as root.
        if log.has(LOG_FLAG_SU) && switch_user_back_permanently() != 0 {
            sys::exit(1);
        }
        sys::execvp(mail_comm, &[mail_comm, b"-s", subject, address]);
        msg!(MESS_ERROR, "cannot execute mail command: ", sys::strerror(sys::errno()), "\n");
        sys::exit(1);
    }
    sys::close(mail_input);
    let mut rc = 0;
    if !sys::exited_ok(sys::waitpid(mail_child)) {
        msg!(MESS_ERROR, "mail command failed for ", log_file, "\n");
        rc = 1;
    }
    if uncompress.is_some() && !sys::exited_ok(sys::waitpid(uncompress_child)) {
        msg!(MESS_ERROR, "uncompress command failed mailing ", log_file, "\n");
        rc = 1;
    }
    rc
}

/// `mailLogWrapper()`.
fn mail_log_wrapper(mail_filename: &[u8], log_num: usize, log: &LogInfo) -> i32 {
    // Uncompress compressed logs before mailing them.
    let mut uncompress = if log.has(LOG_FLAG_COMPRESS) { log.uncompress_prog.as_deref() } else { None };
    let mut subject = mail_filename;
    if log.has(LOG_FLAG_MAILFIRST) {
        if log.has(LOG_FLAG_DELAYCOMPRESS) {
            // The log being mailed isn't compressed yet.
            uncompress = None;
        }
        if uncompress.is_some() {
            subject = &log.files[log_num];
        }
    }
    let address = log.log_address.clone().unwrap_or_default();
    mail_log(log, mail_filename, &mail_command(), uncompress, &address, subject)
}

/// `is_probably_sparse()`.
fn is_probably_sparse(sb: &sys::Stat) -> bool {
    sys::s_isreg(sb.st_mode) && sb.st_blksize != 0 && sb.st_blocks < sb.st_size / sb.st_blksize
}

/// `sparse_copy()`: 1 on success, 0 after an error message.
fn sparse_copy(src: sys::Fd, dst: sys::Fd, sb: &sys::Stat, save_log: &[u8], curr_log: &[u8]) -> i32 {
    let make_holes = is_probably_sparse(sb);
    let mut last_write_made_hole = false;
    let mut total: i64 = 0;
    let mut buf = vec![0u8; BUFSIZ];
    loop {
        let n = sys::read(src, &mut buf);
        if n < 0 {
            if sys::errno() == libc::EINTR {
                continue;
            }
            msg!(MESS_ERROR, "error reading ", curr_log, ": ", sys::strerror(sys::errno()), "\n");
            return 0;
        }
        if n == 0 {
            break;
        }
        let chunk = &buf[..n as usize];
        total += n as i64;
        let mut make_hole = false;
        if make_holes && chunk.iter().all(|&b| b == 0) {
            make_hole = true;
            if sys::lseek_cur(dst, n as i64) < 0 {
                msg!(MESS_ERROR, "error seeking ", save_log, ": ", sys::strerror(sys::errno()), "\n");
                return 0;
            }
        }
        if !make_hole {
            // full_write()
            let mut done = 0;
            while done < chunk.len() {
                let w = sys::write(dst, &chunk[done..]);
                if w < 0 && sys::errno() == libc::EINTR {
                    continue;
                }
                if w <= 0 {
                    break;
                }
                done += w as usize;
            }
            if done != chunk.len() {
                msg!(MESS_ERROR, "error writing to ", save_log, ": ", sys::strerror(sys::errno()), "\n");
                return 0;
            }
        }
        last_write_made_hole = make_hole;
    }
    if last_write_made_hole && sys::ftruncate(dst, total) < 0 {
        msg!(MESS_ERROR, "error ftruncate ", save_log, ": ", sys::strerror(sys::errno()), "\n");
        return 0;
    }
    1
}

/// `copyTruncate()`.
fn copy_truncate(curr_log: &[u8], save_log: &[u8], sb: &sys::Stat, log: &LogInfo, skip_copy: bool) -> i32 {
    msg!(MESS_DEBUG, if skip_copy { "skip " } else { "" }, "copying ", curr_log, " to ", save_log, "\n");
    let mut fdcurr = -1;
    let mut fdsave = -1;
    let close_all = |a: sys::Fd, b: sys::Fd| {
        if a >= 0 {
            sys::close(a);
        }
        if b >= 0 {
            sys::close(b);
        }
    };
    if !debug() {
        // Reading is enough for `copy`, not for `copytruncate`.
        let read_only = log.has(LOG_FLAG_COPY) && !log.has(LOG_FLAG_COPYTRUNCATE);
        fdcurr = open_logfile(curr_log, log, !read_only);
        if fdcurr < 0 {
            msg!(MESS_ERROR, "error opening ", curr_log, ": ", sys::strerror(sys::errno()), "\n");
            return 1;
        }
        if !skip_copy {
            let Ok(acl) = get_acl_fd(fdcurr, curr_log) else {
                close_all(fdcurr, fdsave);
                return 1;
            };
            fdsave = create_output_file(save_log, libc::O_WRONLY, sb, acl.as_ref(), false);
            drop(acl);
            if fdsave < 0 {
                close_all(fdcurr, fdsave);
                return 1;
            }
            if sparse_copy(fdcurr, fdsave, sb, save_log, curr_log) != 1 {
                msg!(MESS_ERROR, "error copying ", curr_log, " to ", save_log, ": ", sys::strerror(sys::errno()), "\n");
                sys::unlink(save_log);
                close_all(fdcurr, fdsave);
                return 1;
            }
        }
    }
    if log.has(LOG_FLAG_COPYTRUNCATE) {
        msg!(MESS_DEBUG, "truncating ", curr_log, "\n");
        if !debug() {
            if fdsave >= 0 {
                sys::fsync(fdsave);
            }
            if sys::ftruncate(fdcurr, 0) != 0 {
                msg!(MESS_ERROR, "error truncating ", curr_log, ": ", sys::strerror(sys::errno()), "\n");
                close_all(fdcurr, fdsave);
                return 1;
            }
        }
    } else {
        msg!(MESS_DEBUG, "Not truncating ", curr_log, "\n");
    }
    close_all(fdcurr, fdsave);
    0
}

/// `mktimeFromDateOnly()`.
fn mktime_date_only(src: &sys::Tm) -> i64 {
    let mut t = *src;
    t.tm_hour = 0;
    t.tm_min = 0;
    t.tm_sec = 0;
    sys::mktime(&mut t)
}

/// `daysElapsed()`.
fn days_elapsed(now: &sys::Tm, last: &sys::Tm) -> i64 {
    let diff = (mktime_date_only(now) - mktime_date_only(last)) as f64;
    (diff as i64) / DAY_SECONDS
}

fn fmt_tm(t: &sys::Tm) -> String {
    format!("{}-{:02}-{:02} {:02}:{:02}", 1900 + t.tm_year, 1 + t.tm_mon, t.tm_mday, t.tm_hour, t.tm_min)
}

/// `findNeedRotating()`: 0, or 1 for an error.
#[allow(clippy::if_same_then_else)] // kept as upstream's chain of checks
fn find_need_rotating(log: &LogInfo, log_num: usize, force: bool, states: &mut States) -> i32 {
    let file = &log.files[log_num];
    msg!(MESS_DEBUG, "considering log ", file, "\n");
    let mut now = sys::localtime(now_secs());

    // Does the log's directory have safe permissions?
    if !log.has(LOG_FLAG_SU) && sys::getuid() == ROOT_UID {
        let ld = sys::dirname_of(file);
        match sys::stat(&ld) {
            Err(e) => {
                // A missing directory isn't an error with missingok.
                if e != libc::ENOENT || !log.has(LOG_FLAG_MISSINGOK) {
                    msg!(MESS_ERROR, "stat of ", ld, " failed: ", sys::strerror(e), "\n");
                    return 1;
                }
                return 0;
            }
            Ok(sb) => {
                if (sb.st_gid != 0 && sb.st_mode & libc::S_IWGRP != 0) || sb.st_mode & libc::S_IWOTH != 0 {
                    msg!(
                        MESS_ERROR,
                        "skipping \"",
                        file,
                        "\" because parent directory has insecure permissions (It's world writable or writable \
                         by group which is not \"root\") Set \"su\" directive in config file to tell logrotate \
                         which user/group should be used for rotation.\n"
                    );
                    return 1;
                }
            }
        }
    }

    let sb = match sys::lstat(file) {
        Ok(sb) => sb,
        Err(e) => {
            if log.has(LOG_FLAG_MISSINGOK) && e == libc::ENOENT {
                msg!(MESS_DEBUG, "  log ", file, " does not exist -- skipping\n");
                return 0;
            }
            msg!(MESS_ERROR, "stat of ", file, " failed: ", sys::strerror(e), "\n");
            return 1;
        }
    };
    let Some(state) = states.find(file) else { return 1 };
    state.do_rotate = false;
    state.sb = sb;
    state.is_used = true;

    if sb.st_mode & libc::S_IFMT == libc::S_IFLNK {
        msg!(
            MESS_DEBUG,
            "  log ",
            file,
            " is symbolic link. Rotation of symbolic links is not allowed to avoid security issues -- skipping.\n"
        );
        return 0;
    }
    if !log.has(LOG_FLAG_ALLOWHARDLINK) && sb.st_nlink != 1 {
        msg!(
            MESS_DEBUG,
            "  log ",
            file,
            format!(
                " has multiple ({}) hard links. Rotation of files with multiple hard links is not allowed for ",
                sb.st_nlink
            ),
            or_null(&log.pattern),
            " -- skipping.\n"
        );
        return 0;
    }

    msg!(MESS_DEBUG, "  Now: ", fmt_tm(&now), "\n");
    msg!(MESS_DEBUG, "  Last rotated at ", fmt_tm(&state.last_rotated), "\n");

    if force {
        state.do_rotate = true;
    } else if log.maxsize != 0 && sb.st_size > log.maxsize {
        state.do_rotate = true;
    } else if log.criterium == Criterium::Size {
        state.do_rotate = sb.st_size >= log.threshold;
        if !state.do_rotate {
            message(MESS_DEBUG, b"  log does not need rotating (log size is below the 'size' threshold)\n");
        }
    } else if (sys::mktime(&mut state.last_rotated) - sys::mktime(&mut now)) as f64 > (25 * 3600) as f64 {
        // 25 hours allows for DST changes and moving across time zones.
        msg!(MESS_ERROR, "log ", file, " last rotated in the future -- rotation forced\n");
        state.do_rotate = true;
    } else if state.last_rotated.tm_year != now.tm_year
        || state.last_rotated.tm_mon != now.tm_mon
        || state.last_rotated.tm_mday != now.tm_mday
        || state.last_rotated.tm_hour != now.tm_hour
    {
        let lr = state.last_rotated;
        let not_needed = |what: &str| {
            msg!(
                MESS_DEBUG,
                "  log does not need rotating (log has been rotated at ",
                fmt_tm(&lr),
                ", which is less than ",
                what,
                " ago)\n"
            );
        };
        match log.criterium {
            Criterium::Weekly => {
                let days = days_elapsed(&now, &lr);
                // 7+ days, or 1+ days and today is the selected weekday.
                state.do_rotate = days >= 7 || (days >= 1 && now.tm_wday as u32 == log.weekday);
                if !state.do_rotate {
                    not_needed("a week");
                }
            }
            Criterium::Hourly => {
                state.do_rotate = now.tm_hour != lr.tm_hour
                    || now.tm_mday != lr.tm_mday
                    || now.tm_mon != lr.tm_mon
                    || now.tm_year != lr.tm_year;
                if !state.do_rotate {
                    not_needed("an hour");
                }
            }
            Criterium::Days => {
                state.do_rotate = now.tm_mday != lr.tm_mday || now.tm_mon != lr.tm_mon || now.tm_year != lr.tm_year;
                if !state.do_rotate {
                    not_needed("a day");
                }
            }
            Criterium::Monthly => {
                state.do_rotate = now.tm_mon != lr.tm_mon || now.tm_year != lr.tm_year;
                if !state.do_rotate {
                    not_needed("a month");
                }
            }
            Criterium::Yearly => {
                state.do_rotate = now.tm_year != lr.tm_year;
                if !state.do_rotate {
                    not_needed("a year");
                }
            }
            Criterium::Size => state.do_rotate = false,
        }
        if log.minsize != 0 && sb.st_size < log.minsize {
            state.do_rotate = false;
            message(
                MESS_DEBUG,
                b"  log does not need rotating ('minsize' directive is used and the log size is smaller than the minsize value)\n",
            );
        }
        if log.rotate_min_age != 0
            && f64::from(log.rotate_min_age.wrapping_mul(DAY_SECONDS as i32)) >= (now_secs() - sb.st_mtime) as f64
        {
            state.do_rotate = false;
            message(
                MESS_DEBUG,
                b"  log does not need rotating ('minage' directive is used and the log age is smaller than the minage days)\n",
            );
        }
    } else if !state.do_rotate {
        message(MESS_DEBUG, b"  log does not need rotating (log has already been rotated)\n");
    }

    // notifempty overrides the other criteria.
    if state.do_rotate && !log.has(LOG_FLAG_IFEMPTY) && sb.st_size == 0 {
        state.do_rotate = false;
        message(MESS_DEBUG, b"  log does not need rotating (log is empty)\n");
    }
    if state.do_rotate {
        message(MESS_DEBUG, b"  log needs rotating\n");
    }
    0
}

/// `findLastRotated()`: the highest index of the rotated files, or -1.
fn find_last_rotated(names: &LogNames, fileext: &[u8], compext: &[u8]) -> i32 {
    let pattern = bcat!(names.dir_name, "/", names.base_name, ".*", fileext, compext);
    let (rc, paths) = sys::glob_rotate(&pattern);
    match rc {
        0 => {}
        sys::GLOB_NOMATCH => return 0,
        _ => return -1,
    }
    let prefix = names.dir_name.len() + 1 + names.base_name.len() + 1;
    let suffix = fileext.len() + compext.len();
    let mut last = 0;
    for p in &paths {
        if p.len() <= prefix + suffix {
            continue;
        }
        let (n, num) = sys::sscanf_d_c(&p[prefix..p.len() - suffix]);
        if n != 1 {
            continue;
        }
        if last < num {
            last = num;
        }
    }
    last
}

const DATEEXT_LEN: usize = 64;
const PATTERN_LEN: usize = DATEEXT_LEN * 2;

/// prerotateSingleLog()'s translation of `dateformat` into a strftime()/
/// strptime() format and a glob pattern: Ok((dformat, pattern)), or Err
/// after the "too long" message.
fn dateformat_pattern(dateformat: &[u8]) -> Result<(Vec<u8>, Vec<u8>), ()> {
    let too_long = || {
        msg!(MESS_ERROR, "Date format ", dateformat, " is too long\n");
    };
    let at = |i: usize| -> u8 { dateformat.get(i).copied().unwrap_or(0) };
    let mut dformat: Vec<u8> = Vec::new();
    let mut pattern: Vec<u8> = Vec::new();
    // `j` counts what C thinks the pattern's length is; strncat() can
    // truncate, but only where the length check fails anyway.
    let mut j: usize = 0;
    let mut d = 0;
    while at(d) == b' ' {
        d += 1;
    }
    let cat = |pattern: &mut Vec<u8>, s: &str| {
        let room = (PATTERN_LEN - 1).saturating_sub(pattern.len());
        pattern.extend_from_slice(&s.as_bytes()[..s.len().min(room)]);
    };
    while at(d) != 0 {
        if j >= PATTERN_LEN - 1 || dformat.len() >= PATTERN_LEN - 2 {
            too_long();
            return Err(());
        }
        if at(d) == b'%' {
            let add = match at(d + 1) {
                b'Y' => {
                    cat(&mut pattern, "[0-9][0-9]");
                    j += 10;
                    Some("[0-9][0-9]")
                }
                b'm' | b'd' | b'H' | b'M' | b'S' | b'V' => Some("[0-9][0-9]"),
                b's' => Some("[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]"),
                b'z' => Some("[-+][0-9][0-9][0-9][0-9]"),
                _ => None,
            };
            match add {
                Some(s) => {
                    cat(&mut pattern, s);
                    j += s.len();
                    if j >= PATTERN_LEN - 1 {
                        too_long();
                        return Err(());
                    }
                    dformat.push(b'%');
                    d += 1;
                    dformat.push(at(d));
                }
                None => {
                    // QUIRK: an unsupported conversion becomes "%%", so the
                    // date suffix holds it literally.
                    dformat.push(b'%');
                    dformat.push(b'%');
                    pattern.truncate(j);
                    pattern.resize(j, 0);
                    pattern.push(b'%');
                    j += 1;
                }
            }
        } else {
            dformat.push(at(d));
            pattern.resize(j, 0);
            pattern.push(at(d));
            j += 1;
        }
        d += 1;
    }
    msg!(MESS_DEBUG, "Converted '", dateformat, "' -> '", dformat, "'\n");
    Ok((dformat, pattern))
}

/// `prerotateSingleLog()`.
fn prerotate_single_log(log: &LogInfo, log_num: usize, state: &mut LogState, names: &mut LogNames) -> i32 {
    if !state.do_rotate {
        return 0;
    }
    let file = &log.files[log_num];
    let mut has_errors = 0;
    let mut rotate_count = if log.rotate_count != 0 { log.rotate_count } else { 1 };
    // Logs with rotate 0 are rotated once, then removed, so scripts and
    // mail work.
    msg!(MESS_DEBUG, "rotating log ", file, format!(", log->rotateCount is {}\n", log.rotate_count));

    let mut compext: &[u8] = b"";
    if log.has(LOG_FLAG_COMPRESS) {
        match &log.compress_ext {
            None => {
                msg!(MESS_ERROR, "log ", file, ": compression enabled, but compression extension is not set\n");
                return 1;
            }
            Some(e) => compext = e,
        }
    }
    let mut now = sys::localtime(now_secs());
    state.last_rotated = now;

    let ld = sys::dirname_of(file);
    names.dir_name = match &log.old_dir {
        Some(d) if d.first() != Some(&b'/') => bcat!(ld, "/", d),
        Some(d) => d.clone(),
        None => ld,
    };
    names.base_name = sys::basename(file);

    let mut fileext: &[u8] = b"";
    if let Some(ext) = &log.addextension {
        if names.base_name.ends_with(ext) {
            names.base_name.truncate(names.base_name.len() - ext.len());
        }
        fileext = ext;
    }
    if let Some(ext) = &log.extension
        && names.base_name.ends_with(ext)
    {
        fileext = ext;
        names.base_name.truncate(names.base_name.len() - ext.len());
    }

    // Yesterday's date, or an hour ago?
    if log.has(LOG_FLAG_DATEYESTERDAY) {
        now.tm_hour = 12; // noon, to stay clear of DST changes
        now.tm_mday -= 1;
        sys::mktime(&mut now);
    }
    if log.has(LOG_FLAG_DATEHOURAGO) {
        now.tm_hour -= 1;
        sys::mktime(&mut now);
    }

    // The glob pattern for the date format.
    let (dformat, dext_pattern, final_dformat): (Vec<u8>, Vec<u8>, Vec<u8>) = match &log.dateformat {
        Some(df) => {
            let Ok((dformat, pattern)) = dateformat_pattern(df) else { return 1 };
            let f = dformat.clone();
            (dformat, pattern, f)
        }
        None if log.criterium == Criterium::Hourly => {
            (Vec::new(), b"-[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]".to_vec(), b"-%Y%m%d%H".to_vec())
        }
        None => (Vec::new(), b"-[0-9][0-9][0-9][0-9][0-9][0-9][0-9][0-9]".to_vec(), b"-%Y%m%d".to_vec()),
    };
    let dext_pattern = crate::bytes::c_str_prefix(&dext_pattern).to_vec();
    let dext_str = sys::strftime_n(&final_dformat, &now, DATEEXT_LEN);
    if dext_str.is_empty() {
        msg!(MESS_ERROR, "failed to apply date format '", final_dformat, "'\n");
        return 1;
    }
    msg!(MESS_DEBUG, "dateext suffix '", dext_str, "'\n");
    msg!(MESS_DEBUG, "glob pattern '", dext_pattern, "'\n");

    let prefix_len = names.dir_name.len() + 1 + names.base_name.len();

    // First compress the previous log, if delayed.
    if log.has(LOG_FLAG_COMPRESS) && log.has(LOG_FLAG_DELAYCOMPRESS) {
        if log.has(LOG_FLAG_DATEEXT) {
            let pattern = bcat!(names.dir_name, "/", names.base_name, dext_pattern, fileext);
            let (rc, mut paths) = sys::glob_rotate(&pattern);
            if rc == 0 && !paths.is_empty() {
                sys::sort_glob_result(&mut paths, prefix_len, &dformat);
                for old in &paths {
                    if has_errors != 0 {
                        break;
                    }
                    match sys::stat(old) {
                        Err(libc::ENOENT) => msg!(MESS_DEBUG, "previous log ", old, " does not exist\n"),
                        Err(e) => msg!(MESS_ERROR, "cannot stat ", old, ": ", sys::strerror(e), "\n"),
                        Ok(sbprev) => has_errors = compress_log_file(old, log, &sbprev),
                    }
                }
            } else {
                message(MESS_DEBUG, b"glob finding logs to compress failed\n");
            }
        } else {
            let old = bcat!(names.dir_name, "/", names.base_name, ".", log.log_start.to_string(), fileext);
            match sys::stat(&old) {
                Err(libc::ENOENT) => msg!(MESS_DEBUG, "previous log ", old, " does not exist\n"),
                Err(e) => msg!(MESS_ERROR, "cannot stat ", old, ": ", sys::strerror(e), "\n"),
                Ok(sbprev) => has_errors = compress_log_file(&old, log, &sbprev),
            }
        }
    }

    let first_compext: &[u8] = if log.has(LOG_FLAG_DELAYCOMPRESS) { b"" } else { compext };
    if log.has(LOG_FLAG_DATEEXT) {
        // Find the old rotated (and compressed) logs with our pattern.
        let pattern = bcat!(names.dir_name, "/", names.base_name, dext_pattern, fileext, compext);
        let (rc, mut paths) = sys::glob_rotate(&pattern);
        if rc == 0 {
            // Remove the first (n - rotateCount) matches: the date is in
            // the name, so there's nothing to rename.  Each one found is
            // remembered; finding another mails and removes the previous.
            let mut mail_out: Option<usize> = None;
            sys::sort_glob_result(&mut paths, prefix_len, &dformat);
            let n = paths.len();
            let rc_usize = rotate_count as isize as usize;
            for (i, p) in paths.iter().enumerate() {
                let Ok(fst) = sys::stat(p) else { continue };
                let by_count = n >= rc_usize && i <= n.wrapping_sub(rc_usize);
                let by_age =
                    log.rotate_age > 0 && (now_secs() - fst.st_mtime) / DAY_SECONDS > i64::from(log.rotate_age);
                if by_count || by_age {
                    if let Some(m) = mail_out {
                        let mail_filename = &paths[m];
                        if has_errors == 0 && log.log_address.is_some() {
                            has_errors = mail_log_wrapper(mail_filename, log_num, log);
                        }
                        if has_errors == 0 {
                            msg!(MESS_DEBUG, "removing ", mail_filename, "\n");
                            has_errors = remove_log_file(mail_filename, log);
                        }
                    }
                    mail_out = Some(i);
                }
            }
            // The oldest one found, removed later.
            names.dispose_name = mail_out.map(|m| paths[m].clone());
        } else {
            message(MESS_DEBUG, b"glob finding old rotated logs failed\n");
            names.dispose_name = None;
        }
        // The most recently created (and compressed) rotated log.
        names.first_rotated = Some(bcat!(names.dir_name, "/", names.base_name, dext_str, fileext, first_compext));
    } else {
        if rotate_count == -1 {
            rotate_count = find_last_rotated(names, fileext, compext);
            if rotate_count < 0 {
                msg!(
                    MESS_ERROR,
                    "could not find last rotated file: ",
                    names.dir_name,
                    "/",
                    names.base_name,
                    ".*",
                    fileext,
                    compext,
                    "\n"
                );
                return 1;
            }
        }
        let numbered =
            |n: i32, ext: &[u8]| bcat!(names.dir_name, "/", names.base_name, ".", n.to_string(), fileext, ext);
        let mut old_name = numbered(log.log_start + rotate_count, compext);
        if log.rotate_count != -1 {
            names.dispose_name = Some(old_name.clone());
        }
        names.first_rotated = Some(numbered(log.log_start, first_compext));
        let mut i = rotate_count + log.log_start - 1;
        while i >= log.log_start && has_errors == 0 {
            let new_name = old_name;
            old_name = numbered(i, compext);
            let cur = i;
            i -= 1;
            // Remove files past maxage.
            if log.rotate_age != 0 {
                match sys::stat(&old_name) {
                    Err(e) => {
                        if e == libc::ENOENT {
                            msg!(MESS_DEBUG, "old log ", old_name, " does not exist\n");
                        } else {
                            msg!(MESS_ERROR, "cannot stat ", old_name, ": ", sys::strerror(e), "\n");
                            has_errors = 1;
                        }
                        continue;
                    }
                    Ok(fst) => {
                        if (now_secs() - fst.st_mtime) / DAY_SECONDS > i64::from(log.rotate_age) {
                            if has_errors == 0 && log.log_address.is_some() {
                                has_errors = mail_log_wrapper(&old_name, log_num, log);
                            }
                            if has_errors == 0 {
                                has_errors = remove_log_file(&old_name, log);
                            }
                            continue;
                        }
                    }
                }
            }
            msg!(
                MESS_DEBUG,
                "renaming ",
                old_name,
                " to ",
                new_name,
                format!(" (rotatecount {rotate_count}, logstart {}, i {cur}), \n", log.log_start)
            );
            if !debug() && sys::rename(&old_name, &new_name) != 0 {
                let e = sys::errno();
                if e == libc::ENOENT {
                    msg!(MESS_DEBUG, "old log ", old_name, " does not exist\n");
                } else {
                    msg!(MESS_ERROR, "error renaming ", old_name, " to ", new_name, ": ", sys::strerror(e), "\n");
                    has_errors = 1;
                }
            }
        }
    }

    if log.has(LOG_FLAG_DATEEXT) {
        let final_name = bcat!(names.dir_name, "/", names.base_name, dext_str, fileext);
        let dest = bcat!(final_name, compext);
        names.final_name = Some(final_name);
        if sys::stat(&dest).is_ok() {
            msg!(MESS_ERROR, "destination ", or_null(&names.first_rotated), " already exists, skipping rotation\n");
            has_errors = 1;
        }
    } else {
        // Note: the compression extension is *not* used here.
        names.final_name = Some(bcat!(names.dir_name, "/", names.base_name, ".", log.log_start.to_string(), fileext));
    }

    // A missing last rotation is fine.
    if let Some(d) = &names.dispose_name
        && !sys::exists(d)
    {
        msg!(MESS_DEBUG, "log ", d, " doesn't exist -- won't try to dispose of it\n");
        names.dispose_name = None;
    }
    has_errors
}

/// `rotateSingleLog()`.
fn rotate_single_log(log: &LogInfo, log_num: usize, state: &mut LogState, names: &mut LogNames) -> i32 {
    if !state.do_rotate {
        return 0;
    }
    let file = &log.files[log_num];
    let final_name = or_null(&names.final_name).to_vec();
    let mut has_errors = 0;
    let mut prev_acl: Option<sys::Acl> = None;
    let copying = log.has(LOG_FLAG_COPYTRUNCATE | LOG_FLAG_COPY);
    if !copying {
        // QUIRK: the log was checked (regular file, not a symlink) before
        // the prerotate script ran; whatever is at its path now is renamed.
        match sys::Acl::get_file(file) {
            Ok(a) => prev_acl = Some(a),
            Err(e) if is_acl_well_supported(e) => {
                msg!(MESS_ERROR, "getting file ACL ", file, ": ", sys::strerror(e), "\n");
                has_errors = 1;
            }
            Err(_) => {}
        }
        if log.has(LOG_FLAG_TMPFILENAME) {
            let tmp = bcat!(file, ".tmp");
            msg!(MESS_DEBUG, "renaming ", file, " to ", tmp, "\n");
            if !debug() && has_errors == 0 && sys::rename(file, &tmp) != 0 {
                msg!(MESS_ERROR, "failed to rename ", file, " to ", tmp, ": ", sys::strerror(sys::errno()), "\n");
                has_errors = 1;
            }
        } else {
            msg!(MESS_DEBUG, "renaming ", file, " to ", final_name, "\n");
            if !debug() && has_errors == 0 && sys::rename(file, &final_name) != 0 {
                msg!(
                    MESS_ERROR,
                    "failed to rename ",
                    file,
                    " to ",
                    final_name,
                    ": ",
                    sys::strerror(sys::errno()),
                    "\n"
                );
                has_errors = 1;
            }
        }
        if log.rotate_count == 0 {
            let ext: &[u8] = match &log.compress_ext {
                Some(e) if log.has(LOG_FLAG_COMPRESS) && !log.has(LOG_FLAG_DELAYCOMPRESS) => e,
                _ => b"",
            };
            let d = bcat!(final_name, ext);
            msg!(MESS_DEBUG, "disposeName will be ", d, "\n");
            names.dispose_name = Some(d);
        }
    }

    if has_errors == 0 && log.has(LOG_FLAG_CREATE) && !copying {
        let mut sb = sys::stat_zero();
        sb.st_uid = if log.create_uid == NO_UID { state.sb.st_uid } else { log.create_uid };
        sb.st_gid = if log.create_gid == NO_GID { state.sb.st_gid } else { log.create_gid };
        let have_create_mode = log.create_mode != NO_MODE;
        sb.st_mode = if have_create_mode { log.create_mode } else { state.sb.st_mode & 0o777 };
        msg!(
            MESS_DEBUG,
            "creating new ",
            file,
            format!(" mode = 0{:o} uid = {} gid = {}\n", sb.st_mode, sb.st_uid as i32, sb.st_gid as i32)
        );
        if !debug() && has_errors == 0 {
            let fd = create_output_file(file, libc::O_RDWR, &sb, prev_acl.as_ref(), have_create_mode);
            prev_acl = None;
            if fd < 0 {
                has_errors = 1;
            } else {
                sys::close(fd);
            }
        }
    }
    drop(prev_acl);

    if has_errors == 0 && copying && !log.has(LOG_FLAG_TMPFILENAME) {
        let sb = state.sb;
        has_errors = copy_truncate(file, &final_name, &sb, log, log.rotate_count == 0 && log.log_address.is_none());
    }
    has_errors
}

/// `postrotateSingleLog()`.
fn postrotate_single_log(log: &LogInfo, log_num: usize, state: &LogState, names: &LogNames) -> i32 {
    if !state.do_rotate {
        return 0;
    }
    let file = &log.files[log_num];
    let final_name = or_null(&names.final_name).to_vec();
    let mut has_errors = 0;
    if log.has(LOG_FLAG_TMPFILENAME) {
        let tmp = bcat!(file, ".tmp");
        has_errors = copy_truncate(&tmp, &final_name, &state.sb, log, false);
        msg!(MESS_DEBUG, "removing tmp log ", tmp, "\n");
        if !debug() && has_errors == 0 {
            sys::unlink(&tmp);
        }
    }
    if has_errors == 0 && log.has(LOG_FLAG_COMPRESS) && !log.has(LOG_FLAG_DELAYCOMPRESS) {
        // Was copying skipped in rotateSingleLog() -> copyTruncate()?
        let skipped_copy = log.has(LOG_FLAG_COPYTRUNCATE | LOG_FLAG_COPY)
            && !log.has(LOG_FLAG_TMPFILENAME)
            && log.rotate_count == 0
            && log.log_address.is_none();
        if !skipped_copy {
            has_errors = compress_log_file(&final_name, log, &state.sb);
        }
    }
    if has_errors == 0 && log.log_address.is_some() {
        let mail_filename =
            if log.has(LOG_FLAG_MAILFIRST) { names.first_rotated.as_ref() } else { names.dispose_name.as_ref() };
        if let Some(m) = mail_filename {
            has_errors = mail_log_wrapper(m, log_num, log);
        }
    }
    if has_errors == 0
        && let Some(d) = &names.dispose_name
    {
        has_errors = remove_log_file(d, log);
    }
    has_errors
}

/// `rotateLogSet()`: the set's error status.
#[allow(clippy::needless_range_loop)] // indexes as upstream does
pub fn rotate_log_set(log: &LogInfo, force: bool, states: &mut States) -> i32 {
    // QUIRK: a definition whose include failed has lost its pattern, and
    // printf prints a NULL %s as "(null)".
    let pattern = or_null(&log.pattern);
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
    let n = log.files.len();
    if n == 0 {
        message(MESS_DEBUG, b"No logs found. Rotation not needed.\n");
        return 0;
    }

    let su = log.has(LOG_FLAG_SU);
    let shared = log.has(LOG_FLAG_SHAREDSCRIPTS);
    let mut has_errors = 0;
    let mut log_has_errors = vec![0i32; n];
    let mut num_rotated = 0;
    if su && switch_user(log.su_uid, log.su_gid) != 0 {
        return 1;
    }
    for i in 0..n {
        log_has_errors[i] = find_need_rotating(log, i, force, states);
        has_errors |= log_has_errors[i];
        // QUIRK: this findState() creates a state for a log that doesn't
        // exist, so missing logs get state file entries dated now.
        if states.find(&log.files[i]).is_some_and(|s| s.do_rotate) {
            num_rotated += 1;
        }
    }
    if let Some(first) = &log.first {
        if num_rotated == 0 {
            message(MESS_DEBUG, b"not running first action script, since no logs will be rotated\n");
        } else {
            message(MESS_DEBUG, b"running first action script\n");
            if run_script(log, log.pattern.as_deref(), None, first) != 0 {
                msg!(MESS_ERROR, "error running first action script for ", pattern, "\n");
                has_errors = 1;
                if su && switch_user_back() != 0 {
                    return 1;
                }
                // firstaction failed: that affects every log in the set.
                return has_errors;
            }
        }
    }

    let mut names: Vec<LogNames> = (0..n).map(|_| LogNames::default()).collect();
    // The files each pass of the outer loop handles.
    let group = |j: usize| if shared { j..n } else { j..j + 1 };
    let skip = |e: i32, all: i32| (e != 0 && !shared) || (all != 0 && shared);
    let passes = if shared { 1 } else { n };
    for j in 0..passes {
        let do_rotate_j = states.find(&log.files[j]).is_some_and(|s| s.do_rotate);
        if let Some(pre) = &log.pre
            && !((!shared && (log_has_errors[j] != 0 || !do_rotate_j)) || (has_errors != 0 && shared))
        {
            if num_rotated == 0 {
                message(MESS_DEBUG, b"not running prerotate script, since no logs will be rotated\n");
            } else {
                message(MESS_DEBUG, b"running prerotate script\n");
                let arg = if shared { log.pattern.as_deref() } else { Some(log.files[j].as_slice()) };
                if run_script(log, arg, None, pre) != 0 {
                    if shared {
                        msg!(MESS_ERROR, "error running shared prerotate script for '", pattern, "'\n");
                    } else {
                        msg!(
                            MESS_ERROR,
                            "error running non-shared prerotate script for ",
                            log.files[j],
                            " of '",
                            pattern,
                            "'\n"
                        );
                    }
                    log_has_errors[j] = 1;
                    has_errors = 1;
                }
            }
        }
        for i in group(j) {
            if !skip(log_has_errors[i], has_errors) {
                let st = states.find(&log.files[i]).expect("state");
                log_has_errors[i] |= prerotate_single_log(log, i, st, &mut names[i]);
                has_errors |= log_has_errors[i];
            }
        }
        for i in group(j) {
            if !skip(log_has_errors[i], has_errors) {
                let st = states.find(&log.files[i]).expect("state");
                log_has_errors[i] |= rotate_single_log(log, i, st, &mut names[i]);
                has_errors |= log_has_errors[i];
            }
        }
        let do_rotate_j = states.find(&log.files[j]).is_some_and(|s| s.do_rotate);
        if let Some(post) = &log.post
            && !((!shared && (log_has_errors[j] != 0 || !do_rotate_j)) || (has_errors != 0 && shared))
        {
            if num_rotated == 0 {
                message(MESS_DEBUG, b"not running postrotate script, since no logs were rotated\n");
            } else {
                let logfn = if shared { log.pattern.as_deref() } else { Some(log.files[j].as_slice()) };
                // A rotated file name makes sense only for unshared scripts.
                let logrotfn = if shared { None } else { names[j].final_name.as_deref() };
                message(MESS_DEBUG, b"running postrotate script\n");
                if run_script(log, logfn, logrotfn, post) != 0 {
                    if shared {
                        msg!(MESS_ERROR, "error running shared postrotate script for '", pattern, "'\n");
                    } else {
                        msg!(
                            MESS_ERROR,
                            "error running non-shared postrotate script for ",
                            log.files[j],
                            " of '",
                            pattern,
                            "'\n"
                        );
                    }
                    log_has_errors[j] = 1;
                    has_errors = 1;
                }
            }
        }
        for i in group(j) {
            if !skip(log_has_errors[i], has_errors) {
                let st = states.find(&log.files[i]).expect("state").clone();
                log_has_errors[i] |= postrotate_single_log(log, i, &st, &names[i]);
                has_errors |= log_has_errors[i];
            }
        }
    }

    if let Some(last) = &log.last {
        if num_rotated == 0 {
            message(MESS_DEBUG, b"not running last action script, since no logs will be rotated\n");
        } else {
            message(MESS_DEBUG, b"running last action script\n");
            if run_script(log, log.pattern.as_deref(), None, last) != 0 {
                msg!(MESS_ERROR, "error running last action script for ", pattern, "\n");
                has_errors = 1;
            }
        }
    }
    if su && switch_user_back() != 0 {
        return 1;
    }
    has_errors
}
