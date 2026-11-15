//! Thin wrappers over libc, popt and libacl.
//!
//! This is the only module with `unsafe` code.  Each wrapper mirrors one C
//! call in upstream logrotate, with the same flags.  Where upstream's
//! behavior depends on the C library (ctype and wide-character classes in
//! the current locale, `strto*`, `sscanf`, `glob`, `strcoll`, POSIX
//! `basename`/`dirname`, `mktime`), the C library is called rather than
//! imitated.

use std::ffi::{CStr, c_char, c_int, c_uint, c_ulong, c_void};

use crate::bytes::to_cstring;

pub type Fd = c_int;

// ---------------------------------------------------------------------------
// errno

/// The current `errno`.
pub fn errno() -> i32 {
    // SAFETY: __errno_location returns this thread's errno slot.
    unsafe { *libc::__errno_location() }
}

pub fn set_errno(e: i32) {
    // SAFETY: as above.
    unsafe { *libc::__errno_location() = e }
}

/// `strerror(e)`.
pub fn strerror(e: i32) -> Vec<u8> {
    // SAFETY: strerror returns a valid C string; copied out at once in this
    // single-threaded program.
    unsafe { CStr::from_ptr(libc::strerror(e)) }.to_bytes().to_vec()
}

// ---------------------------------------------------------------------------
// locale and character classes

/// `setlocale(LC_ALL, "")`.
pub fn setlocale_all() {
    // SAFETY: a valid category and a static empty string.
    unsafe { libc::setlocale(libc::LC_ALL, c"".as_ptr()) };
}

macro_rules! ctype {
    ($($name:ident),*) => {$(
        /// The C library's classification of the byte, as `(unsigned char)c`.
        pub fn $name(b: u8) -> bool {
            // SAFETY: any unsigned char value is a valid argument.
            unsafe { libc::$name(c_int::from(b)) != 0 }
        }
    )*};
}
ctype!(isalpha, isspace, isblank, isprint, isdigit);

unsafe extern "C" {
    fn mbrtowc(pwc: *mut libc::wchar_t, s: *const c_char, n: usize, ps: *mut c_void) -> usize;
    fn iswprint(wc: c_uint) -> c_int;
    fn iswblank(wc: c_uint) -> c_int;
    #[link_name = "secure_getenv"]
    fn c_secure_getenv(name: *const c_char) -> *mut c_char;
}

/// readPath()'s check: every character (as `mbrtowc` decodes it in the
/// current locale) is printable and not blank.
pub fn path_chars_ok(path: &[u8]) -> bool {
    let c = to_cstring(path);
    let bytes = c.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let mut wc: libc::wchar_t = 0;
        let rest = &bytes[i..];
        // SAFETY: rest points into c, which is NUL-terminated after rest.len()
        // bytes; a NULL state means mbrtowc's internal state, as in C.
        let len = unsafe { mbrtowc(&mut wc, rest.as_ptr().cast(), rest.len(), std::ptr::null_mut()) };
        if len == 0 {
            break;
        }
        // SAFETY: plain classification calls.
        let bad = len == usize::MAX
            || len == usize::MAX - 1
            || unsafe { iswprint(wc as c_uint) } == 0
            || unsafe { iswblank(wc as c_uint) } != 0;
        if bad {
            return false;
        }
        i += len;
    }
    true
}

// ---------------------------------------------------------------------------
// numbers

/// Result of a `strto*` call: the value, where conversion stopped, and errno.
#[derive(Debug, Clone, Copy)]
pub struct Strto<T> {
    pub value: T,
    pub end: usize,
    pub errno: i32,
}

macro_rules! strto {
    ($name:ident, $f:ident, $t:ty) => {
        /// The C library's function of the same name, on `s` up to its first NUL.
        pub fn $name(s: &[u8], base: c_int) -> Strto<$t> {
            let c = to_cstring(s);
            let mut end: *mut c_char = std::ptr::null_mut();
            set_errno(0);
            // SAFETY: c is NUL-terminated; end receives a pointer into it.
            let value = unsafe { libc::$f(c.as_ptr(), &mut end, base) };
            let e = errno();
            // SAFETY: end points into c's buffer.
            let off = unsafe { end.offset_from(c.as_ptr()) } as usize;
            Strto { value, end: off, errno: e }
        }
    };
}
strto!(strtoul, strtoul, c_ulong);
strto!(strtol, strtol, libc::c_long);
strto!(strtoull, strtoull, libc::c_ulonglong);

