//! Differential test harness: run C logrotate and logrotate-rs in the same
//! sandbox and compare everything observable.
//!
//! # The sandbox
//!
//! Each run gets a fresh directory `T` under `target/difftest/<case>/<impl>`
//! and runs in new user and mount namespaces.  Ids are mapped with the
//! setuid `newuidmap`/`newgidmap` helpers and the invoking user's
//! subordinate ranges (`/etc/subuid`, `/etc/subgid`): namespace id 0 is the
//! invoking user, and ids 1-65536 are the first subordinate ids.  So the
//! sandbox has a real root and real other users.
//!
//! The sandbox's `/etc` is `T/etc`: a farm of symlinks to the real `/etc`
//! (bound aside at `T/.realetc`), with the harness's own `passwd`, `group`
//! and `popt`.  Also bound: `T/tmp` over `/tmp`, `T/misc` over
//! `/var/lib/misc` (the default state file's directory) and `T/devlog` (a
//! socket the harness reads) over `/dev/log`.  `@T@` in arguments,
//! environment and files stands for `T`.  The clock is the LD_PRELOAD shim
//! in tests/shim/.
//!
//! # What is compared
//!
//! The exit status, stdout, stderr, the syslog messages, and every file under
//! `T` (type, mode, owner, contents), with `T` and the pid normalized.  The
//! snapshot and the cleanup run as root inside the sandbox, so files owned
//! by other users are covered too.
//!
//! The C program comes from `$LOGROTATE_REFERENCE` (a build tree), by
//! default `../logrotate` (tools/build-reference.sh builds it there).

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::ffi::{CString, c_char, c_int};
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// 2025-11-11 14:00:00 UTC, a Tuesday.
pub const DEFAULT_TIME: (i64, i64) = (1_762_869_600, 0);

pub const PASSWD: &str = "\
root:x:0:0:root:@T@/home/root:/bin/sh
alice:x:1001:1001:Alice:@T@/home/alice:/bin/sh
bob:x:1002:1002:Bob:@T@/home/bob:/bin/sh
syslog:x:105:4:syslog:/var/empty:/sbin/nologin
nobody:x:65534:65534:nobody:/var/empty:/sbin/nologin
";
pub const GROUP: &str = "\
root:x:0:
adm:x:4:syslog
users:x:100:alice,bob
utmp:x:406:
alice:x:1001:
bob:x:1002:
nobody:x:65534:
";

/// Who the program runs as.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum As {
    Root,
    /// A test user: (uid, gid, supplementary groups).
    User(u32, u32, &'static [u32]),
}

pub const ALICE: As = As::User(1001, 1001, &[1001, 100]);
pub const BOB: As = As::User(1002, 1002, &[1002, 100]);

/// What to make before the run.
#[derive(Debug, Clone)]
pub enum Node {
    /// A file: contents, mode, owner, group.
    File(Vec<u8>, u32, u32, u32),
    /// A directory: mode, owner, group.
    Dir(u32, u32, u32),
    /// A symbolic link to the target.
    Symlink(String),
    /// A FIFO, mode.
    Fifo(u32),
    /// A hard link to another path under T.
    Hardlink(String),
}

#[derive(Debug, Clone)]
pub struct Case {
    pub name: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Paths under T, made in order.
    pub nodes: Vec<(String, Node)>,
    pub time: (i64, i64),
    pub user: As,
    /// Modification times for paths under T (files default to an hour
    /// before the case's time).
    pub mtimes: Vec<(String, i64)>,
}

impl Case {
    /// A case running `logrotate ARGS`.
    pub fn new(name: &str, args: &[&str]) -> Case {
        Case {
            name: name.to_string(),
            args: args.iter().map(|s| s.to_string()).collect(),
            env: vec![("TZ".into(), "UTC".into()), ("PATH".into(), "/usr/bin:/bin".into())],
            nodes: Vec::new(),
            time: DEFAULT_TIME,
            user: As::Root,
            mtimes: Vec::new(),
        }
    }

    pub fn env(mut self, k: &str, v: &str) -> Case {
        self.env.retain(|(kk, _)| kk != k);
        self.env.push((k.into(), v.into()));
        self
    }

    pub fn no_env(mut self, k: &str) -> Case {
        self.env.retain(|(kk, _)| kk != k);
        self
    }

