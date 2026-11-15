//! Port of config.c: the configuration parser.
//!
//! `readConfigFile()` is a character-at-a-time state machine over the
//! mapped file.  The port keeps its structure: `start` is an index into the
//! file, and each branch ends the way the C code's does, either falling to
//! the bottom of the loop body (where a `'\n'` under `start` counts a line)
//! or `continue`-ing past it.  Several upstream quirks follow from exactly
//! that, so the distinction is kept (see [`Flow`]).
//!
//! Log definitions live in a tail queue (`TAILQ`) in C, and one error path
//! copies a stale `list` entry back into a definition, so the queue is
//! modeled literally: an arena of definitions linked by index.

use crate::bcat;
use crate::log::{MESS_DEBUG, MESS_ERROR, MESS_FATAL, MESS_WARN, message};
use crate::msg;
use crate::rotate;
use crate::sys;

pub const LOG_FLAG_COMPRESS: u32 = 1 << 0;
pub const LOG_FLAG_CREATE: u32 = 1 << 1;
pub const LOG_FLAG_IFEMPTY: u32 = 1 << 2;
pub const LOG_FLAG_DELAYCOMPRESS: u32 = 1 << 3;
pub const LOG_FLAG_COPYTRUNCATE: u32 = 1 << 4;
pub const LOG_FLAG_MISSINGOK: u32 = 1 << 5;
pub const LOG_FLAG_MAILFIRST: u32 = 1 << 6;
pub const LOG_FLAG_SHAREDSCRIPTS: u32 = 1 << 7;
pub const LOG_FLAG_COPY: u32 = 1 << 8;
pub const LOG_FLAG_DATEEXT: u32 = 1 << 9;
pub const LOG_FLAG_SHRED: u32 = 1 << 10;
pub const LOG_FLAG_SU: u32 = 1 << 11;
pub const LOG_FLAG_DATEYESTERDAY: u32 = 1 << 12;
pub const LOG_FLAG_OLDDIRCREATE: u32 = 1 << 13;
pub const LOG_FLAG_TMPFILENAME: u32 = 1 << 14;
pub const LOG_FLAG_DATEHOURAGO: u32 = 1 << 15;
pub const LOG_FLAG_ALLOWHARDLINK: u32 = 1 << 16;
pub const LOG_FLAG_IGNOREDUPLICATES: u32 = 1 << 17;

pub const NO_MODE: u32 = u32::MAX;
pub const NO_UID: u32 = u32::MAX;
pub const NO_GID: u32 = u32::MAX;

pub const COMPRESS_COMMAND: &[u8] = b"/bin/gzip";
pub const UNCOMPRESS_COMMAND: &[u8] = b"/bin/gunzip";
pub const COMPRESS_EXT: &[u8] = b".gz";

const ROOT_UID: u32 = 0;
const MAX_NESTING: u32 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Criterium {
    Hourly,
    Days,
    Weekly,
    Monthly,
    Yearly,
    Size,
}

/// `crit_to_string()`.
fn crit_to_string(c: Criterium) -> &'static str {
    match c {
        Criterium::Hourly => "hourly",
        Criterium::Days => "daily",
        Criterium::Weekly => "weekly",
        Criterium::Monthly => "monthly",
        Criterium::Yearly => "yearly",
        Criterium::Size => "size",
    }
}

/// A place a `TAILQ` link pointer can point to: the head's `tqh_first`, or
/// an element's `tqe_next`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Slot {
    Head,
    Next(usize),
}

/// `TAILQ_ENTRY(logInfo) list`.
#[derive(Debug, Clone, Copy)]
struct Entry {
    next: Option<usize>,
    prev: Slot,
}

/// `struct logInfo`.
#[derive(Debug, Clone)]
pub struct LogInfo {
    pub pattern: Option<Vec<u8>>,
    pub files: Vec<Vec<u8>>,
    pub old_dir: Option<Vec<u8>>,
    pub criterium: Criterium,
    pub weekday: u32,
    pub threshold: i64,
    pub maxsize: i64,
    pub minsize: i64,
    pub rotate_count: i32,
    pub rotate_min_age: i32,
    pub rotate_age: i32,
    pub log_start: i32,
    pub pre: Option<Vec<u8>>,
    pub post: Option<Vec<u8>>,
    pub first: Option<Vec<u8>>,
    pub last: Option<Vec<u8>>,
    pub preremove: Option<Vec<u8>>,
    pub log_address: Option<Vec<u8>>,
    pub extension: Option<Vec<u8>>,
    pub addextension: Option<Vec<u8>>,
    pub compress_prog: Option<Vec<u8>>,
    pub uncompress_prog: Option<Vec<u8>>,
    pub compress_ext: Option<Vec<u8>>,
    pub dateformat: Option<Vec<u8>>,
    pub flags: u32,
    pub shred_cycles: i32,
    pub create_mode: u32,
    pub create_uid: u32,
    pub create_gid: u32,
    pub su_uid: u32,
    pub su_gid: u32,
    pub olddir_mode: u32,
    pub olddir_uid: u32,
    pub olddir_gid: u32,
    /// `compress_options_list`/`_count`; `Some` only with at least one item.
    pub compress_options: Option<Vec<Vec<u8>>>,
    list: Entry,
}

impl LogInfo {
    /// readAllConfigPaths()'s initial `defConfig`.
    fn initial() -> LogInfo {
        LogInfo {
            pattern: None,
            files: Vec::new(),
            old_dir: None,
            criterium: Criterium::Size,
            weekday: 0,
            threshold: 1024 * 1024,
            maxsize: 0,
            minsize: 0,
            rotate_count: 0,
            rotate_min_age: 0,
            rotate_age: 0,
            log_start: 1,
            pre: None,
            post: None,
            first: None,
            last: None,
            preremove: None,
            log_address: None,
            extension: None,
            addextension: None,
            compress_prog: None,
            uncompress_prog: None,
            compress_ext: None,
            dateformat: None,
            flags: LOG_FLAG_IFEMPTY,
            shred_cycles: 0,
            create_mode: NO_MODE,
            create_uid: NO_UID,
            create_gid: NO_GID,
            su_uid: NO_UID,
            su_gid: NO_GID,
            olddir_mode: NO_MODE,
            olddir_uid: NO_UID,
            olddir_gid: NO_GID,
            compress_options: None,
            list: Entry { next: None, prev: Slot::Head },
        }
    }

    /// `copyLogInfo()`.  QUIRK: `pattern`, `files` and `addextension` are
    /// not copied, while the `list` links are.
    fn copy_from(from: &LogInfo) -> LogInfo {
        LogInfo { pattern: None, files: Vec::new(), addextension: None, ..from.clone() }
    }

    pub fn has(&self, flag: u32) -> bool {
        self.flags & flag != 0
    }
}

/// Which script a `STATE_LOAD_SCRIPT` is collecting (`scriptDest`).
#[derive(Debug, Clone, Copy)]
enum ScriptField {
    Pre,
    Post,
    First,
    Last,
    Preremove,
}

/// How a branch of the parser's loop body ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Flow {
    /// `break` out of the switch: the line counter sees `start`.
    Bottom,
    /// `continue` (or `goto next_state`): straight to `start++`.
    Next,
    /// `goto error`.
    Error,
}

const STATE_DEFAULT: i32 = 2;
const STATE_SKIP_LINE: i32 = 4;
const STATE_DEFINITION_END: i32 = 8;
const STATE_SKIP_CONFIG: i32 = 16;
const STATE_LOAD_SCRIPT: i32 = 32;
const STATE_ERROR: i32 = 64;

const DEF_TABOO_EXTS: [&str; 22] = [
    ",v",
    ".bak",
    ".cfsaved",
    ".disabled",
    ".dpkg-bak",
    ".dpkg-del",
    ".dpkg-dist",
    ".dpkg-new",
    ".dpkg-old",
    ".dpkg-tmp",
    ".new",
    ".old",
    ".orig",
    ".rhn-cfg-tmp-*",
    ".rpmnew",
    ".rpmorig",
    ".rpmsave",
    ".swp",
    ".ucf-dist",
    ".ucf-new",
    ".ucf-old",
    "~",
];

