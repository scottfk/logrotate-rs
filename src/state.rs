//! Port of logrotate.c's state file code: the hash table of states,
//! `readState()`, `lockState()` and `writeState()`.

use crate::bcat;
use crate::log::{MESS_DEBUG, MESS_ERROR, MESS_WARN, message};
use crate::msg;
use crate::rotate::{self, create_output_file};
use crate::sys;

/// `STATEFILE_BUFFER_SIZE`: `2 * PATH_MAX + 16`.
const STATEFILE_BUFFER_SIZE: usize = 2 * 4096 + 16;
const HASH_SIZE_MIN: u64 = 64;
const HASH_SIZE_MAX: u64 = 8192;
const HASH_CONST: u32 = 13;
/// Seconds in a solar year.
const SECONDS_IN_YEAR: f64 = 31556926.0;

/// `struct logState`.
#[derive(Debug, Clone)]
pub struct LogState {
    pub fn_: Vec<u8>,
    /// Only tm_hour, tm_mday, tm_mon and tm_year are meaningful.
    pub last_rotated: sys::Tm,
    pub sb: sys::Stat,
    pub do_rotate: bool,
    /// There is a real log file for this state.
    pub is_used: bool,
}

/// The `states` hash table: buckets of lists, newest entry first.
#[derive(Debug, Default)]
pub struct States {
    buckets: Vec<Vec<LogState>>,
}

impl States {
    /// `allocateHash()`.
    fn allocate(&mut self, hs: u64) {
        let hs = hs.clamp(HASH_SIZE_MIN, HASH_SIZE_MAX);
        msg!(MESS_DEBUG, format!("Allocating hash table for state file, size {hs} entries\n"));
        self.buckets = vec![Vec::new(); hs as usize];
    }

    /// `hashIndex()`.
    fn hash_index(&self, fn_: &[u8]) -> Option<usize> {
        if self.buckets.is_empty() {
            return None;
        }
        let mut hash: u32 = 0;
        for &b in crate::bytes::c_str_prefix(fn_) {
            hash = hash.wrapping_mul(HASH_CONST).wrapping_add(u32::from(b));
        }
        Some((hash % self.buckets.len() as u32) as usize)
    }

    /// `findState()`: the state for `fn_`, created if new.
    pub fn find(&mut self, fn_: &[u8]) -> Option<&mut LogState> {
        let i = self.hash_index(fn_)?;
        let pos = self.buckets[i].iter().position(|p| p.fn_ == fn_);
        let pos = match pos {
            Some(p) => p,
            None => {
                self.buckets[i].insert(0, new_state(fn_));
                0
            }
        };
        Some(&mut self.buckets[i][pos])
    }
}

/// `newState()`.
fn new_state(fn_: &[u8]) -> LogState {
    message(MESS_DEBUG, b"Creating new state\n");
    let now = sys::localtime(rotate::now_secs());
    let mut lr = sys::tm_zero();
    lr.tm_hour = now.tm_hour;
    lr.tm_mday = now.tm_mday;
    lr.tm_mon = now.tm_mon;
    lr.tm_year = now.tm_year;
    lr.tm_isdst = now.tm_isdst;
    let t = sys::mktime(&mut lr);
    let sb = sys::stat_zero();
    LogState { fn_: fn_.to_vec(), last_rotated: sys::localtime(t), sb, do_rotate: false, is_used: false }
}

/// `unescape()`: `\n` and `\\` in a state file name.
fn unescape(arg: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(arg.len());
    let mut i = 0;
    while i < arg.len() {
        if arg[i] == b'\\' {
            match arg.get(i + 1) {
                Some(b'n') => {
                    out.push(b'\n');
                    i += 2;
                    continue;
                }
                Some(b'\\') => {
                    out.push(b'\\');
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }
        out.push(arg[i]);
        i += 1;
    }
    out
}

/// `fgets(buf, n, f)` over the file's contents: the next chunk, up to and
/// including a newline, or `n - 1` bytes.
fn fgets<'a>(data: &'a [u8], pos: &mut usize, n: usize) -> Option<&'a [u8]> {
    if *pos >= data.len() {
        return None;
    }
    let rest = &data[*pos..];
    let max = n - 1;
    let len = match rest.iter().take(max).position(|&b| b == b'\n') {
        Some(i) => i + 1,
        None => rest.len().min(max),
    };
    *pos += len;
    Some(&rest[..len])
}