    /// A root-owned file.
    pub fn file(self, path: &str, contents: &[u8], mode: u32) -> Case {
        self.file_owned(path, contents, mode, 0, 0)
    }

    pub fn file_owned(mut self, path: &str, contents: &[u8], mode: u32, uid: u32, gid: u32) -> Case {
        self.nodes.push((path.into(), Node::File(contents.to_vec(), mode, uid, gid)));
        self
    }

    /// A root-owned config file, mode 0644.
    pub fn conf(self, path: &str, contents: &str) -> Case {
        self.file(path, contents.as_bytes(), 0o644)
    }

    pub fn dir(self, path: &str, mode: u32) -> Case {
        self.dir_owned(path, mode, 0, 0)
    }

    pub fn dir_owned(mut self, path: &str, mode: u32, uid: u32, gid: u32) -> Case {
        self.nodes.push((path.into(), Node::Dir(mode, uid, gid)));
        self
    }

    pub fn symlink(mut self, path: &str, target: &str) -> Case {
        self.nodes.push((path.into(), Node::Symlink(target.into())));
        self
    }

    /// A hard link at `path` to `target` (both under T).
    pub fn hardlink(mut self, path: &str, target: &str) -> Case {
        self.nodes.push((path.into(), Node::Hardlink(target.into())));
        self
    }

    /// Drop a node added earlier (e.g. a default one).
    pub fn no_file(mut self, path: &str) -> Case {
        self.nodes.retain(|(p, _)| p != path);
        self
    }

    pub fn fifo(mut self, path: &str, mode: u32) -> Case {
        self.nodes.push((path.into(), Node::Fifo(mode)));
        self
    }

    pub fn time(mut self, sec: i64) -> Case {
        self.time = (sec, 0);
        self
    }

    /// Set a file's modification time (seconds).
    pub fn mtime(mut self, path: &str, t: i64) -> Case {
        self.mtimes.push((path.into(), t));
        self
    }

    pub fn run_as(mut self, user: As) -> Case {
        self.user = user;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub status: String,
    pub stdout: String,
    pub stderr: String,
    pub syslog: String,
    pub files: BTreeMap<String, String>,
}

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn reference_binary() -> PathBuf {
    let dir =
        std::env::var_os("LOGROTATE_REFERENCE").map_or_else(|| manifest_dir().join("../logrotate"), PathBuf::from);
    dir.join("logrotate")
}

pub fn rust_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_logrotate"))
}

fn shim() -> &'static Path {
    static SHIM: OnceLock<PathBuf> = OnceLock::new();
    SHIM.get_or_init(|| {
        let dir = manifest_dir().join("target/difftest");
        std::fs::create_dir_all(&dir).unwrap();
        let out = dir.join("faketime.so");
        let tmp = dir.join(format!("faketime.so.{}", std::process::id()));
        let st = Command::new("cc")
            .args(["-shared", "-fPIC", "-O2", "-o"])
            .arg(&tmp)
            .arg(manifest_dir().join("tests/shim/faketime.c"))
            .arg("-ldl")
            .status()
            .expect("run cc");
        assert!(st.success(), "building the faketime shim failed");
        std::fs::rename(&tmp, &out).unwrap();
        out
    })
}

/// Where every run's `T` appears inside its sandbox (each run binds its own
/// directory there, in its own mount namespace), so the C and Rust runs see
/// the same paths: logrotate's state file order depends on them.
pub fn fixed_root() -> PathBuf {
    let p = manifest_dir().join("target/difftest/.root");
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// `@T@` as the program sees it.
fn subst(s: &str, _t: &Path) -> String {
    s.replace("@T@", fixed_root().to_str().unwrap())
}

/// `@T@` as the harness sees it, outside the sandbox.
fn subst_real(s: &str, t: &Path) -> String {
    s.replace("@T@", t.to_str().unwrap())
}

fn subst_bytes(s: &[u8], _t: &Path) -> Vec<u8> {
    let pat = b"@T@";
    let root = fixed_root();
    let rep = root.to_str().unwrap().as_bytes();
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        if s[i..].starts_with(pat) {
            out.extend_from_slice(rep);
            i += pat.len();
        } else {
            out.push(s[i]);
            i += 1;
        }
    }
    out
}

fn cstr(s: impl AsRef<[u8]>) -> CString {
    CString::new(s.as_ref().to_vec()).unwrap()
}