const COMPRESS_CMD_LIST: [(&[u8], &[u8]); 6] = [
    (b"gzip", b".gz"),
    (b"bzip2", b".bz2"),
    (b"xz", b".xz"),
    (b"zstd", b".zst"),
    (b"compress", b".Z"),
    (b"zip", b".zip"),
];

/// The parsed configuration: the `logs` queue (with the definitions it
/// holds) and `numLogs`.
#[derive(Debug)]
pub struct Logs {
    nodes: Vec<LogInfo>,
    first: Option<usize>,
    last: Slot,
    pub num_logs: i32,
}

impl Logs {
    fn slot_get(&self, s: Slot) -> Option<usize> {
        match s {
            Slot::Head => self.first,
            Slot::Next(i) => self.nodes[i].list.next,
        }
    }

    fn slot_set(&mut self, s: Slot, v: Option<usize>) {
        match s {
            Slot::Head => self.first = v,
            Slot::Next(i) => self.nodes[i].list.next = v,
        }
    }

    /// `TAILQ_INSERT_TAIL()`.
    fn insert_tail(&mut self, id: usize) {
        self.nodes[id].list.next = None;
        self.nodes[id].list.prev = self.last;
        self.slot_set(self.last, Some(id));
        self.last = Slot::Next(id);
    }

    /// `TAILQ_REMOVE()`.
    fn remove(&mut self, id: usize) {
        let Entry { next, prev } = self.nodes[id].list;
        match next {
            Some(n) => self.nodes[n].list.prev = prev,
            None => self.last = prev,
        }
        self.slot_set(prev, next);
    }

    /// `TAILQ_LAST()`: `*(((struct logInfoHead *)tqh_last)->tqh_last)`.
    fn last_elem(&self) -> Option<usize> {
        let prev = match self.last {
            Slot::Head => self.last,
            Slot::Next(i) => self.nodes[i].list.prev,
        };
        self.slot_get(prev)
    }

    /// The definitions in queue order (`TAILQ_FOREACH`).
    pub fn iter(&self) -> impl Iterator<Item = &LogInfo> {
        let mut cur = self.first;
        std::iter::from_fn(move || {
            let id = cur?;
            cur = self.nodes[id].list.next;
            Some(&self.nodes[id])
        })
    }

    fn ids(&self) -> Vec<usize> {
        let mut out = Vec::new();
        let mut cur = self.first;
        while let Some(id) = cur {
            out.push(id);
            cur = self.nodes[id].list.next;
        }
        out
    }
}

/// The parser's globals: the queue, the taboo patterns and the include depth.
#[derive(Debug)]
struct Parser {
    logs: Logs,
    taboo: Vec<Vec<u8>>,
    recursion_depth: u32,
}

/// The id of readAllConfigPaths()'s `defConfig` in the arena.
const DEF_CONFIG: usize = 0;

/// The bytes a C `%s` of `&buf[at]` prints: up to the first NUL.  The
/// buffer is a mapping of the file, zero-filled after its end.
fn c_rest(buf: &[u8], at: usize) -> &[u8] {
    let rest = buf.get(at..).unwrap_or(&[]);
    crate::bytes::c_str_prefix(rest)
}

/// printf's `%#x` of a `char` promoted to `int`.  QUIRK: `char` is signed,
/// so bytes from 0x80 print as `0xffffff80` and up.
fn fmt_hash_x(c: u8) -> String {
    let v = c as i8 as i32;
    if v == 0 { "0".to_string() } else { format!("{:#x}", v as u32) }
}

/// `isolateLine()`.  QUIRK: trailing blanks are trimmed only before a
/// newline, not at the end of a file without one.
fn isolate_line(strt: &mut usize, buf: &[u8]) -> Vec<u8> {
    let max = buf.len();
    let start = *strt;
    let mut endtag = start;
    while endtag < max && buf[endtag] != b'\n' {
        endtag += 1;
    }
    let tmp = endtag.wrapping_sub(1);
    let mut e = endtag as isize;
    while e >= start as isize && (e as usize) < max && sys::isspace(buf[e as usize]) {
        e -= 1;
    }
    let mut llen = (e - start as isize + 1) as usize;
    if start + llen > max {
        llen = max - start;
    }
    let key = crate::bytes::c_str_prefix(&buf[start..start + llen]).to_vec();
    *strt = tmp;
    key
}

/// `isolateValue()`.
fn isolate_value(file: &[u8], line: i32, key: &[u8], start: &mut usize, buf: &[u8]) -> Option<Vec<u8>> {
    let max = buf.len();
    let mut ch = *start;
    while ch < max && sys::isblank(buf[ch]) {
        ch += 1;
    }
    if ch < max && buf[ch] == b'=' {
        ch += 1;
        while ch < max && sys::isblank(buf[ch]) {
            ch += 1;
        }
    }
    if ch < max && buf[ch] == b'\n' {
        msg!(MESS_ERROR, file, ":", line.to_string(), " argument expected after ", key, "\n");
        return None;
    }
    *start = ch;
    Some(isolate_line(start, buf))
}

/// `isolateWord()`.
fn isolate_word(strt: &mut usize, buf: &[u8]) -> Vec<u8> {
    let max = buf.len();
    let mut start = *strt;
    while start < max && sys::isblank(buf[start]) {
        start += 1;
    }
    let mut endtag = start;
    while endtag < max && sys::isalpha(buf[endtag]) {
        endtag += 1;
    }
    let key = buf[start..endtag].to_vec();
    *strt = endtag;
    key
}

/// `readPath()`.
fn read_path(file: &[u8], line: i32, key: &str, start: &mut usize, buf: &[u8]) -> Option<Vec<u8>> {
    let path = isolate_value(file, line, key.as_bytes(), start, buf)?;
    if !sys::path_chars_ok(&path) {
        msg!(MESS_ERROR, file, ":", line.to_string(), " bad ", key, " path ", path, "\n");
        return None;
    }
    Some(path)
}

/// `resolveUid()`.
fn resolve_uid(name: &[u8]) -> Option<u32> {
    if let Some(uid) = sys::getpwnam_uid(name) {
        return Some(uid);
    }
    let p = sys::strtoul(name, 10);
    if !name.is_empty() && p.end == name.len() && p.value < i32::MAX as libc::c_ulong {
        let uid = p.value as u32;
        if sys::getpwuid_exists(uid) {
            return Some(uid);
        }
    }
    None
}

/// `resolveGid()`.
fn resolve_gid(name: &[u8]) -> Option<u32> {
    if let Some(gid) = sys::getgrnam_gid(name) {
        return Some(gid);
    }
    let p = sys::strtoul(name, 10);
    if !name.is_empty() && p.end == name.len() && p.value < i32::MAX as libc::c_ulong {
        let gid = p.value as u32;
        if sys::getgrgid_exists(gid) {
            return Some(gid);
        }
    }
    None
}