/// `sscanf(s, "%u%c", &u, &c)`: (count, u).
pub fn sscanf_u_c(s: &[u8]) -> (c_int, c_uint) {
    let c = to_cstring(s);
    let mut u: c_uint = 0;
    let mut ch: c_char = 0;
    // SAFETY: the format matches the two out-pointers' types.
    let n = unsafe { libc::sscanf(c.as_ptr(), c"%u%c".as_ptr(), &mut u as *mut c_uint, &mut ch as *mut c_char) };
    (n, u)
}

/// `sscanf(s, "%d-%d-%d-%d:%d:%d", ...)`: the count and the six fields
/// (unconverted fields keep their 0).
pub fn sscanf_state_date(s: &[u8]) -> (c_int, [c_int; 6]) {
    let c = to_cstring(s);
    let mut v = [0 as c_int; 6];
    let [a, b, d, e, f, g] = &mut v;
    // SAFETY: the format has six %d, matching six int out-pointers.
    let n = unsafe {
        libc::sscanf(
            c.as_ptr(),
            c"%d-%d-%d-%d:%d:%d".as_ptr(),
            a as *mut c_int,
            b as *mut c_int,
            d as *mut c_int,
            e as *mut c_int,
            f as *mut c_int,
            g as *mut c_int,
        )
    };
    (n, v)
}

// ---------------------------------------------------------------------------
// strings and paths

/// `strcoll()`, for sorting the files of an included directory.
pub fn strcoll(a: &[u8], b: &[u8]) -> std::cmp::Ordering {
    let (a, b) = (to_cstring(a), to_cstring(b));
    // SAFETY: both are NUL-terminated.
    let r = unsafe { libc::strcoll(a.as_ptr(), b.as_ptr()) };
    r.cmp(&0)
}

/// `fnmatch(pattern, name, FNM_PERIOD) == 0`.
pub fn fnmatch_period(pattern: &[u8], name: &[u8]) -> bool {
    let (p, n) = (to_cstring(pattern), to_cstring(name));
    // SAFETY: both are NUL-terminated.
    unsafe { libc::fnmatch(p.as_ptr(), n.as_ptr(), libc::FNM_PERIOD) == 0 }
}

unsafe extern "C" {
    fn __xpg_basename(path: *mut c_char) -> *mut c_char;
    fn dirname(path: *mut c_char) -> *mut c_char;
}

fn libgen(path: &[u8], f: unsafe extern "C" fn(*mut c_char) -> *mut c_char) -> Vec<u8> {
    let mut buf = to_cstring(path).into_bytes_with_nul();
    // SAFETY: buf is a writable NUL-terminated copy; the result points into
    // it or to a static string, and is copied before buf is dropped.
    unsafe {
        let r = f(buf.as_mut_ptr().cast());
        CStr::from_ptr(r).to_bytes().to_vec()
    }
}

/// POSIX `basename()` (config.c includes <libgen.h>).
pub fn basename(path: &[u8]) -> Vec<u8> {
    libgen(path, __xpg_basename)
}

/// POSIX `dirname()`.
pub fn dirname_of(path: &[u8]) -> Vec<u8> {
    libgen(path, dirname)
}

/// `secure_getenv(name)`.
pub fn secure_getenv(name: &CStr) -> Option<Vec<u8>> {
    // SAFETY: name is NUL-terminated; the result is copied at once.
    unsafe {
        let p = c_secure_getenv(name.as_ptr());
        (!p.is_null()).then(|| CStr::from_ptr(p).to_bytes().to_vec())
    }
}

// ---------------------------------------------------------------------------
// users and groups

pub fn getuid() -> u32 {
    // SAFETY: no preconditions.
    unsafe { libc::getuid() }
}
pub fn geteuid() -> u32 {
    // SAFETY: no preconditions.
    unsafe { libc::geteuid() }
}
pub fn getegid() -> u32 {
    // SAFETY: no preconditions.
    unsafe { libc::getegid() }
}
pub fn getpid() -> i32 {
    // SAFETY: no preconditions.
    unsafe { libc::getpid() }
}
/// `setegid()`; 0 or -1 (errno set).
pub fn setegid(g: u32) -> c_int {
    // SAFETY: no memory arguments.
    unsafe { libc::setegid(g) }
}
/// `seteuid()`; 0 or -1 (errno set).
pub fn seteuid(u: u32) -> c_int {
    // SAFETY: no memory arguments.
    unsafe { libc::seteuid(u) }
}