/// `readState()`: 0, or 1 after an error message.
pub fn read_state(states: &mut States, state_file: &[u8]) -> i32 {
    msg!(MESS_DEBUG, "Reading state from file: ", state_file, "\n");
    let mut rc = 0;
    let mut size: u64 = 0;
    let fd = sys::open(state_file, libc::O_RDONLY, 0);
    if fd == -1 {
        let e = sys::errno();
        // In debug mode the state file might not exist: lockState() isn't
        // called.
        if !rotate::debug() {
            msg!(MESS_ERROR, "error opening state file ", state_file, ": ", sys::strerror(e), "\n");
            rc = 1;
        } else if e == libc::ENOENT {
            msg!(MESS_DEBUG, "state file ", state_file, " does not exist\n");
        } else {
            msg!(
                MESS_ERROR,
                "error opening state file ",
                state_file,
                "; assuming empty state: ",
                sys::strerror(e),
                "\n"
            );
        }
    } else {
        match sys::fstat(fd) {
            Ok(sb) => size = sb.st_size as u64,
            Err(e) => {
                msg!(MESS_ERROR, "error stat()ing state file ", state_file, ": ", sys::strerror(e), "\n");
                rc = 1;
            }
        }
    }
    states.allocate(size / 80 / 200);
    if rc != 0 || size == 0 {
        if fd != -1 {
            sys::close(fd);
        }
        return rc;
    }
    let data = match sys::read_all(fd) {
        Ok(d) => d,
        Err(e) => {
            msg!(MESS_ERROR, "error opening state file ", state_file, ": ", sys::strerror(e), "\n");
            sys::close(fd);
            return 1;
        }
    };
    sys::close(fd);
    let n = STATEFILE_BUFFER_SIZE - 1;
    let mut pos = 0;
    let Some(top) = fgets(&data, &mut pos, n) else {
        msg!(MESS_ERROR, "error reading top line of ", state_file, "\n");
        return 1;
    };
    let top = crate::bytes::c_str_prefix(top);
    if top != b"logrotate state -- version 1\n" && top != b"logrotate state -- version 2\n" {
        msg!(MESS_ERROR, "bad top line in state file ", state_file, "\n");
        return 1;
    }
    let mut line = 1;
    while let Some(chunk) = fgets(&data, &mut pos, n) {
        let buf = crate::bytes::c_str_prefix(chunk);
        let i = buf.len();
        line += 1;
        if i == 0 {
            msg!(MESS_ERROR, format!("line {line} not parsable in state file "), state_file, "\n");
            return 1;
        }
        if buf[i - 1] != b'\n' {
            msg!(MESS_ERROR, format!("line {line} too long in state file "), state_file, "\n");
            return 1;
        }
        let buf = &buf[..i - 1];
        if i == 1 {
            continue;
        }
        let argv = match sys::popt_parse_argv_string(buf) {
            Ok(a) if a.len() == 2 => Some(a),
            _ => None,
        };
        let parsed = argv.as_ref().map(|a| sys::sscanf_state_date(&a[1]));
        let (argv, [year, month, day, hour, minute, second]) = match (argv, parsed) {
            (Some(a), Some((count, v))) if count >= 3 => (a, v),
            _ => {
                msg!(MESS_ERROR, format!("bad line {line} in state file "), state_file, "\n");
                return 1;
            }
        };
        let name = &argv[0];
        let bad = |what: &str, v: i32| {
            msg!(MESS_ERROR, format!("bad {what} {v} for file "), name, " in state file ", state_file, "\n");
        };
        // Hacks to hide earlier bugs: year 1900 and day 0 are accepted.
        if year != 1900 && !(1970..=2100).contains(&year) {
            bad("year", year);
            return 1;
        }
        if !(1..=12).contains(&month) {
            bad("month", month);
            return 1;
        }
        if !(0..=31).contains(&day) {
            bad("day", day);
            return 1;
        }
        if !(0..=23).contains(&hour) {
            bad("hour", hour);
            return 1;
        }
        if !(0..=59).contains(&minute) {
            bad("minute", minute);
            return 1;
        }
        if !(0..=59).contains(&second) {
            bad("second", second);
            return 1;
        }
        let filename = unescape(name);
        let Some(st) = states.find(&filename) else { return 1 };
        let mut tm = sys::tm_zero();
        tm.tm_year = year - 1900;
        tm.tm_mon = month - 1;
        tm.tm_mday = day;
        tm.tm_hour = hour;
        tm.tm_min = minute;
        tm.tm_sec = second;
        tm.tm_isdst = -1;
        let t = sys::mktime(&mut tm);
        st.last_rotated = sys::localtime(t);
    }
    0
}