/// `readModeUidGid()`: 0, or -1 after an error message.
fn read_mode_uid_gid(
    file: &[u8],
    line: i32,
    key: &[u8],
    directive: &str,
    mode: &mut u32,
    uid: &mut u32,
    gid: &mut u32,
) -> i32 {
    let at = |i: usize| -> u8 { key.get(i).copied().unwrap_or(0) };
    let is_su = directive == "su";
    let ln = line.to_string();
    let mut p = 0;
    while at(p) != 0 && sys::isspace(at(p)) {
        p += 1;
    }
    if at(p) == 0 {
        return 0;
    }
    let mut val: [Option<Vec<u8>>; 3] = [None, None, None];
    let mut i = 0;
    while i < 3 {
        let mut start = p;
        let mut endchr = 0u8;
        while sys::isspace(at(start)) {
            start += 1;
        }
        if at(start) == b'\'' || at(start) == b'"' {
            endchr = at(start);
            start += 1;
        }
        p = start;
        while at(p) != 0 && if endchr != 0 { at(p) != endchr } else { !sys::isspace(at(p)) } {
            p += 1;
        }
        if endchr != 0 && at(p) != endchr {
            msg!(MESS_ERROR, file, ":", ln, " invalid arguments for ", directive, "\n");
            return -1;
        }
        val[i] = Some(key[start.min(key.len())..p.min(key.len())].to_vec());
        if endchr != 0 {
            p += 1;
        }
        if at(p) == 0 {
            break;
        }
        i += 1;
    }
    if i >= 3 || (i == 2 && is_su) {
        msg!(MESS_ERROR, file, ":", ln, " extra arguments for ", directive, "\n");
        return -1;
    }
    let [v0, v1, v2] = val;
    let (modestr, userstr, groupstr) = match i {
        2 => (v0, v1, v2),
        1 => (None, v0, v1),
        _ if is_su => (None, v0, None),
        _ => (v0, None, None),
    };
    if let Some(g) = groupstr {
        match resolve_gid(&g) {
            Some(v) => *gid = v,
            None => {
                msg!(MESS_ERROR, file, ":", ln, " unknown group '", g, "'\n");
                return -1;
            }
        }
    }
    if let Some(u) = userstr {
        match resolve_uid(&u) {
            Some(v) => *uid = v,
            None => {
                msg!(MESS_ERROR, file, ":", ln, " unknown user '", u, "'\n");
                return -1;
            }
        }
    }
    if let Some(m) = modestr {
        let r = sys::strtol(&m, 8);
        let v = r.value;
        if r.errno != 0
            || m.is_empty()
            || r.end != m.len()
            || v < 0
            || v >= i32::MAX as libc::c_long
            || (v as libc::mode_t) as libc::c_long != v
        {
            msg!(MESS_ERROR, file, ":", ln, " invalid mode '", m, "'\n");
            return -1;
        }
        *mode = v as u32;
    }
    0
}

/// `readAddress()`.
fn read_address(file: &[u8], line: i32, key: &str, start: &mut usize, buf: &[u8]) -> Option<Vec<u8>> {
    let orig = *start;
    let address = isolate_value(file, line, key.as_bytes(), start, buf)?;
    let mut i = 0;
    while i < address.len() && sys::isprint(address[i]) && address[i] != b' ' {
        i += 1;
    }
    if i < address.len() {
        // QUIRK: prints the file from the directive's argument to its end.
        msg!(MESS_ERROR, file, ":", line.to_string(), " bad ", key, " address ", c_rest(buf, orig), "\n");
        return None;
    }
    Some(address)
}

/// `do_mkdir()`.
fn do_mkdir(path: &[u8], mode: u32, uid: u32, gid: u32) -> i32 {
    if sys::mkdir(path, mode) == 0 {
        let fd = sys::open(path, libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW, 0);
        if fd < 0 {
            msg!(MESS_ERROR, "error opening ", path, " after creation: ", sys::strerror(sys::errno()), "\n");
            return -1;
        }
        if sys::fchown(fd, uid, gid) != 0 {
            msg!(
                MESS_ERROR,
                "error setting owner of ",
                path,
                format!(" to uid {uid} and gid {gid}: "),
                sys::strerror(sys::errno()),
                "\n"
            );
            sys::close(fd);
            return -1;
        }
        if sys::fchmod(fd, mode) != 0 {
            msg!(
                MESS_ERROR,
                "error setting permissions of ",
                path,
                format!(" to 0{mode:o}: "),
                sys::strerror(sys::errno()),
                "\n"
            );
            sys::close(fd);
            return -1;
        }
        sys::close(fd);
        return 0;
    }
    if sys::errno() == libc::EEXIST {
        if let Ok(sb) = sys::stat(path)
            && sys::s_isdir(sb.st_mode)
        {
            return 0;
        }
        msg!(MESS_ERROR, "path ", path, " already exists, but it is not a directory\n");
        sys::set_errno(libc::ENOTDIR);
        return -1;
    }
    msg!(MESS_ERROR, "error creating ", path, ": ", sys::strerror(sys::errno()), "\n");
    -1
}

/// `mkpath()`.
fn mkpath(path: &[u8], mode: u32, uid: u32, gid: u32) -> i32 {
    msg!(
        MESS_DEBUG,
        "creating new directory ",
        path,
        format!(" mode = 0{mode:o} uid = {} gid = {}\n", uid as i32, gid as i32)
    );
    let mut rv = 0;
    let mut pp = 0;
    while rv == 0 {
        let Some(off) = path[pp..].iter().position(|&b| b == b'/') else { break };
        let sp = pp + off;
        if sp != pp {
            rv = do_mkdir(&path[..sp], mode, uid, gid);
        }
        pp = sp + 1;
    }
    if rv == 0 {
        rv = do_mkdir(path, mode, uid, gid);
    }
    rv
}

/// `expand_home_relative_path()`: 0, or 1 after an error message.
fn expand_home_relative_path(path: &mut Vec<u8>) -> i32 {
    if path.first() != Some(&b'~') || path.get(1) != Some(&b'/') {
        return 0;
    }
    let home = match sys::secure_getenv(c"HOME") {
        Some(h) => h,
        None => {
            message(
                MESS_DEBUG,
                b"cannot get HOME directory from environment to replace ~/, trying password database...\n",
            );
            match sys::getpwuid_dir(sys::getuid()) {
                Some(d) => d,
                None => {
                    msg!(
                        MESS_ERROR,
                        format!("cannot get passwd entry for running user {}: ", sys::getuid()),
                        sys::strerror(sys::errno()),
                        "\n"
                    );
                    return 1;
                }
            }
        }
    };
    let new_path = bcat!(home, "/", &path[2..]);
    msg!(MESS_DEBUG, "replaced '", &path[..], "' with '", new_path, "'\n");
    *path = new_path;
    0
}

impl Parser {
    /// `checkFile()`.
    fn check_file(&self, fname: &[u8]) -> bool {
        // Gentoo's ignore-hidden patch: any name starting with '.'.
        if fname.first() == Some(&b'.') {
            return false;
        }
        for pattern in &self.taboo {
            if sys::fnmatch_period(pattern, fname) {
                msg!(MESS_DEBUG, "Ignoring ", fname, ", because of ", pattern, " pattern match\n");
                return false;
            }
        }
        true
    }

    /// `newLogInfo()`.
    fn new_log_info(&mut self, template: usize) -> usize {
        let new = LogInfo::copy_from(&self.logs.nodes[template]);
        self.logs.nodes.push(new);
        let id = self.logs.nodes.len() - 1;
        self.logs.insert_tail(id);
        self.logs.num_logs += 1;
        id
    }

    /// `freeTailLogs()`.
    fn free_tail_logs(&mut self, mut num: i32) {
        msg!(MESS_DEBUG, format!("removing last {num} log configs\n"));
        while num > 0 {
            num -= 1;
            // removeLogInfo()
            if let Some(id) = self.logs.last_elem() {
                self.logs.remove(id);
                self.logs.num_logs -= 1;
            }
        }
    }

    /// Restore `def` from a backup after a failed file (`freeLogInfo` plus
    /// `copyLogInfo`).
    fn restore(&mut self, def: usize, backup: &LogInfo) {
        self.logs.nodes[def] = LogInfo::copy_from(backup);
    }