fn path_c(p: &Path) -> CString {
    cstr(p.to_str().unwrap())
}

/// The invoking user's first subordinate id, from /etc/subuid or /etc/subgid.
fn subordinate(file: &str, name: &str, id: u32) -> u32 {
    let text = std::fs::read_to_string(file).unwrap_or_default();
    for line in text.lines() {
        let f: Vec<&str> = line.split(':').collect();
        if f.len() == 3 && (f[0] == name || f[0] == id.to_string()) && f[2].parse::<u32>().unwrap_or(0) >= 65536 {
            return f[1].parse().unwrap();
        }
    }
    panic!("no subordinate range of 65536 ids for {name} in {file}: the sandbox needs one");
}

/// What the child does between fork and exec, prepared beforehand.
struct Spawn {
    binds: Vec<(CString, CString)>,
    /// (path, owner, group, mode) fixups made as root in the sandbox
    /// (lchown, and chmod unless the mode is u32::MAX).
    chowns: Vec<(CString, u32, u32, u32)>,
    user: As,
    dir: CString,
    stdio: [c_int; 3],
    exe: CString,
    argv: Vec<CString>,
    envp: Vec<CString>,
}

fn pipe() -> (c_int, c_int) {
    let mut fds = [0 as c_int; 2];
    // SAFETY: fds has room for two ints.
    assert_eq!(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
    (fds[0], fds[1])
}

/// fork(); the child enters the sandbox and execs.  Returns the pid once
/// the child has reached exec.
fn spawn_in_sandbox(s: Spawn) -> u32 {
    // SAFETY: getuid/getgid have no preconditions.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    let name = std::env::var("USER").unwrap_or_default();
    let subuid = subordinate("/etc/subuid", &name, uid);
    let subgid = subordinate("/etc/subgid", &name, gid);

    let mut argv: Vec<*const c_char> = s.argv.iter().map(|a| a.as_ptr()).collect();
    argv.push(std::ptr::null());
    let mut envp: Vec<*const c_char> = s.envp.iter().map(|a| a.as_ptr()).collect();
    envp.push(std::ptr::null());
    let gs: Vec<libc::gid_t> = match s.user {
        As::User(_, _, groups) => groups.to_vec(),
        As::Root => Vec::new(),
    };
    let (up_r, up_w) = pipe();
    let (go_r, go_w) = pipe();
    let root = cstr("/");

    // SAFETY: between fork and exec the child only makes async-signal-safe
    // system calls on memory prepared above, then execs or _exits.
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0, "fork failed");
    if pid == 0 {
        // SAFETY: as above.
        unsafe {
            libc::close(go_w);
            libc::close(up_r);
            if libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNS) != 0 {
                libc::_exit(120);
            }
            libc::write(up_w, b"u".as_ptr().cast(), 1);
            let mut b = [0u8; 1];
            if libc::read(go_r, b.as_mut_ptr().cast(), 1) != 1 {
                libc::_exit(121);
            }
            let none = std::ptr::null::<c_char>();
            if libc::mount(none, root.as_ptr(), none, libc::MS_REC | libc::MS_PRIVATE, std::ptr::null()) != 0 {
                libc::_exit(122);
            }
            for (src, dst) in &s.binds {
                if libc::mount(src.as_ptr(), dst.as_ptr(), none, libc::MS_BIND | libc::MS_REC, std::ptr::null()) != 0 {
                    libc::_exit(123);
                }
            }
            for (path, owner, group, mode) in &s.chowns {
                if libc::lchown(path.as_ptr(), *owner, *group) != 0
                    || (*mode != u32::MAX && libc::chmod(path.as_ptr(), *mode) != 0)
                {
                    libc::_exit(124);
                }
            }
            if let As::User(u, g, _) = s.user
                && (libc::setgroups(gs.len(), gs.as_ptr()) != 0 || libc::setgid(g) != 0 || libc::setuid(u) != 0)
            {
                libc::_exit(125);
            }
            for (i, fd) in s.stdio.iter().enumerate() {
                libc::dup2(*fd, i as c_int);
            }
            if libc::chdir(s.dir.as_ptr()) != 0 {
                libc::_exit(126);
            }
            // up_w is close-on-exec: EOF tells the parent we reached exec.
            libc::execve(s.exe.as_ptr(), argv.as_ptr(), envp.as_ptr());
            libc::_exit(127);
        }
    }
    // SAFETY: closing our copies of the child's ends.
    unsafe {
        libc::close(go_r);
        libc::close(up_w);
    }
    let mut b = [0u8; 1];
    // SAFETY: reading one byte into b.
    let n = unsafe { libc::read(up_r, b.as_mut_ptr().cast(), 1) };
    if n != 1 {
        panic!("sandbox child failed before unshare ({})", wait(pid as u32, Duration::from_secs(5)));
    }
    for (tool, host, sub) in [("newuidmap", uid, subuid), ("newgidmap", gid, subgid)] {
        let st = Command::new(tool)
            .arg(pid.to_string())
            .args(["0", &host.to_string(), "1", "1", &sub.to_string(), "65536"])
            .status()
            .unwrap_or_else(|e| panic!("{tool}: {e}"));
        assert!(st.success(), "{tool} failed");
    }
    // SAFETY: writing one byte from a static buffer, then closing.
    unsafe {
        libc::write(go_w, b"g".as_ptr().cast(), 1);
        libc::close(go_w);
    }
    // The child execs (close-on-exec makes this EOF) or exits.
    // SAFETY: reading into b; then closing.
    unsafe {
        libc::read(up_r, b.as_mut_ptr().cast(), 1);
        libc::close(up_r);
    }
    pid as u32
}