/// `lockState()`: 0, or 1 after an error message.  The lock's fd stays
/// open until the process ends.
pub fn lock_state(state_file: &[u8], skip_state_lock: bool, wait_for_state_lock: bool) -> i32 {
    if state_file == b"/dev/null" {
        return 0;
    }
    let mut fd = sys::open(state_file, libc::O_RDWR | libc::O_CLOEXEC, 0);
    if fd == -1 {
        let e = sys::errno();
        if e == libc::ENOENT {
            msg!(MESS_DEBUG, "Creating stub state file: ", state_file, "\n");
            fd = sys::open(
                state_file,
                libc::O_CREAT | libc::O_EXCL | libc::O_WRONLY,
                libc::S_IWUSR | libc::S_IRUSR | libc::S_IRGRP,
            );
            if fd == -1 {
                msg!(
                    MESS_ERROR,
                    "error creating stub state file ",
                    state_file,
                    ": ",
                    sys::strerror(sys::errno()),
                    "\n"
                );
                return 1;
            }
        } else {
            msg!(MESS_ERROR, "error opening state file ", state_file, ": ", sys::strerror(e), "\n");
            return 1;
        }
    }
    if skip_state_lock {
        msg!(MESS_DEBUG, "Skip locking state file ", state_file, "\n");
        sys::close(fd);
        return 0;
    }
    let sb = match sys::fstat(fd) {
        Ok(sb) => sb,
        Err(e) => {
            msg!(MESS_ERROR, "error stat()ing state file ", state_file, ": ", sys::strerror(e), "\n");
            sys::close(fd);
            return 1;
        }
    };
    if sb.st_mode & libc::S_IROTH != 0 {
        msg!(
            MESS_WARN,
            "state file ",
            state_file,
            " is world-readable and thus can be locked from other unprivileged users. Skipping lock acquisition...\n"
        );
        sys::close(fd);
        return 0;
    }
    let mut op = libc::LOCK_EX;
    if wait_for_state_lock {
        msg!(MESS_DEBUG, "waiting for lock on state file ", state_file, "\n");
    } else {
        op |= libc::LOCK_NB;
    }
    if sys::flock(fd, op) == -1 {
        let e = sys::errno();
        if e == libc::EWOULDBLOCK {
            msg!(
                MESS_ERROR,
                "state file ",
                state_file,
                " is already locked\nlogrotate does not support parallel execution on the same set of logfiles.\n"
            );
        } else {
            msg!(MESS_ERROR, "error acquiring lock on state file ", state_file, ": ", sys::strerror(e), "\n");
        }
        sys::close(fd);
        return 1;
    }
    msg!(MESS_DEBUG, "acquired lock on state file ", state_file, "\n");
    0
}