    /// `readConfigPath()`.
    fn read_config_path(&mut self, path: &[u8], def: usize) -> i32 {
        let sb = match sys::stat(path) {
            Ok(sb) => sb,
            Err(e) => {
                msg!(MESS_ERROR, "cannot stat ", path, ": ", sys::strerror(e), "\n");
                return 1;
            }
        };
        let mut result = 0;
        if sys::s_isdir(sb.st_mode) {
            let here = sys::open(b".", libc::O_RDONLY | libc::O_DIRECTORY, 0);
            if here == -1 {
                msg!(MESS_ERROR, "cannot open current directory: ", sys::strerror(sys::errno()), "\n");
                return 1;
            }
            let names = match sys::readdir_names(path) {
                Ok(n) => n,
                Err(e) => {
                    msg!(MESS_ERROR, "cannot open directory ", path, ": ", sys::strerror(e), "\n");
                    sys::close(here);
                    return 1;
                }
            };
            let mut namelist: Vec<Vec<u8>> = names.into_iter().filter(|n| self.check_file(n)).collect();
            if namelist.is_empty() {
                sys::close(here);
                return 0;
            }
            namelist.sort_by(|a, b| sys::strcoll(a, b));
            if sys::chdir(path) != 0 {
                msg!(MESS_ERROR, "error in chdir(\"", path, "\"): ", sys::strerror(sys::errno()), "\n");
                sys::close(here);
                return 1;
            }
            for name in &namelist {
                let backup = LogInfo::copy_from(&self.logs.nodes[def]);
                if self.read_config_file(name, def) != 0 {
                    msg!(MESS_ERROR, "found error in file ", name, ", skipping\n");
                    self.restore(def, &backup);
                    result = 1;
                    continue;
                }
            }
            if sys::fchdir(here) < 0 {
                msg!(MESS_ERROR, "could not change directory to '.': ", sys::strerror(sys::errno()), "\n");
            }
            sys::close(here);
        } else {
            let backup = LogInfo::copy_from(&self.logs.nodes[def]);
            if self.read_config_file(path, def) != 0 {
                self.restore(def, &backup);
                result = 1;
            }
        }
        result
    }

    /// `set_criterium()`.
    fn set_criterium(&mut self, newlog: usize, src: Criterium, set: &mut bool) {
        let dst = self.logs.nodes[newlog].criterium;
        if *set && dst != src {
            msg!(
                MESS_DEBUG,
                "note: '",
                crit_to_string(src),
                "' overrides previously specified '",
                crit_to_string(dst),
                "'\n"
            );
        }
        self.logs.nodes[newlog].criterium = src;
        *set = true;
    }

    /// `readConfigFile()`.
    fn read_config_file(&mut self, file: &[u8], def: usize) -> i32 {
        // QUIRK: a FIFO blocks here, before the regular-file check.
        let fd = sys::open(file, libc::O_RDONLY, 0);
        if fd < 0 {
            msg!(MESS_ERROR, "failed to open config file ", file, ": ", sys::strerror(sys::errno()), "\n");
            return 1;
        }
        if !sys::read_lock(fd) {
            msg!(MESS_ERROR, "Could not lock file ", file, " for reading\n");
        }
        let sb = match sys::fstat(fd) {
            Ok(sb) => sb,
            Err(e) => {
                msg!(MESS_ERROR, "fstat of ", file, " failed: ", sys::strerror(e), "\n");
                sys::close(fd);
                return 1;
            }
        };
        if !sys::s_isreg(sb.st_mode) {
            msg!(MESS_DEBUG, "Ignoring ", file, " because it's not a regular file.\n");
            sys::close(fd);
            return 0;
        }
        if sys::getuid() == ROOT_UID {
            if sb.st_mode & 0o7533 != 0o400 {
                msg!(MESS_WARN, "Potentially dangerous mode on ", file, format!(": 0{:o}\n", sb.st_mode & 0o7777));
            }
            if sb.st_mode & 0o022 != 0 {
                msg!(MESS_ERROR, "Ignoring ", file, " because it is writable by group or others.\n");
                sys::close(fd);
                return 1;
            }
            if sb.st_uid != ROOT_UID {
                msg!(
                    MESS_ERROR,
                    "Ignoring ",
                    file,
                    " because the file owner is wrong (should be root or user with uid 0).\n"
                );
                sys::close(fd);
                return 1;
            }
        }
        if sb.st_size > 0xffffff {
            msg!(MESS_ERROR, "file ", file, " too large, probably not a config file.\n");
            sys::close(fd);
            return 1;
        }
        let length = sb.st_size as usize;
        if length == 0 {
            msg!(MESS_DEBUG, "Ignoring ", file, " because it's empty.\n");
            sys::close(fd);
            return 0;
        }
        let buf = match sys::read_exact_len(fd, length) {
            Ok(b) => b,
            Err(e) => {
                msg!(MESS_ERROR, "Error mapping config file ", file, ": ", sys::strerror(e), "\n");
                sys::close(fd);
                return 1;
            }
        };
        msg!(MESS_DEBUG, "reading config file ", file, "\n");

        let r = self.parse(file, def, &buf);
        sys::close(fd);
        r
    }