/// `getpwnam(name)->pw_uid`.
pub fn getpwnam_uid(name: &[u8]) -> Option<u32> {
    let c = to_cstring(name);
    // SAFETY: c is NUL-terminated; the entry is read at once.
    unsafe {
        let p = libc::getpwnam(c.as_ptr());
        (!p.is_null()).then(|| (*p).pw_uid)
    }
}

/// `getpwuid(uid) != NULL`.
pub fn getpwuid_exists(uid: u32) -> bool {
    // SAFETY: no memory arguments.
    unsafe { !libc::getpwuid(uid).is_null() }
}

/// `getpwuid(uid)->pw_dir`.
pub fn getpwuid_dir(uid: u32) -> Option<Vec<u8>> {
    // SAFETY: the entry is read at once.
    unsafe {
        let p = libc::getpwuid(uid);
        (!p.is_null()).then(|| CStr::from_ptr((*p).pw_dir).to_bytes().to_vec())
    }
}

/// `getgrnam(name)->gr_gid`.
pub fn getgrnam_gid(name: &[u8]) -> Option<u32> {
    let c = to_cstring(name);
    // SAFETY: c is NUL-terminated; the entry is read at once.
    unsafe {
        let p = libc::getgrnam(c.as_ptr());
        (!p.is_null()).then(|| (*p).gr_gid)
    }
}

/// `getgrgid(gid) != NULL`.
pub fn getgrgid_exists(gid: u32) -> bool {
    // SAFETY: no memory arguments.
    unsafe { !libc::getgrgid(gid).is_null() }
}

// ---------------------------------------------------------------------------
// files

pub type Stat = libc::stat;

/// `open(path, flags, mode)`; the fd or -1 (errno set).
pub fn open(path: &[u8], flags: c_int, mode: libc::mode_t) -> Fd {
    let c = to_cstring(path);
    // SAFETY: c is NUL-terminated.
    unsafe { libc::open(c.as_ptr(), flags, mode as c_uint) }
}

pub fn close(fd: Fd) {
    // SAFETY: closing an fd we own.
    unsafe { libc::close(fd) };
}

fn stat_with(f: impl FnOnce(*mut Stat) -> c_int) -> Result<Stat, i32> {
    let mut sb = std::mem::MaybeUninit::<Stat>::zeroed();
    if f(sb.as_mut_ptr()) == 0 {
        // SAFETY: the call succeeded and filled sb.
        Ok(unsafe { sb.assume_init() })
    } else {
        Err(errno())
    }
}

/// `stat()`; Err(errno).
pub fn stat(path: &[u8]) -> Result<Stat, i32> {
    let c = to_cstring(path);
    // SAFETY: c is NUL-terminated; sb is a valid out-pointer.
    stat_with(|sb| unsafe { libc::stat(c.as_ptr(), sb) })
}

/// `lstat()`; Err(errno).
pub fn lstat(path: &[u8]) -> Result<Stat, i32> {
    let c = to_cstring(path);
    // SAFETY: as for stat.
    stat_with(|sb| unsafe { libc::lstat(c.as_ptr(), sb) })
}

/// `fstat()`; Err(errno).
pub fn fstat(fd: Fd) -> Result<Stat, i32> {
    // SAFETY: sb is a valid out-pointer.
    stat_with(|sb| unsafe { libc::fstat(fd, sb) })
}

pub fn s_isreg(m: libc::mode_t) -> bool {
    m & libc::S_IFMT == libc::S_IFREG
}
pub fn s_isdir(m: libc::mode_t) -> bool {
    m & libc::S_IFMT == libc::S_IFDIR
}