/// `writeState()`: 0, or nonzero after an error message.
pub fn write_state(states: &mut States, state_file: &[u8]) -> i32 {
    if state_file == b"/dev/null" {
        // Explicitly asked not to write the state file.
        return 0;
    }
    let fdcurr = sys::open(state_file, libc::O_RDONLY, 0);
    if fdcurr == -1 {
        // lockState() created a stub if the file did not exist.
        msg!(MESS_ERROR, "error opening state file ", state_file, ": ", sys::strerror(sys::errno()), "\n");
        return 1;
    }
    let mut sb = match sys::fstat(fdcurr) {
        Ok(sb) => sb,
        Err(e) => {
            msg!(MESS_ERROR, "error stating ", state_file, ": ", sys::strerror(e), "\n");
            sys::close(fdcurr);
            return 1;
        }
    };
    if !sys::s_isreg(sb.st_mode) {
        msg!(MESS_ERROR, "not writing state to ", state_file, " because it is not a regular file\n");
        sys::close(fdcurr);
        return 1;
    }
    let tmp = bcat!(state_file, ".tmp");
    if sys::unlink(&tmp) == -1 && sys::errno() != libc::ENOENT {
        msg!(MESS_ERROR, "error removing old temporary state file ", tmp, ": ", sys::strerror(sys::errno()), "\n");
        sys::close(fdcurr);
        return 1;
    }
    let prev_acl = match sys::Acl::get_fd(fdcurr) {
        Ok(a) => Some(a),
        Err(e) => {
            if e != libc::ENOTSUP && e != libc::EINVAL && e != libc::ENOSYS && e != libc::EBUSY {
                msg!(MESS_ERROR, "getting file ACL ", state_file, ": ", sys::strerror(e), "\n");
                sys::close(fdcurr);
                return 1;
            }
            None
        }
    };
    sys::close(fdcurr);
    let mut force_mode = false;
    if sb.st_mode & libc::S_IROTH != 0 {
        // Drop the world-readable bit, so others can't lock the file.
        sb.st_mode &= !libc::S_IROTH;
        force_mode = true;
    }
    let fdsave = create_output_file(&tmp, libc::O_RDWR, &sb, prev_acl.as_ref(), force_mode);
    drop(prev_acl);
    if fdsave < 0 {
        return 1;
    }

    let mut out: Vec<u8> = b"logrotate state -- version 2\n".to_vec();
    let now = rotate::now_secs();
    for bucket in &mut states.buckets {
        for p in bucket.iter_mut() {
            // Skip states unused for more than a year.
            let last_time = sys::mktime(&mut p.last_rotated);
            if !p.is_used && (now - last_time) as f64 > SECONDS_IN_YEAR {
                msg!(
                    MESS_DEBUG,
                    "Removing ",
                    p.fn_,
                    " from state file, because it does not exist and has not been rotated for one year\n"
                );
                continue;
            }
            out.push(b'"');
            for &c in crate::bytes::c_str_prefix(&p.fn_) {
                match c {
                    b'"' | b'\\' => out.extend_from_slice(&[b'\\', c]),
                    b'\n' => out.extend_from_slice(b"\\n"),
                    _ => out.push(c),
                }
            }
            out.push(b'"');
            let t = &p.last_rotated;
            out.extend_from_slice(
                format!(
                    " {}-{}-{}-{}:{}:{}\n",
                    t.tm_year + 1900,
                    t.tm_mon + 1,
                    t.tm_mday,
                    t.tm_hour,
                    t.tm_min,
                    t.tm_sec
                )
                .as_bytes(),
            );
        }
    }
    let ok = sys::write_all(fdsave, &out) && sys::fsync(fdsave) == 0;
    let err = sys::errno();
    sys::close(fdsave);
    if ok {
        if sys::rename(&tmp, state_file) != 0 {
            msg!(
                MESS_ERROR,
                "error renaming temp state file ",
                tmp,
                " to ",
                state_file,
                ": ",
                sys::strerror(sys::errno()),
                "\n"
            );
            sys::unlink(&tmp);
            return 1;
        }
        0
    } else {
        msg!(MESS_ERROR, "error creating temp state file ", tmp, ": ", sys::strerror(err), "\n");
        sys::unlink(&tmp);
        1
    }
}