    /// The body of `readConfigFile()` after the file is mapped.
    fn parse(&mut self, file: &[u8], def: usize, buf: &[u8]) -> i32 {
        let length = buf.len();
        let mut line_num: i32 = 1;
        let mut script_start: Option<usize> = None;
        let mut script_dest: Option<(usize, ScriptField)> = None;
        let mut newlog = def;
        let mut state = STATE_DEFAULT;
        let mut logerror = 0;
        let mut criterium_set = false;
        let mut globerr_msg: Option<Vec<u8>> = None;
        let mut in_config = false;
        let mut start: usize = 0;

        // RAISE_ERROR(): inside a definition, skip it; outside, give up.
        macro_rules! raise_error {
            () => {{
                if newlog != def {
                    state = STATE_ERROR;
                    Flow::Next
                } else {
                    Flow::Error
                }
            }};
        }

        while start < length {
            let ln = || line_num.to_string();
            let flow = match state {
                STATE_DEFAULT => 'sw: {
                    let c = buf[start];
                    if sys::isblank(c) {
                        break 'sw Flow::Next;
                    }
                    if c == b'#' {
                        state = STATE_SKIP_LINE;
                        break 'sw Flow::Next;
                    }
                    if sys::isalpha(c) {
                        let key = isolate_word(&mut start, buf);
                        if start < length && !sys::isspace(buf[start]) && buf[start] != b'=' {
                            msg!(
                                MESS_ERROR,
                                file,
                                ":",
                                ln(),
                                " keyword '",
                                key,
                                "' not properly separated, found ",
                                fmt_hash_x(buf[start]),
                                "\n"
                            );
                            break 'sw raise_error!();
                        }
                        let nl = newlog;
                        macro_rules! cur {
                            () => {
                                self.logs.nodes[nl]
                            };
                        }
                        match key.as_slice() {
                            b"compress" => cur!().flags |= LOG_FLAG_COMPRESS,
                            b"nocompress" => cur!().flags &= !LOG_FLAG_COMPRESS,
                            b"delaycompress" => cur!().flags |= LOG_FLAG_DELAYCOMPRESS,
                            b"nodelaycompress" => cur!().flags &= !LOG_FLAG_DELAYCOMPRESS,
                            b"shred" => cur!().flags |= LOG_FLAG_SHRED,
                            b"noshred" => cur!().flags &= !LOG_FLAG_SHRED,
                            b"allowhardlink" => cur!().flags |= LOG_FLAG_ALLOWHARDLINK,
                            b"noallowhardlink" => cur!().flags &= !LOG_FLAG_ALLOWHARDLINK,
                            b"sharedscripts" => cur!().flags |= LOG_FLAG_SHAREDSCRIPTS,
                            b"nosharedscripts" => cur!().flags &= !LOG_FLAG_SHAREDSCRIPTS,
                            b"copytruncate" => {
                                cur!().flags |= LOG_FLAG_COPYTRUNCATE;
                                cur!().flags &= !LOG_FLAG_TMPFILENAME;
                            }
                            b"nocopytruncate" => cur!().flags &= !LOG_FLAG_COPYTRUNCATE,
                            b"renamecopy" => {
                                cur!().flags |= LOG_FLAG_TMPFILENAME;
                                cur!().flags &= !LOG_FLAG_COPYTRUNCATE;
                            }
                            b"norenamecopy" => cur!().flags &= !LOG_FLAG_TMPFILENAME,
                            b"copy" => cur!().flags |= LOG_FLAG_COPY,
                            b"nocopy" => cur!().flags &= !LOG_FLAG_COPY,
                            b"ifempty" => cur!().flags |= LOG_FLAG_IFEMPTY,
                            b"notifempty" => cur!().flags &= !LOG_FLAG_IFEMPTY,
                            b"dateext" => cur!().flags |= LOG_FLAG_DATEEXT,
                            b"nodateext" => cur!().flags &= !LOG_FLAG_DATEEXT,
                            b"dateyesterday" => cur!().flags |= LOG_FLAG_DATEYESTERDAY,
                            b"datehourago" => cur!().flags |= LOG_FLAG_DATEHOURAGO,
                            b"dateformat" => {
                                cur!().dateformat = isolate_value(file, line_num, &key, &mut start, buf);
                            }
                            b"noolddir" => cur!().old_dir = None,
                            b"mailfirst" => cur!().flags |= LOG_FLAG_MAILFIRST,
                            b"maillast" => cur!().flags &= !LOG_FLAG_MAILFIRST,
                            b"su" => {
                                let value = isolate_line(&mut start, buf);
                                let mut tmp_mode = NO_MODE;
                                let (mut u, mut g) = (cur!().su_uid, cur!().su_gid);
                                let rv = read_mode_uid_gid(file, line_num, &value, "su", &mut tmp_mode, &mut u, &mut g);
                                cur!().su_uid = u;
                                cur!().su_gid = g;
                                if rv == -1 {
                                    break 'sw raise_error!();
                                } else if tmp_mode != NO_MODE {
                                    msg!(MESS_ERROR, file, ":", ln(), " extra arguments for su\n");
                                    break 'sw raise_error!();
                                } else if u == NO_UID {
                                    msg!(MESS_ERROR, file, ":", ln(), " no user for su\n");
                                    break 'sw raise_error!();
                                } else if g == NO_GID {
                                    msg!(MESS_ERROR, file, ":", ln(), " no group for su\n");
                                    break 'sw raise_error!();
                                }
                                cur!().flags |= LOG_FLAG_SU;
                            }
                            b"create" => {
                                let value = isolate_line(&mut start, buf);
                                let (mut m, mut u, mut g) = (cur!().create_mode, cur!().create_uid, cur!().create_gid);
                                let rv = read_mode_uid_gid(file, line_num, &value, "create", &mut m, &mut u, &mut g);
                                cur!().create_mode = m;
                                cur!().create_uid = u;
                                cur!().create_gid = g;
                                if rv == -1 {
                                    break 'sw raise_error!();
                                }
                                cur!().flags |= LOG_FLAG_CREATE;
                            }
                            b"createolddir" => {
                                let value = isolate_line(&mut start, buf);
                                let (mut m, mut u, mut g) = (cur!().olddir_mode, cur!().olddir_uid, cur!().olddir_gid);
                                let rv =
                                    read_mode_uid_gid(file, line_num, &value, "createolddir", &mut m, &mut u, &mut g);
                                if m == NO_MODE {
                                    m = 0o755;
                                }
                                cur!().olddir_mode = m;
                                cur!().olddir_uid = u;
                                cur!().olddir_gid = g;
                                if rv == -1 {
                                    break 'sw raise_error!();
                                }
                                cur!().flags |= LOG_FLAG_OLDDIRCREATE;
                            }
                            b"nocreateolddir" => cur!().flags &= !LOG_FLAG_OLDDIRCREATE,
                            b"nocreate" => cur!().flags &= !LOG_FLAG_CREATE,
                            b"size" | b"minsize" | b"maxsize" => {
                                let opt = key.clone();
                                let value = isolate_value(file, line_num, &opt, &mut start, buf);
                                let Some(mut value) = value.filter(|v| !v.is_empty()) else {
                                    // QUIRK: `continue` skips the line count when
                                    // `start` is still on the newline ("size\n").
                                    break 'sw Flow::Next;
                                };
                                let l = value.len() - 1;
                                let multiplier: u64 = match value[l] {
                                    b'k' | b'K' => 1024,
                                    b'M' => 1024 * 1024,
                                    b'G' => 1024 * 1024 * 1024,
                                    c if !sys::isdigit(c) => {
                                        msg!(MESS_ERROR, file, ":", ln(), " unknown unit '", [c], "'\n");
                                        break 'sw raise_error!();
                                    }
                                    _ => 1,
                                };
                                if multiplier != 1 {
                                    value.truncate(l);
                                }
                                let r = sys::strtoull(&value, 0);
                                let size = multiplier.wrapping_mul(r.value) as i64;
                                if r.end != value.len() || size < 0 {
                                    msg!(MESS_ERROR, file, ":", ln(), " bad size '", value, "'\n");
                                    break 'sw raise_error!();
                                }
                                if opt.starts_with(b"size") {
                                    self.set_criterium(nl, Criterium::Size, &mut criterium_set);
                                    cur!().threshold = size;
                                } else if opt.starts_with(b"maxsize") {
                                    cur!().maxsize = size;
                                } else {
                                    cur!().minsize = size;
                                }
                            }
                            b"shredcycles" => {
                                let Some(value) = isolate_value(file, line_num, b"shred cycles", &mut start, buf)
                                else {
                                    break 'sw raise_error!();
                                };
                                let r = sys::strtoul(&value, 0);
                                cur!().shred_cycles = r.value as i32;
                                if r.end != value.len() || cur!().shred_cycles < 0 {
                                    msg!(MESS_ERROR, file, ":", ln(), " bad shred cycles '", value, "'\n");
                                    break 'sw raise_error!();
                                }
                            }
                            b"hourly" => self.set_criterium(nl, Criterium::Hourly, &mut criterium_set),
                            b"daily" => {
                                self.set_criterium(nl, Criterium::Days, &mut criterium_set);
                                cur!().threshold = 1;
                            }
                            b"monthly" => self.set_criterium(nl, Criterium::Monthly, &mut criterium_set),
                            b"weekly" => {
                                self.set_criterium(nl, Criterium::Weekly, &mut criterium_set);
                                let value = isolate_line(&mut start, buf);
                                if value.is_empty() {
                                    cur!().weekday = 0;
                                    break 'sw Flow::Next;
                                }
                                let (n, weekday) = sys::sscanf_u_c(&value);
                                if n == 1 && weekday <= 7 {
                                    cur!().weekday = weekday;
                                    break 'sw Flow::Next;
                                }
                                msg!(MESS_ERROR, file, ":", ln(), " bad weekly directive '", value, "'\n");
                                break 'sw Flow::Error;
                            }
                            b"yearly" => self.set_criterium(nl, Criterium::Yearly, &mut criterium_set),
                            b"rotate" => {
                                let Some(value) = isolate_value(file, line_num, b"rotate count", &mut start, buf)
                                else {
                                    break 'sw raise_error!();
                                };
                                let r = sys::strtol(&value, 0);
                                cur!().rotate_count = r.value as i32;
                                if r.end != value.len() || cur!().rotate_count < -1 {
                                    msg!(MESS_ERROR, file, ":", ln(), " bad rotation count '", value, "'\n");
                                    break 'sw raise_error!();
                                }
                            }
                            b"start" => {
                                let Some(value) = isolate_value(file, line_num, b"start count", &mut start, buf) else {
                                    break 'sw raise_error!();
                                };
                                let r = sys::strtoul(&value, 0);
                                cur!().log_start = r.value as i32;
                                if r.end != value.len() || cur!().log_start < 0 {
                                    msg!(MESS_ERROR, file, ":", ln(), " bad start count '", value, "'\n");
                                    break 'sw raise_error!();
                                }
                            }
                            b"minage" | b"maxage" => {
                                let min = key.as_slice() == b"minage";
                                let what: &[u8] = if min { b"minage count" } else { b"maxage count" };
                                let Some(value) = isolate_value(file, line_num, what, &mut start, buf) else {
                                    break 'sw raise_error!();
                                };
                                let r = sys::strtoul(&value, 0);
                                let v = r.value as i32;
                                if min {
                                    cur!().rotate_min_age = v;
                                } else {
                                    cur!().rotate_age = v;
                                }
                                if r.end != value.len() || v < 0 {
                                    // QUIRK: prints the file from the value's last
                                    // character to its end, not the value.
                                    msg!(
                                        MESS_ERROR,
                                        file,
                                        ":",
                                        ln(),
                                        if min { " bad minimum age '" } else { " bad maximum age '" },
                                        c_rest(buf, start),
                                        "'\n"
                                    );
                                    break 'sw raise_error!();
                                }
                            }
                            // QUIRK: the argument isn't skipped, and is parsed as a keyword.
                            b"errors" => {
                                msg!(
                                    MESS_WARN,
                                    file,
                                    ": ",
                                    ln(),
                                    ": the errors directive is deprecated and no longer used.\n"
                                );
                            }
                            b"mail" => {
                                cur!().log_address = None;
                                match read_address(file, line_num, "mail", &mut start, buf) {
                                    Some(a) => cur!().log_address = Some(a),
                                    None => break 'sw raise_error!(),
                                }
                            }
                            b"nomail" => cur!().log_address = None,
                            b"missingok" => cur!().flags |= LOG_FLAG_MISSINGOK,
                            b"nomissingok" => cur!().flags &= !LOG_FLAG_MISSINGOK,
                            b"ignoreduplicates" => cur!().flags |= LOG_FLAG_IGNOREDUPLICATES,
                            b"prerotate" | b"firstaction" | b"postrotate" | b"lastaction" | b"preremove" => {
                                let field = match key.as_slice() {
                                    b"prerotate" => {
                                        cur!().pre = None;
                                        ScriptField::Pre
                                    }
                                    b"firstaction" => {
                                        cur!().first = None;
                                        ScriptField::First
                                    }
                                    b"postrotate" => {
                                        cur!().post = None;
                                        ScriptField::Post
                                    }
                                    b"lastaction" => {
                                        cur!().last = None;
                                        ScriptField::Last
                                    }
                                    _ => {
                                        cur!().preremove = None;
                                        ScriptField::Preremove
                                    }
                                };
                                script_start = Some(start);
                                script_dest = Some((nl, field));
                                state = STATE_LOAD_SCRIPT;
                            }
                            b"tabooext" | b"taboopat" => {
                                let ext = key.as_slice() == b"tabooext";
                                let name = if ext { "tabooext" } else { "taboopat" };
                                if newlog != def {
                                    msg!(
                                        MESS_ERROR,
                                        file,
                                        ":",
                                        ln(),
                                        " ",
                                        name,
                                        " may not appear inside of log file definition\n"
                                    );
                                    state = STATE_ERROR;
                                    break 'sw Flow::Next;
                                }
                                let Some(value) = isolate_value(file, line_num, name.as_bytes(), &mut start, buf)
                                else {
                                    break 'sw Flow::Next;
                                };
                                let at = |i: usize| -> u8 { value.get(i).copied().unwrap_or(0) };
                                let mut endtag = 0;
                                if at(0) == b'+' {
                                    endtag += 1;
                                    while sys::isspace(at(endtag)) && at(endtag) != 0 {
                                        endtag += 1;
                                    }
                                } else {
                                    self.taboo.clear();
                                }
                                while at(endtag) != 0 {
                                    let mut chptr = endtag;
                                    while !sys::isspace(at(chptr)) && at(chptr) != b',' && at(chptr) != 0 {
                                        chptr += 1;
                                    }
                                    if ext {
                                        // Only non-empty patterns, to avoid excluding everything.
                                        if endtag < chptr {
                                            self.taboo.push(bcat!("*", &value[endtag..chptr]));
                                        }
                                    } else {
                                        self.taboo.push(value[endtag..chptr].to_vec());
                                    }
                                    endtag = chptr;
                                    if at(endtag) == b',' {
                                        endtag += 1;
                                    }
                                    while at(endtag) != 0 && sys::isspace(at(endtag)) {
                                        endtag += 1;
                                    }
                                }
                            }
                            b"include" => {
                                let Some(mut value) = isolate_value(file, line_num, b"include", &mut start, buf) else {
                                    break 'sw raise_error!();
                                };
                                if expand_home_relative_path(&mut value) != 0 {
                                    break 'sw raise_error!();
                                }
                                msg!(MESS_DEBUG, "including ", value, "\n");
                                if self.recursion_depth >= MAX_NESTING {
                                    msg!(MESS_ERROR, file, ":", ln(), " include nesting too deep\n");
                                    logerror = 1;
                                    break 'sw Flow::Next;
                                }
                                self.recursion_depth += 1;
                                let rv = self.read_config_path(&value, newlog);
                                self.recursion_depth -= 1;
                                if rv != 0 {
                                    logerror = 1;
                                    break 'sw Flow::Next;
                                }
                            }
                            b"olddir" => {
                                cur!().old_dir = None;
                                let Some(mut dir) = read_path(file, line_num, "olddir", &mut start, buf) else {
                                    break 'sw raise_error!();
                                };
                                let rv = expand_home_relative_path(&mut dir);
                                cur!().old_dir = Some(dir.clone());
                                if rv != 0 {
                                    break 'sw raise_error!();
                                }
                                msg!(MESS_DEBUG, "olddir is now ", dir, "\n");
                            }
                            b"extension" => {
                                let Some(value) = isolate_value(file, line_num, b"extension name", &mut start, buf)
                                else {
                                    break 'sw raise_error!();
                                };
                                msg!(MESS_DEBUG, "extension is now ", value, "\n");
                                cur!().extension = Some(value);
                            }
                            b"addextension" => {
                                let Some(value) = isolate_value(file, line_num, b"addextension name", &mut start, buf)
                                else {
                                    break 'sw raise_error!();
                                };
                                msg!(MESS_DEBUG, "addextension is now ", value, "\n");
                                cur!().addextension = Some(value);
                            }
                            b"compresscmd" => {
                                cur!().compress_prog = None;
                                let Some(prog) = read_path(file, line_num, "compress", &mut start, buf) else {
                                    break 'sw raise_error!();
                                };
                                msg!(MESS_DEBUG, "compress_prog is now ", prog, "\n");
                                let base = sys::basename(&prog);
                                cur!().compress_prog = Some(prog);
                                if let Some((_, ext)) =
                                    COMPRESS_CMD_LIST.iter().find(|(cmd, _)| *cmd == base.as_slice())
                                {
                                    cur!().compress_ext = Some(ext.to_vec());
                                    msg!(MESS_DEBUG, "compress_ext was changed to ", ext, "\n");
                                }
                            }
                            b"uncompresscmd" => {
                                cur!().uncompress_prog = None;
                                let Some(prog) = read_path(file, line_num, "uncompress", &mut start, buf) else {
                                    break 'sw raise_error!();
                                };
                                msg!(MESS_DEBUG, "uncompress_prog is now ", prog, "\n");
                                cur!().uncompress_prog = Some(prog);
                            }
                            b"compressoptions" => {
                                cur!().compress_options = None;
                                let options = isolate_line(&mut start, buf);
                                match sys::popt_parse_argv_string(&options) {
                                    Ok(list) => cur!().compress_options = Some(list),
                                    Err(_) => {
                                        msg!(MESS_ERROR, file, ":", ln(), " invalid compression options\n");
                                        break 'sw raise_error!();
                                    }
                                }
                                msg!(MESS_DEBUG, "compress_options is now ", options, "\n");
                            }
                            b"compressext" => {
                                cur!().compress_ext = None;
                                let Some(ext) = read_path(file, line_num, "compress-ext", &mut start, buf) else {
                                    break 'sw raise_error!();
                                };
                                msg!(MESS_DEBUG, "compress_ext is now ", ext, "\n");
                                cur!().compress_ext = Some(ext);
                            }
                            _ => {
                                msg!(MESS_WARN, file, ":", ln(), " unknown option '", key, "' -- ignoring line\n");
                                if start < length && buf[start] != b'\n' {
                                    state = STATE_SKIP_LINE;
                                }
                            }
                        }
                        Flow::Bottom
                    } else if matches!(c, b'/' | b'"' | b'\'' | b'~') {
                        in_config = false;
                        if newlog != def {
                            msg!(MESS_ERROR, file, ":", ln(), " unexpected log filename\n");
                            state = STATE_ERROR;
                            break 'sw Flow::Next;
                        }
                        // Default compression settings, if none were given.
                        {
                            let d = &mut self.logs.nodes[newlog];
                            d.compress_prog.get_or_insert_with(|| COMPRESS_COMMAND.to_vec());
                            d.uncompress_prog.get_or_insert_with(|| UNCOMPRESS_COMMAND.to_vec());
                            d.compress_ext.get_or_insert_with(|| COMPRESS_EXT.to_vec());
                        }
                        newlog = self.new_log_info(def);
                        let Some(glob_string) = parse_glob_string(file, line_num, buf, &mut start) else {
                            break 'sw Flow::Error;
                        };
                        in_config = true;
                        let argv = match sys::popt_parse_argv_string(&glob_string) {
                            Ok(a) if !a.is_empty() => a,
                            Ok(_) => {
                                msg!(MESS_ERROR, file, ":", ln(), " { expected after log file name(s)\n");
                                break 'sw Flow::Error;
                            }
                            Err(_) => {
                                msg!(MESS_ERROR, file, ":", ln(), " error parsing filename\n");
                                break 'sw Flow::Error;
                            }
                        };
                        self.logs.nodes[newlog].files.clear();
                        for arg in &argv {
                            globerr_msg = None;
                            if arg.len() > 2048 {
                                msg!(MESS_ERROR, file, ":", ln(), format!(" glob too long ({} > 2048)\n", arg.len()));
                                logerror = 1;
                                continue;
                            }
                            let (rc, mut paths) = sys::glob_config(arg);
                            if rc == sys::GLOB_ABORTED {
                                if self.logs.nodes[newlog].has(LOG_FLAG_MISSINGOK) {
                                    continue;
                                }
                                // Whether this definition has "missingok" isn't known
                                // yet, so keep the message for later.
                                let m = bcat!(
                                    file,
                                    ":",
                                    ln(),
                                    " glob failed for ",
                                    arg,
                                    ": ",
                                    sys::strerror(sys::config_glob_errno()),
                                    "\n"
                                );
                                message(MESS_DEBUG, &m);
                                globerr_msg = Some(m);
                                paths.clear();
                            }
                            if paths.is_empty() {
                                msg!(MESS_DEBUG, file, ":", ln(), " no matches for glob '", arg, "', skipping\n");
                                continue;
                            }
                            'paths: for path in &paths {
                                if let Ok(sb) = sys::lstat(path)
                                    && sys::s_isdir(sb.st_mode)
                                {
                                    continue;
                                }
                                let mut add_file = true;
                                for id in self.logs.ids() {
                                    let log = &self.logs.nodes[id];
                                    for f in &log.files {
                                        if f == path {
                                            if log.has(LOG_FLAG_IGNOREDUPLICATES) {
                                                add_file = false;
                                                msg!(
                                                    MESS_DEBUG,
                                                    file,
                                                    ":",
                                                    ln(),
                                                    " ignore duplicate log entry for ",
                                                    path,
                                                    "\n"
                                                );
                                            } else {
                                                msg!(
                                                    MESS_ERROR,
                                                    file,
                                                    ":",
                                                    ln(),
                                                    " duplicate log entry for ",
                                                    path,
                                                    "\n"
                                                );
                                                logerror = 1;
                                                break 'paths;
                                            }
                                        }
                                    }
                                }
                                if add_file {
                                    self.logs.nodes[newlog].files.push(path.clone());
                                }
                            }
                        }
                        self.logs.nodes[newlog].pattern = Some(glob_string);
                        Flow::Bottom
                    } else if c == b'}' {
                        if newlog == def {
                            msg!(MESS_ERROR, file, ":", ln(), " unexpected }\n");
                            break 'sw Flow::Error;
                        }
                        if !in_config {
                            msg!(MESS_ERROR, file, ":", ln(), " unexpected } (missing previous '{')\n");
                            break 'sw Flow::Error;
                        }
                        in_config = false;
                        if let Some(m) = globerr_msg.take() {
                            let missingok = self.logs.nodes[newlog].has(LOG_FLAG_MISSINGOK);
                            if !missingok {
                                message(MESS_ERROR, &m);
                                break 'sw Flow::Error;
                            }
                        }
                        if self.check_olddirs(file, line_num, newlog) != 0 {
                            break 'sw Flow::Error;
                        }
                        criterium_set = false;
                        newlog = def;
                        state = STATE_DEFINITION_END;
                        Flow::Bottom
                    } else if c != b'\n' {
                        msg!(
                            MESS_ERROR,
                            file,
                            ":",
                            ln(),
                            " lines must begin with a keyword or a filename (possibly in double quotes)\n"
                        );
                        raise_error!()
                    } else {
                        Flow::Bottom
                    }
                }
                s if s == STATE_SKIP_LINE || s == STATE_SKIP_LINE | STATE_SKIP_CONFIG => {
                    if buf[start] == b'\n' {
                        state = if state & STATE_SKIP_CONFIG != 0 { STATE_SKIP_CONFIG } else { STATE_DEFAULT };
                    }
                    Flow::Bottom
                }
                s if s == STATE_SKIP_LINE | STATE_LOAD_SCRIPT => {
                    if buf[start] == b'\n' {
                        state = STATE_LOAD_SCRIPT;
                    }
                    Flow::Bottom
                }
                s if s == STATE_SKIP_LINE | STATE_LOAD_SCRIPT | STATE_SKIP_CONFIG => {
                    if buf[start] == b'\n' {
                        state = STATE_LOAD_SCRIPT | STATE_SKIP_CONFIG;
                    }
                    Flow::Bottom
                }
                s if s == STATE_DEFINITION_END || s == STATE_DEFINITION_END | STATE_SKIP_CONFIG => {
                    if sys::isblank(buf[start]) {
                        Flow::Next
                    } else {
                        if buf[start] != b'\n' {
                            msg!(MESS_ERROR, file, ":", ln(), ", unexpected text after }\n");
                            state = STATE_SKIP_LINE | (state & STATE_SKIP_CONFIG);
                        } else {
                            state = if state & STATE_SKIP_CONFIG != 0 { STATE_SKIP_CONFIG } else { STATE_DEFAULT };
                        }
                        Flow::Bottom
                    }
                }
                STATE_ERROR => {
                    debug_assert!(newlog != def);
                    let pattern = self.logs.nodes[newlog].pattern.clone();
                    msg!(MESS_ERROR, "found error in ", pattern.as_deref().unwrap_or(b"log config"), ", skipping\n");
                    logerror = 1;
                    state = STATE_SKIP_CONFIG;
                    Flow::Bottom
                }
                s if s == STATE_LOAD_SCRIPT || s == STATE_LOAD_SCRIPT | STATE_SKIP_CONFIG => {
                    let key = isolate_word(&mut start, buf);
                    if key.as_slice() == b"endscript" {
                        if state & STATE_SKIP_CONFIG == 0 {
                            let from = script_start.expect("script start");
                            let mut endtag = start as isize - 9;
                            while endtag > from as isize && buf[endtag as usize] != b'\n' {
                                endtag -= 1;
                            }
                            endtag += 1;
                            let script = crate::bytes::c_str_prefix(&buf[from..endtag as usize]).to_vec();
                            if let Some((id, field)) = script_dest.take() {
                                let l = &mut self.logs.nodes[id];
                                let dest = match field {
                                    ScriptField::Pre => &mut l.pre,
                                    ScriptField::Post => &mut l.post,
                                    ScriptField::First => &mut l.first,
                                    ScriptField::Last => &mut l.last,
                                    ScriptField::Preremove => &mut l.preremove,
                                };
                                *dest = Some(script);
                            }
                            script_start = None;
                        }
                        state = if state & STATE_SKIP_CONFIG != 0 { STATE_SKIP_CONFIG } else { STATE_DEFAULT };
                    } else {
                        state = (if start < length && buf[start] == b'\n' { 0 } else { STATE_SKIP_LINE })
                            | STATE_LOAD_SCRIPT
                            | (state & STATE_SKIP_CONFIG);
                    }
                    Flow::Bottom
                }
                STATE_SKIP_CONFIG => {
                    if buf[start] == b'}' {
                        state = STATE_DEFAULT;
                        self.free_tail_logs(1);
                        newlog = def;
                    } else {
                        let key = isolate_word(&mut start, buf);
                        if matches!(
                            key.as_slice(),
                            b"postrotate" | b"prerotate" | b"firstaction" | b"lastaction" | b"preremove"
                        ) {
                            state = STATE_LOAD_SCRIPT | STATE_SKIP_CONFIG;
                        } else if start < length && buf[start] != b'\n' {
                            // QUIRK: "   }" lands here (isolateWord skipped the
                            // blanks), so the brace is skipped with the line.
                            state = STATE_SKIP_LINE | STATE_SKIP_CONFIG;
                        }
                    }
                    Flow::Bottom
                }
                _ => {
                    msg!(MESS_FATAL, file, format!(": {line_num}: readConfigFile() unknown state: {state:#x}\n"));
                    Flow::Bottom
                }
            };
            match flow {
                Flow::Bottom => {
                    if start < length && buf[start] == b'\n' {
                        line_num += 1;
                    }
                }
                Flow::Next => {}
                Flow::Error => return self.config_error(newlog, def),
            }
            start = start.wrapping_add(1);
        }

        if script_start.is_some() {
            msg!(MESS_ERROR, file, ":prerotate, postrotate or preremove without endscript\n");
            return self.config_error(newlog, def);
        }
        logerror
    }

    /// readConfigFile()'s `error:` label.
    fn config_error(&mut self, newlog: usize, def: usize) -> i32 {
        if newlog != def {
            self.free_tail_logs(1);
        }
        1
    }

    /// The `olddir` checks at the end of a definition: each log's directory
    /// exists, the old directory exists (or is created), and both are on
    /// the same device unless the log is copied.  0, or 1 for `goto error`.
    fn check_olddirs(&mut self, file: &[u8], line_num: i32, newlog: usize) -> i32 {
        let log = self.logs.nodes[newlog].clone();
        let Some(old_dir) = log.old_dir.clone() else { return 0 };
        let ln = line_num.to_string();
        for f in &log.files {
            let dir_name = sys::dirname_of(f);
            let sb_logdir = match sys::stat(&dir_name) {
                Ok(sb) => sb,
                Err(e) => {
                    if !log.has(LOG_FLAG_MISSINGOK) {
                        msg!(
                            MESS_ERROR,
                            file,
                            ":",
                            ln,
                            " error verifying log file path ",
                            dir_name,
                            ": ",
                            sys::strerror(e),
                            "\n"
                        );
                        return 1;
                    }
                    msg!(
                        MESS_DEBUG,
                        file,
                        ":",
                        ln,
                        " verifying log file path failed ",
                        dir_name,
                        ": ",
                        sys::strerror(e),
                        ", log is probably missing, but missingok is set, so this is not an error.\n"
                    );
                    continue;
                }
            };
            let target = if old_dir.first() != Some(&b'/') { bcat!(dir_name, "/", old_dir) } else { old_dir.clone() };
            let sb_olddir = match sys::stat(&target) {
                Ok(sb) => sb,
                Err(e) if e == libc::ENOENT && log.has(LOG_FLAG_OLDDIRCREATE) => {
                    if log.has(LOG_FLAG_SU) && rotate::switch_user(log.su_uid, log.su_gid) != 0 {
                        return 1;
                    }
                    let ret = mkpath(&target, log.olddir_mode, log.olddir_uid, log.olddir_gid);
                    if log.has(LOG_FLAG_SU) && rotate::switch_user_back() != 0 {
                        return 1;
                    }
                    if ret != 0 {
                        return 1;
                    }
                    match sys::stat(&target) {
                        Ok(sb) => sb,
                        Err(e) => {
                            msg!(
                                MESS_ERROR,
                                file,
                                ":",
                                ln,
                                " error verifying created olddir path ",
                                target,
                                ": ",
                                sys::strerror(e),
                                "\n"
                            );
                            return 1;
                        }
                    }
                }
                Err(e) => {
                    msg!(
                        MESS_ERROR,
                        file,
                        ":",
                        ln,
                        " error verifying olddir path ",
                        target,
                        ": ",
                        sys::strerror(e),
                        "\n"
                    );
                    return 1;
                }
            };
            if sb_logdir.st_dev != sb_olddir.st_dev
                && !log.has(LOG_FLAG_COPYTRUNCATE | LOG_FLAG_COPY | LOG_FLAG_TMPFILENAME)
            {
                msg!(
                    MESS_ERROR,
                    file,
                    ":",
                    ln,
                    " olddir ",
                    old_dir,
                    " and log file ",
                    f,
                    " are on different devices\n"
                );
                return 1;
            }
        }
        0
    }
}