/// `fcntl(fd, F_SETLK, {F_RDLCK, whole file})`; true on success.
pub fn read_lock(fd: Fd) -> bool {
    // SAFETY: flock is plain data; zeroed is a valid value.
    let mut fl: libc::flock = unsafe { std::mem::zeroed() };
    fl.l_type = libc::F_RDLCK as libc::c_short;
    fl.l_whence = libc::SEEK_SET as libc::c_short;
    // SAFETY: fl is a valid flock for F_SETLK.
    unsafe { libc::fcntl(fd, libc::F_SETLK, &fl) != -1 }
}

/// `flock(fd, op)`; 0 or -1 (errno set).
pub fn flock(fd: Fd, op: c_int) -> c_int {
    // SAFETY: no memory arguments.
    unsafe { libc::flock(fd, op) }
}

/// Read `len` bytes from the start of `fd` (config.c maps the file).
pub fn read_exact_len(fd: Fd, len: usize) -> Result<Vec<u8>, i32> {
    let mut buf = vec![0u8; len];
    let mut off = 0;
    while off < len {
        // SAFETY: the destination is within buf.
        let n = unsafe { libc::pread(fd, buf[off..].as_mut_ptr().cast(), len - off, off as libc::off_t) };
        if n < 0 {
            if errno() == libc::EINTR {
                continue;
            }
            return Err(errno());
        }
        if n == 0 {
            // The file shrank: mmap would show zeros (or fault); keep zeros.
            break;
        }
        off += n as usize;
    }
    Ok(buf)
}

/// Read all of `fd` from its current offset.
pub fn read_all(fd: Fd) -> Result<Vec<u8>, i32> {
    let mut out = Vec::new();
    let mut buf = [0u8; 8192];
    loop {
        // SAFETY: buf is valid for its length.
        let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
        if n < 0 {
            if errno() == libc::EINTR {
                continue;
            }
            return Err(errno());
        }
        if n == 0 {
            return Ok(out);
        }
        out.extend_from_slice(&buf[..n as usize]);
    }
}

/// `write()` everything; false on error (errno set).
pub fn write_all(fd: Fd, mut data: &[u8]) -> bool {
    while !data.is_empty() {
        // SAFETY: data is valid for its length.
        let n = unsafe { libc::write(fd, data.as_ptr().cast(), data.len()) };
        if n < 0 {
            if errno() == libc::EINTR {
                continue;
            }
            return false;
        }
        data = &data[n as usize..];
    }
    true
}

pub fn fsync(fd: Fd) -> c_int {
    // SAFETY: no memory arguments.
    unsafe { libc::fsync(fd) }
}

pub fn fchmod(fd: Fd, mode: libc::mode_t) -> c_int {
    // SAFETY: no memory arguments.
    unsafe { libc::fchmod(fd, mode) }
}

pub fn fchown(fd: Fd, uid: u32, gid: u32) -> c_int {
    // SAFETY: no memory arguments.
    unsafe { libc::fchown(fd, uid, gid) }
}

pub fn mkdir(path: &[u8], mode: libc::mode_t) -> c_int {
    let c = to_cstring(path);
    // SAFETY: c is NUL-terminated.
    unsafe { libc::mkdir(c.as_ptr(), mode) }
}

pub fn unlink(path: &[u8]) -> c_int {
    let c = to_cstring(path);
    // SAFETY: c is NUL-terminated.
    unsafe { libc::unlink(c.as_ptr()) }
}

pub fn rename(from: &[u8], to: &[u8]) -> c_int {
    let (a, b) = (to_cstring(from), to_cstring(to));
    // SAFETY: both are NUL-terminated.
    unsafe { libc::rename(a.as_ptr(), b.as_ptr()) }
}

pub fn chdir(path: &[u8]) -> c_int {
    let c = to_cstring(path);
    // SAFETY: c is NUL-terminated.
    unsafe { libc::chdir(c.as_ptr()) }
}

pub fn fchdir(fd: Fd) -> c_int {
    // SAFETY: no memory arguments.
    unsafe { libc::fchdir(fd) }
}

/// The names `readdir()` returns, in its order ("." and ".." included).
pub fn readdir_names(path: &[u8]) -> Result<Vec<Vec<u8>>, i32> {
    let c = to_cstring(path);
    // SAFETY: c is NUL-terminated; each dirent is copied before the next
    // readdir call, and the stream is closed at the end.
    unsafe {
        let d = libc::opendir(c.as_ptr());
        if d.is_null() {
            return Err(errno());
        }
        let mut names = Vec::new();
        loop {
            let ent = libc::readdir(d);
            if ent.is_null() {
                break;
            }
            names.push(CStr::from_ptr((*ent).d_name.as_ptr()).to_bytes().to_vec());
        }
        libc::closedir(d);
        Ok(names)
    }
}