/// waitpid with a timeout; the status as text.
fn wait(pid: u32, timeout: Duration) -> String {
    let deadline = Instant::now() + timeout;
    loop {
        let mut st: c_int = 0;
        // SAFETY: st is a valid out-pointer.
        let r = unsafe { libc::waitpid(pid as i32, &mut st, libc::WNOHANG) };
        if r == pid as i32 {
            return if libc::WIFEXITED(st) {
                format!("exit {}", libc::WEXITSTATUS(st))
            } else if libc::WIFSIGNALED(st) {
                format!("signal {}", libc::WTERMSIG(st))
            } else {
                "unknown".into()
            };
        }
        if Instant::now() > deadline {
            // SAFETY: killing and reaping our own child.
            unsafe {
                libc::kill(pid as i32, libc::SIGKILL);
                libc::waitpid(pid as i32, &mut st, 0);
            }
            return "timeout".into();
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// The binds every sandbox gets.
fn binds(t: &Path) -> Vec<(CString, CString)> {
    let mut b = vec![
        (path_c(t), path_c(&fixed_root())),
        (cstr("/etc"), path_c(&t.join(".realetc"))),
        (path_c(&t.join("etc")), cstr("/etc")),
        (path_c(&t.join("tmp")), cstr("/tmp")),
        (path_c(&t.join("misc")), cstr("/var/lib/misc")),
    ];
    if Path::new("/dev/log").exists() {
        b.push((path_c(&t.join("devlog")), cstr("/dev/log")));
    }
    b
}

/// Run a shell script as root in a sandbox over `t`, for snapshots and
/// cleanup, which must see files owned by the sandbox's other users.
fn root_shell(t: &Path, script: &str) -> Vec<u8> {
    use std::os::fd::AsRawFd;
    let out = t.with_extension("rootsh.out");
    let outf = std::fs::File::create(&out).unwrap();
    let null = std::fs::File::open("/dev/null").unwrap();
    // No binds: it works on real paths, and must work on a half-removed tree.
    let pid = spawn_in_sandbox(Spawn {
        binds: Vec::new(),
        chowns: Vec::new(),
        user: As::Root,
        dir: cstr("/"),
        stdio: [null.as_raw_fd(), outf.as_raw_fd(), outf.as_raw_fd()],
        exe: cstr("/bin/sh"),
        argv: vec![cstr("sh"), cstr("-c"), cstr(script)],
        envp: vec![cstr("PATH=/usr/bin:/bin"), cstr("LC_ALL=C")],
    });
    let st = wait(pid, Duration::from_secs(30));
    let data = std::fs::read(&out).unwrap();
    let _ = std::fs::remove_file(&out);
    assert_eq!(st, "exit 0", "root shell failed: {script}\n{}", String::from_utf8_lossy(&data));
    data
}

/// Remove a previous run's directory, including files sandbox users made.
fn clean(t: &Path) {
    if !t.exists() || std::fs::remove_dir_all(t).is_ok() {
        return;
    }
    for d in ["etc", ".realetc", "tmp", "misc"] {
        std::fs::create_dir_all(t.join(d)).ok();
    }
    root_shell(t, &format!("chown -R 0:0 '{0}' 2>/dev/null; chmod -R u+rwX '{0}' 2>/dev/null; exit 0", t.display()));
    std::fs::remove_dir_all(t).unwrap_or_else(|e| panic!("can't clean {}: {e}", t.display()));
}

/// Replace `T` with `@T@`, the program's pid with `@PID@`, and the pids
/// children report in "(pid N)" with `@CHILD@`.
pub fn normalize(s: &str, t: &Path, pid: u32) -> String {
    let s = s.replace(t.to_str().unwrap(), "@T@").replace(fixed_root().to_str().unwrap(), "@T@");
    let s = {
        let mut out = String::new();
        let mut rest = s.as_str();
        while let Some(k) = rest.find("(pid ") {
            out.push_str(&rest[..k + 5]);
            rest = &rest[k + 5..];
            let n = rest.bytes().take_while(u8::is_ascii_digit).count();
            if n > 0 && rest[n..].starts_with(')') && rest[..n] != pid.to_string() {
                out.push_str("@CHILD@");
                rest = &rest[n..];
            }
        }
        out.push_str(rest);
        out
    };
    let pid_s = pid.to_string();
    let mut out = String::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() && (i == 0 || !b[i - 1].is_ascii_alphanumeric()) {
            let j = i + b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
            let after_ok = j == b.len() || !b[j].is_ascii_alphanumeric();
            out.push_str(if after_ok && s[i..j] == pid_s { "@PID@" } else { &s[i..j] });
            i = j;
        } else {
            let ch = s[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

/// The tree under `t`, as root in the sandbox: type, mode, owner, contents.
/// Regular files show their modification time ("now" for times the run
/// set, which are real time), extended ACLs, and their contents, with
/// compressed files shown decompressed (gzip headers hold times).
fn snapshot(t: &Path, pid: u32) -> BTreeMap<String, String> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
    let script = format!(
        "cd '{}' || exit 1
         find . -mindepth 1 \\( -path ./.realetc -o -path ./etc -o -name devlog -o -name .faketime \
            -o -name stdout -o -name stderr \\) -prune -o -print | LC_ALL=C sort | while read -r f; do
           printf '\\001%s\\002%s' \"$f\" \"$(stat -c '%F %a %u:%g' \"$f\")\"
           if [ -f \"$f\" ] && [ ! -L \"$f\" ]; then
             m=$(stat -c %Y \"$f\"); [ \"$m\" -gt {recent} ] && m=now; printf ' mtime %s' \"$m\"
             getfacl -c --skip-base \"$f\" 2>/dev/null | grep -v '^$' | tr '\\n' ' '
           fi
           printf '\\002'
           if [ -L \"$f\" ]; then readlink \"$f\"
           elif [ -f \"$f\" ]; then
             case \"$f\" in
               *.gz) printf 'gzip:'; gzip -dc < \"$f\" 2>&1 ;;
               *.xz) printf 'xz:'; xz -dc < \"$f\" 2>&1 ;;
               *.bz2) printf 'bzip2:'; bzip2 -dc < \"$f\" 2>&1 ;;
               *.zst) printf 'zstd:'; zstd -qdc < \"$f\" 2>&1 ;;
               *) cat \"$f\" ;;
             esac
           fi
         done; exit 0",
        t.display(),
        recent = now - 86400
    );
    let raw = root_shell(t, &script);
    let text = String::from_utf8_lossy(&raw).to_string();
    let mut out = BTreeMap::new();
    for rec in text.split('\u{1}').skip(1) {
        let mut parts = rec.splitn(3, '\u{2}');
        let (name, meta, data) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        out.insert(normalize(name, t, pid), normalize(&format!("{meta}\n{data}"), t, pid));
    }
    out
}

/// Build `T/etc`: symlinks to the real /etc (seen at `T/.realetc` inside
/// the sandbox), plus the harness's own files.
fn build_etc(t: &Path) {
    let etc = t.join("etc");
    std::fs::create_dir_all(&etc).unwrap();
    let own = ["passwd", "group", "shadow", "gshadow", "popt", "popt.d"];
    for ent in std::fs::read_dir("/etc").unwrap().flatten() {
        let name = ent.file_name();
        if own.contains(&name.to_str().unwrap_or("")) {
            continue;
        }
        std::os::unix::fs::symlink(t.join(".realetc").join(&name), etc.join(&name)).unwrap();
    }
    std::fs::write(etc.join("passwd"), subst(PASSWD, t)).unwrap();
    std::fs::write(etc.join("group"), GROUP).unwrap();
}

pub fn run(case: &Case, bin: &Path, label: &str) -> Outcome {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::PermissionsExt;
    let t = manifest_dir().join("target/difftest").join(&case.name).join(label);
    clean(&t);
    for d in [".realetc", "tmp", "misc", "home/root", "home/alice", "home/bob"] {
        std::fs::create_dir_all(t.join(d)).unwrap();
    }
    build_etc(&t);
    let mut chowns = vec![
        (path_c(&t.join("home/alice")), 1001, 1001, 0o755),
        (path_c(&t.join("home/bob")), 1002, 1002, 0o755),
        (path_c(&t.join("tmp")), 0, 0, 0o1777),
    ];
    for (path, node) in &case.nodes {
        let p = t.join(subst_real(path, &t));
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        match node {
            Node::File(data, mode, uid, gid) => {
                std::fs::write(&p, subst_bytes(data, &t)).unwrap();
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(*mode & 0o777)).unwrap();
                if *uid != 0 || *gid != 0 || *mode > 0o777 {
                    chowns.push((path_c(&p), *uid, *gid, *mode));
                }
            }
            Node::Dir(mode, uid, gid) => {
                std::fs::create_dir_all(&p).unwrap();
                // Made writable now; the final mode is applied in the sandbox,
                // after the files inside it exist.
                chowns.push((path_c(&p), *uid, *gid, *mode));
            }
            Node::Symlink(target) => {
                std::os::unix::fs::symlink(subst(target, &t), &p).unwrap();
            }
            Node::Hardlink(target) => {
                std::fs::hard_link(t.join(subst_real(target, &t)), &p).unwrap();
            }
            Node::Fifo(mode) => {
                let c = path_c(&p);
                // SAFETY: c is NUL-terminated.
                assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), *mode) }, 0);
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(*mode)).unwrap();
            }
        }
    }
    // Fixed modification times: an hour before the case's time, or as set.
    let set_mtime = |p: &Path, t: i64| {
        let c = path_c(p);
        let ts = [libc::timespec { tv_sec: t, tv_nsec: 0 }, libc::timespec { tv_sec: t, tv_nsec: 0 }];
        // SAFETY: c is NUL-terminated and ts holds two timespecs.
        unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), ts.as_ptr(), libc::AT_SYMLINK_NOFOLLOW) };
    };
    for (path, node) in &case.nodes {
        if let Node::File(..) = node {
            set_mtime(&t.join(subst_real(path, &t)), case.time.0 - 3600);
        }
    }
    for (path, mt) in &case.mtimes {
        set_mtime(&t.join(subst_real(path, &t)), *mt);
    }
    // Directories' modes last, innermost first, so the files inside them
    // could be made.
    let (dirs, mut rest): (Vec<_>, Vec<_>) = chowns
        .into_iter()
        .partition(|(p, ..)| Path::new(std::ffi::OsStr::new(std::str::from_utf8(p.as_bytes()).unwrap())).is_dir());
    let mut dirs = dirs;
    dirs.sort_by_key(|(p, ..)| std::cmp::Reverse(p.as_bytes().len()));
    rest.extend(dirs);
    let chowns = rest;

    std::fs::write(t.join(".faketime"), format!("{} {}\n", case.time.0, case.time.1)).unwrap();

    let syslog = UnixDatagram::bind(t.join("devlog")).unwrap();
    // A datagram socket queues only a few messages before senders block, so
    // read them as they come.
    syslog.set_read_timeout(Some(Duration::from_millis(50))).unwrap();
    let stop_reader = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let reader = {
        let sock = syslog.try_clone().unwrap();
        let stop = stop_reader.clone();
        std::thread::spawn(move || {
            let mut msgs: Vec<Vec<u8>> = Vec::new();
            let mut buf = [0u8; 65536];
            loop {
                match sock.recv(&mut buf) {
                    Ok(n) => msgs.push(buf[..n].to_vec()),
                    Err(_) if stop.load(std::sync::atomic::Ordering::SeqCst) => break,
                    Err(_) => {}
                }
            }
            msgs
        })
    };
    std::fs::set_permissions(t.join("devlog"), std::fs::Permissions::from_mode(0o666)).unwrap();
    let stdin = std::fs::File::open("/dev/null").unwrap();
    let stdout = std::fs::File::create(t.join("stdout")).unwrap();
    let stderr = std::fs::File::create(t.join("stderr")).unwrap();

    let mut envp = vec![
        cstr(format!("LD_PRELOAD={}", shim().display())),
        cstr(format!("TEST_FAKETIME={}", t.join(".faketime").display())),
    ];
    for (k, v) in &case.env {
        envp.push(cstr(format!("{k}={}", subst(v, &t))));
    }
    let mut argv = vec![cstr("logrotate")];
    argv.extend(case.args.iter().map(|a| cstr(subst(a, &t))));

    let pid = spawn_in_sandbox(Spawn {
        binds: binds(&t),
        chowns,
        user: case.user,
        dir: path_c(&fixed_root()),
        stdio: [stdin.as_raw_fd(), stdout.as_raw_fd(), stderr.as_raw_fd()],
        exe: path_c(bin),
        argv,
        envp,
    });
    let status = wait(pid, Duration::from_secs(20));
    assert_ne!(status, "timeout", "{}: {label}: did not exit", case.name);
    // 120-127: the sandbox child failed before or at exec (see spawn_in_sandbox).
    if let Some(code) = status.strip_prefix("exit ").and_then(|c| c.parse::<i32>().ok()) {
        assert!(!(120..=127).contains(&code), "{}: {label}: sandbox setup failed ({status})", case.name);
    }

    stop_reader.store(true, std::sync::atomic::Ordering::SeqCst);
    let mut log = String::new();
    for raw in reader.join().unwrap() {
        // "<pri>Mmm dd hh:mm:ss ident[pid]: msg": glibc's syslog() reads the
        // clock internally (not through the shim), so drop the timestamp.
        let msg = String::from_utf8_lossy(&raw).to_string();
        let msg = match (msg.find('>'), msg.len() > 20) {
            (Some(gt), true) if msg.is_char_boundary(gt + 16) => format!("{}{}", &msg[..=gt], &msg[gt + 16..]),
            _ => msg,
        };
        log += &normalize(&msg, &t, pid);
        log.push('\n');
    }
    let rd = |n: &str| normalize(&String::from_utf8_lossy(&std::fs::read(t.join(n)).unwrap()), &t, pid);
    let files = snapshot(&t, pid);
    Outcome { status, stdout: rd("stdout"), stderr: rd("stderr"), syslog: log, files }
}