/// `parseGlobString()`: the text up to `{` (comments dropped, trailing
/// blanks trimmed), leaving `*pos` on the `{`.
fn parse_glob_string(file: &[u8], line: i32, buf: &[u8], pos: &mut usize) -> Option<Vec<u8>> {
    #[derive(PartialEq)]
    enum S {
        Init,
        Data,
        Comment,
    }
    let mut out: Vec<u8> = Vec::new();
    let mut state = S::Init;
    while *pos < buf.len() && buf[*pos] != 0 {
        let c = buf[*pos];
        match state {
            S::Init => {
                if c == b'#' {
                    state = S::Comment;
                } else if !sys::isspace(c) {
                    state = S::Data;
                }
            }
            _ => {
                if c == b'\n' {
                    state = S::Init;
                }
            }
        }
        if state == S::Comment {
            *pos += 1;
            continue;
        }
        match c {
            b'}' => {
                msg!(MESS_ERROR, file, ":", line.to_string(), " unexpected } (missing previous '{')\n");
                return None;
            }
            b'{' => {
                let mut i = out.len();
                while i > 1 && sys::isspace(out[i - 1]) {
                    i -= 1;
                }
                out.truncate(i);
                return Some(out);
            }
            _ => {}
        }
        out.push(c);
        *pos += 1;
    }
    msg!(MESS_ERROR, file, ":", line.to_string(), " missing '{' after log files definition\n");
    None
}

/// `readAllConfigPaths()`: (error, logs).
pub fn read_all_config_paths(paths: &[Vec<u8>]) -> (i32, Logs) {
    let mut p = Parser {
        logs: Logs { nodes: vec![LogInfo::initial()], first: None, last: Slot::Head, num_logs: 0 },
        taboo: DEF_TABOO_EXTS.iter().map(|e| bcat!("*", e)).collect(),
        recursion_depth: 0,
    };
    let mut result = 0;
    for path in paths {
        if p.read_config_path(path, DEF_CONFIG) != 0 {
            result = 1;
        }
    }
    (result, p.logs)
}