// ---------------------------------------------------------------------------
// glob

/// The error callback config.c passes: it keeps going on ENOTDIR and ENOENT,
/// and otherwise records the error and aborts.
static GLOB_ERRNO: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

extern "C" fn config_globerr(_path: *const c_char, err: c_int) -> c_int {
    match err {
        libc::ENOTDIR | libc::ENOENT => 0,
        _ => {
            GLOB_ERRNO.store(err, std::sync::atomic::Ordering::Relaxed);
            1
        }
    }
}

/// config.c's `glob_errno`.
pub fn config_glob_errno() -> i32 {
    GLOB_ERRNO.load(std::sync::atomic::Ordering::Relaxed)
}

const GLOB_TILDE: c_int = 1 << 12;

/// `glob(pattern, GLOB_NOCHECK | GLOB_TILDE, globerr, &g)` with config.c's
/// error callback: (return code, paths).
pub fn glob_config(pattern: &[u8]) -> (c_int, Vec<Vec<u8>>) {
    let c = to_cstring(pattern);
    // SAFETY: glob_t is plain data; zeroed is its documented initial state.
    let mut g: libc::glob_t = unsafe { std::mem::zeroed() };
    // SAFETY: c is NUL-terminated, g is a valid glob_t, and the callback has
    // the signature glob expects.
    let rc = unsafe { libc::glob(c.as_ptr(), libc::GLOB_NOCHECK | GLOB_TILDE, Some(config_globerr), &mut g) };
    let mut out = Vec::new();
    if rc == 0 {
        for i in 0..g.gl_pathc {
            // SAFETY: gl_pathv holds gl_pathc valid C strings.
            out.push(unsafe { CStr::from_ptr(*g.gl_pathv.add(i)) }.to_bytes().to_vec());
        }
    }
    // SAFETY: g was filled by glob (or is still zeroed).
    unsafe { libc::globfree(&mut g) };
    (rc, out)
}

pub const GLOB_ABORTED: c_int = libc::GLOB_ABORTED;

// ---------------------------------------------------------------------------
// time

pub type Tm = libc::tm;

/// `time(NULL)`.
pub fn time_now() -> i64 {
    // SAFETY: a NULL out-pointer is allowed.
    unsafe { libc::time(std::ptr::null_mut()) }
}

pub fn tzset() {
    unsafe extern "C" {
        fn tzset();
    }
    // SAFETY: no arguments.
    unsafe { tzset() }
}

pub fn tm_zero() -> Tm {
    // SAFETY: struct tm is plain data (tm_zone NULL is fine).
    unsafe { std::mem::zeroed() }
}

/// `localtime_r(&t, &tm)`.
pub fn localtime(t: i64) -> Tm {
    let mut tm = tm_zero();
    let tt: libc::time_t = t;
    // SAFETY: both pointers are valid.
    unsafe { libc::localtime_r(&tt, &mut tm) };
    tm
}

/// `mktime(&tm)`, normalizing tm in place.
pub fn mktime(tm: &mut Tm) -> i64 {
    // SAFETY: tm is a valid struct tm.
    unsafe { libc::mktime(tm) }
}

/// `strftime(fmt, tm)`.
pub fn strftime(fmt: &CStr, tm: &Tm) -> Vec<u8> {
    let mut buf = [0u8; 256];
    // SAFETY: buf is valid for its length; fmt is NUL-terminated.
    let n = unsafe { libc::strftime(buf.as_mut_ptr().cast(), buf.len(), fmt.as_ptr(), tm) };
    buf[..n].to_vec()
}

// ---------------------------------------------------------------------------
// syslog

/// `openlog("logrotate", 0, LOG_USER)`.
pub fn openlog_logrotate() {
    // SAFETY: the ident is a static string, as syslog requires.
    unsafe { libc::openlog(c"logrotate".as_ptr(), 0, libc::LOG_USER) };
}