/// Run a case with C logrotate and with the port; assert identical outcomes.
pub fn check(case: Case) -> Outcome {
    let reference = reference_binary();
    assert!(
        reference.exists(),
        "reference logrotate not found at {} (run tools/build-reference.sh or set LOGROTATE_REFERENCE)",
        reference.display()
    );
    let c = run(&case, &reference, "c");
    // Labels of equal length: some messages print path lengths.
    let r = run(&case, &rust_binary(), "r");
    if c != r {
        let mut msg = format!("case {}: outcomes differ\n", case.name);
        for (what, a, b) in [
            ("status", &c.status, &r.status),
            ("stdout", &c.stdout, &r.stdout),
            ("stderr", &c.stderr, &r.stderr),
            ("syslog", &c.syslog, &r.syslog),
        ] {
            if a != b {
                msg += &format!("{what}:\n--- C\n{a}\n--- Rust\n{b}\n");
            }
        }
        for k in c.files.keys().chain(r.files.keys()).collect::<std::collections::BTreeSet<_>>() {
            let (a, b) = (c.files.get(k), r.files.get(k));
            if a != b {
                msg += &format!(
                    "file {k}:\n--- C\n{}\n--- Rust\n{}\n",
                    a.map_or("(missing)", |s| s),
                    b.map_or("(missing)", |s| s)
                );
            }
        }
        panic!("{msg}");
    }
    if std::env::var_os("LOGROTATE_DIFF_SHOW").is_some() {
        eprintln!("== {} ==\n{c:#?}", case.name);
    }
    c
}