/// `syslog(priority, "%s", msg)`.
pub fn syslog(priority: c_int, msg: &[u8]) {
    let c = to_cstring(msg);
    // SAFETY: a "%s" format with one C string argument.
    unsafe { libc::syslog(priority, c"%s".as_ptr(), c.as_ptr()) };
}

// ---------------------------------------------------------------------------
// ACLs (libacl)

#[allow(non_camel_case_types)]
type acl_t = *mut c_void;

unsafe extern "C" {
    fn acl_get_fd(fd: c_int) -> acl_t;
    fn acl_set_fd(fd: c_int, acl: acl_t) -> c_int;
    fn acl_free(obj: *mut c_void) -> c_int;
}

/// An ACL read with `acl_get_fd()`, freed on drop.
#[derive(Debug)]
pub struct Acl(acl_t);

impl Acl {
    /// `acl_get_fd(fd)`; Err(errno).
    pub fn get_fd(fd: Fd) -> Result<Acl, i32> {
        // SAFETY: plain call; a non-NULL result is ours to free.
        let a = unsafe { acl_get_fd(fd) };
        if a.is_null() { Err(errno()) } else { Ok(Acl(a)) }
    }

    /// `acl_set_fd(fd, acl)`; 0 or -1 (errno set).
    pub fn set_fd(&self, fd: Fd) -> c_int {
        // SAFETY: self.0 is a valid ACL.
        unsafe { acl_set_fd(fd, self.0) }
    }
}

impl Drop for Acl {
    fn drop(&mut self) {
        // SAFETY: self.0 came from acl_get_fd and is freed once.
        unsafe { acl_free(self.0) };
    }
}

// ---------------------------------------------------------------------------
// popt

#[repr(C)]
struct PoptOption {
    long_name: *const c_char,
    short_name: c_char,
    arg_info: c_uint,
    arg: *mut c_void,
    val: c_int,
    descrip: *const c_char,
    arg_descrip: *const c_char,
}

#[allow(non_camel_case_types)]
type poptContext = *mut c_void;

unsafe extern "C" {
    static mut poptHelpOptions: [PoptOption; 0];
    fn poptGetContext(
        name: *const c_char,
        argc: c_int,
        argv: *const *const c_char,
        options: *const PoptOption,
        flags: c_uint,
    ) -> poptContext;
    fn poptReadDefaultConfig(con: poptContext, use_env: c_int) -> c_int;
    fn poptSetOtherOptionHelp(con: poptContext, text: *const c_char);
    fn poptGetNextOpt(con: poptContext) -> c_int;
    fn poptBadOption(con: poptContext, flags: c_uint) -> *const c_char;
    fn poptStrerror(error: c_int) -> *const c_char;
    fn poptGetArgs(con: poptContext) -> *mut *const c_char;
    fn poptPrintUsage(con: poptContext, fp: *mut libc::FILE, flags: c_int);
    fn poptFreeContext(con: poptContext) -> poptContext;
    fn poptParseArgvString(s: *const c_char, argc: *mut c_int, argv: *mut *mut *const c_char) -> c_int;
    static mut stderr: *mut libc::FILE;
}

const POPT_ARG_NONE: c_uint = 0;
const POPT_ARG_STRING: c_uint = 1;
const POPT_ARG_INCLUDE_TABLE: c_uint = 4;
pub const POPT_BADOPTION_NOALIAS: c_uint = 1;

/// Where popt stores logrotate's option values (main()'s locals).
#[derive(Debug)]
#[repr(C)]
struct OptionValues {
    force: c_int,
    skip_state_lock: c_int,
    wait_for_state_lock: c_int,
    mail_command: *const c_char,
    state_file: *const c_char,
    log_file: *const c_char,
}

/// main()'s popt context, with logrotate's option table.
#[derive(Debug)]
pub struct Popt {
    con: poptContext,
    values: *mut OptionValues,
}

fn cptr_bytes(p: *const c_char) -> Option<Vec<u8>> {
    // SAFETY: p is NULL or a C string popt (or we) left there.
    (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_bytes().to_vec())
}

impl Popt {
    /// `poptGetContext("logrotate", argc, argv, options, 0)` with main()'s
    /// option table; `mail` and `state` start at the given defaults.
    pub fn new(args: &[Vec<u8>], mail_default: &'static CStr, state_default: &'static CStr) -> Popt {
        // popt keeps pointers to argv, the table and the values for the
        // context's lifetime, which is the program's: leak them.
        let argv: Vec<*const c_char> = args.iter().map(|a| to_cstring(a).into_raw() as *const c_char).collect();
        let argc = argv.len() as c_int;
        let mut argv = argv;
        argv.push(std::ptr::null());
        let argv = Box::leak(argv.into_boxed_slice());
        let values = Box::into_raw(Box::new(OptionValues {
            force: 0,
            skip_state_lock: 0,
            wait_for_state_lock: 0,
            mail_command: mail_default.as_ptr(),
            state_file: state_default.as_ptr(),
            log_file: std::ptr::null(),
        }));
        // SAFETY: values is a valid, leaked allocation; the field pointers
        // stay valid for the program's lifetime.
        let (force, skip, wait, mail, state, log) = unsafe {
            (
                (&raw mut (*values).force).cast::<c_void>(),
                (&raw mut (*values).skip_state_lock).cast::<c_void>(),
                (&raw mut (*values).wait_for_state_lock).cast::<c_void>(),
                (&raw mut (*values).mail_command).cast::<c_void>(),
                (&raw mut (*values).state_file).cast::<c_void>(),
                (&raw mut (*values).log_file).cast::<c_void>(),
            )
        };
        let o = |long: Option<&'static CStr>,
                 short: u8,
                 info: c_uint,
                 arg: *mut c_void,
                 val: c_int,
                 descrip: Option<&'static CStr>,
                 argd: Option<&'static CStr>| PoptOption {
            long_name: long.map_or(std::ptr::null(), CStr::as_ptr),
            short_name: short as c_char,
            arg_info: info,
            arg,
            val,
            descrip: descrip.map_or(std::ptr::null(), CStr::as_ptr),
            arg_descrip: argd.map_or(std::ptr::null(), CStr::as_ptr),
        };
        let null = std::ptr::null_mut();
        let table = vec![
            o(
                Some(c"debug"),
                b'd',
                0,
                null,
                c_int::from(b'd'),
                Some(c"Don't do anything, just test and print debug messages"),
                None,
            ),
            o(Some(c"force"), b'f', 0, force, 0, Some(c"Force file rotation"), None),
            o(
                Some(c"mail"),
                b'm',
                POPT_ARG_STRING,
                mail,
                0,
                Some(c"Command to send mail (instead of `/bin/mail')"),
                Some(c"command"),
            ),
            o(Some(c"state"), b's', POPT_ARG_STRING, state, 0, Some(c"Path of state file"), Some(c"statefile")),
            o(Some(c"skip-state-lock"), 0, POPT_ARG_NONE, skip, 0, Some(c"Do not lock the state file"), None),
            o(Some(c"wait-for-state-lock"), 0, POPT_ARG_NONE, wait, 0, Some(c"Wait for lock on the state file"), None),
            o(Some(c"verbose"), b'v', 0, null, c_int::from(b'v'), Some(c"Display messages during rotation"), None),
            o(
                Some(c"log"),
                b'l',
                POPT_ARG_STRING,
                log,
                c_int::from(b'l'),
                Some(c"Log file or 'syslog' to log to syslog"),
                Some(c"logfile"),
            ),
            o(Some(c"version"), 0, POPT_ARG_NONE, null, c_int::from(b'V'), Some(c"Display version information"), None),
            PoptOption {
                long_name: std::ptr::null(),
                short_name: 0,
                arg_info: POPT_ARG_INCLUDE_TABLE,
                arg: (&raw mut poptHelpOptions).cast::<c_void>(),
                val: 0,
                descrip: c"Help options:".as_ptr(),
                arg_descrip: std::ptr::null(),
            },
            o(None, 0, 0, null, 0, None, None),
        ];
        let table = Box::leak(table.into_boxed_slice());
        // SAFETY: every pointer passed lives for the rest of the program.
        let con = unsafe { poptGetContext(c"logrotate".as_ptr(), argc, argv.as_ptr(), table.as_ptr(), 0) };
        Popt { con, values }
    }

    pub fn read_default_config(&self) {
        // SAFETY: con is a live context.
        unsafe { poptReadDefaultConfig(self.con, 1) };
    }

    pub fn set_other_option_help(&self, text: &'static CStr) {
        // SAFETY: con is live; text is static.
        unsafe { poptSetOtherOptionHelp(self.con, text.as_ptr()) };
    }

    pub fn next_opt(&self) -> c_int {
        // SAFETY: con is live.
        unsafe { poptGetNextOpt(self.con) }
    }

    pub fn bad_option(&self, flags: c_uint) -> Vec<u8> {
        // SAFETY: con is live; the result is copied at once.
        cptr_bytes(unsafe { poptBadOption(self.con, flags) }).unwrap_or_default()
    }

    pub fn strerror(err: c_int) -> Vec<u8> {
        // SAFETY: poptStrerror returns a static string.
        cptr_bytes(unsafe { poptStrerror(err) }).unwrap_or_default()
    }

    /// `poptGetArgs()`: the leftover arguments, or None.
    pub fn get_args(&self) -> Option<Vec<Vec<u8>>> {
        // SAFETY: con is live; the array is NULL-terminated.
        unsafe {
            let p = poptGetArgs(self.con);
            if p.is_null() {
                return None;
            }
            let mut out = Vec::new();
            let mut i = 0;
            while !(*p.add(i)).is_null() {
                out.push(CStr::from_ptr(*p.add(i)).to_bytes().to_vec());
                i += 1;
            }
            Some(out)
        }
    }

    /// `poptPrintUsage(con, stderr, 0)`.
    pub fn print_usage_stderr(&self) {
        // SAFETY: con is live; stderr is the C library's stream.
        unsafe { poptPrintUsage(self.con, stderr, 0) };
    }

    pub fn force(&self) -> bool {
        // SAFETY: values lives for the program's lifetime.
        unsafe { (*self.values).force != 0 }
    }
    pub fn skip_state_lock(&self) -> bool {
        // SAFETY: as above.
        unsafe { (*self.values).skip_state_lock != 0 }
    }
    pub fn wait_for_state_lock(&self) -> bool {
        // SAFETY: as above.
        unsafe { (*self.values).wait_for_state_lock != 0 }
    }
    pub fn mail_command(&self) -> Vec<u8> {
        // SAFETY: as above.
        cptr_bytes(unsafe { (*self.values).mail_command }).unwrap_or_default()
    }
    pub fn state_file(&self) -> Vec<u8> {
        // SAFETY: as above.
        cptr_bytes(unsafe { (*self.values).state_file }).unwrap_or_default()
    }
    pub fn log_file(&self) -> Option<Vec<u8>> {
        // SAFETY: as above.
        cptr_bytes(unsafe { (*self.values).log_file })
    }

    /// `poptFreeContext()`.
    pub fn free(&mut self) {
        if !self.con.is_null() {
            // SAFETY: con is live and freed once.
            unsafe { poptFreeContext(self.con) };
            self.con = std::ptr::null_mut();
        }
    }
}

/// `poptParseArgvString(s, &argc, &argv)`: Ok(argv) or Err(popt error).
pub fn popt_parse_argv_string(s: &[u8]) -> Result<Vec<Vec<u8>>, c_int> {
    let c = to_cstring(s);
    let mut argc: c_int = 0;
    let mut argv: *mut *const c_char = std::ptr::null_mut();
    // SAFETY: c is NUL-terminated; on success argv is one malloc'd block
    // holding argc strings, which we copy and then free.
    unsafe {
        let rc = poptParseArgvString(c.as_ptr(), &mut argc, &mut argv);
        if rc != 0 {
            return Err(rc);
        }
        let mut out = Vec::with_capacity(argc as usize);
        for i in 0..argc as usize {
            out.push(CStr::from_ptr(*argv.add(i)).to_bytes().to_vec());
        }
        libc::free(argv.cast());
        Ok(out)
    }
}

// ---------------------------------------------------------------------------
// process

/// `exit(code)`: C's exit, so the C library flushes its streams (popt
/// writes through them).
pub fn exit(code: c_int) -> ! {
    // SAFETY: ends the process.
    unsafe { libc::exit(code) }
}

/// Restore SIGPIPE's default disposition (Rust's runtime ignores it, and
/// children would inherit that).
pub fn sigpipe_default() {
    // SAFETY: setting a standard disposition.
    unsafe { libc::signal(libc::SIGPIPE, libc::SIG_DFL) };
}
