//! OS differences in one place (Windows-native engine, 2026-10-05): the home
//! directory, the bash that runs shell steps, PATH prepends and killing a
//! process tree.  Everything else in iter5 is written for a POSIX shell, so on
//! Windows the shell is Git for Windows' bash — never `C:\Windows\System32\
//! bash.exe`, which would run the step inside WSL against Linux paths.

use std::path::{Path, PathBuf};
use std::process::Command;

/// `$HOME`, else `%USERPROFILE%` (Windows sets only the latter).
pub fn home_dir() -> Option<PathBuf> {
    ["HOME", "USERPROFILE"]
        .iter()
        .filter_map(|k| std::env::var_os(k))
        .find(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// The bash binary shell steps run under: `$ITER_BASH` when set; on Windows
/// Git for Windows' `bin\bash.exe` (found beside the `git` on PATH, then in
/// the usual install dirs); elsewhere plain `bash`.
pub fn bash_path() -> PathBuf {
    if let Some(b) = std::env::var_os("ITER_BASH").filter(|b| !b.is_empty()) {
        return PathBuf::from(b);
    }
    #[cfg(windows)]
    {
        if let Some(b) = git_bash() {
            return b;
        }
    }
    PathBuf::from("bash")
}

/// A `Command` for `bash` (see `bash_path`).
pub fn bash() -> Command {
    Command::new(bash_path())
}

#[cfg(windows)]
fn git_bash() -> Option<PathBuf> {
    // git.exe lives in <Git>\cmd (installer) or <Git>\bin / <Git>\mingw64\bin
    let from_path = std::env::var_os("PATH").into_iter().flat_map(|p| std::env::split_paths(&p).collect::<Vec<_>>()).filter(|d| d.join("git.exe").is_file()).flat_map(|d| {
        let mut v = Vec::new();
        for up in [d.parent(), d.parent().and_then(Path::parent)].into_iter().flatten() {
            v.push(up.join("bin").join("bash.exe"));
        }
        v
    });
    let installs = ["ProgramFiles", "ProgramW6432", "LOCALAPPDATA"].iter().filter_map(|k| std::env::var_os(k)).flat_map(|base| {
        let base = PathBuf::from(base);
        [base.join("Git").join("bin").join("bash.exe"), base.join("Programs").join("Git").join("bin").join("bash.exe")]
    });
    from_path.chain(installs).find(|p| p.is_file())
}

/// `$MSYS` for the engine's children (2026-10-08): Git Bash's `ln -s` copies
/// the whole target unless MSYS asks for real symlinks, and an agent symlinking
/// a multi-GB repo into a temp dir saturated the disk.  `winsymlinks:native`
/// makes a real symlink (Developer Mode or admin) and still falls back to a
/// copy when Windows refuses.  An explicit `winsymlinks:` setting is kept.
/// None when nothing needs to change.
pub fn msys_with_symlinks(current: Option<&str>) -> Option<String> {
    let cur = current.unwrap_or("").trim();
    if cur.split_whitespace().any(|o| o.starts_with("winsymlinks")) {
        return None;
    }
    Some(if cur.is_empty() { "winsymlinks:native".to_string() } else { format!("{cur} winsymlinks:native") })
}

/// Process-environment defaults every child (shell step, claude, its Bash
/// tool) inherits; Windows only (see `msys_with_symlinks`).
///
/// # Safety
/// Writes the process environment: call from `main` before any thread starts.
pub unsafe fn init_child_env() {
    #[cfg(windows)]
    {
        let cur = std::env::var("MSYS").ok();
        if let Some(v) = msys_with_symlinks(cur.as_deref()) {
            unsafe { std::env::set_var("MSYS", v) };
        }
    }
}

/// `std::fs::canonicalize` without Windows' verbatim prefix: `\\?\C:\x`
/// comes back as `C:\x`.  A verbatim path turns off `/` normalisation (so
/// `top.join("a/b")` no longer opens) and Git Bash cannot read it.  UNC and
/// other verbatim forms are returned as canonicalize gave them.
pub fn canonicalize(p: &Path) -> std::io::Result<PathBuf> {
    let c = std::fs::canonicalize(p)?;
    #[cfg(windows)]
    {
        let s = c.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            let b = rest.as_bytes();
            if b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':' {
                return Ok(PathBuf::from(rest));
            }
        }
    }
    Ok(c)
}

/// `PATH` with `dir` in front, joined with the OS separator (`;` on Windows).
pub fn path_with(dir: &Path) -> String {
    let mut dirs = vec![dir.to_path_buf()];
    if let Some(p) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&p));
    }
    std::env::join_paths(dirs).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Kill `pid` and everything it started.  Unix: the process group `pid`
/// leads (callers spawn with `process_group(0)`).  Windows: `taskkill /T`
/// walks the child tree.  Best effort; the caller still kills the child.
pub fn kill_tree(pid: u32) {
    #[cfg(unix)]
    {
        let _ = Command::new("kill").args(["-TERM", "--", &format!("-{pid}")]).status();
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .status();
    }
}

/// A spawned child and everything it starts (2026-10-08).  `kill_tree` alone
/// misses orphans on Windows: `taskkill /T` follows parent links, so a test
/// script whose bash parent had already exited kept copying gigabytes for
/// hours after its agent session was gone.  On Windows the child goes into a
/// job object; every descendant joins it however deep, and dropping the tree
/// (`KILL_ON_JOB_CLOSE`) kills whatever is still running — after a timeout, a
/// stop, a normal exit, or the engine itself dying.  A descendant started in
/// the first instants, before `adopt` assigns the child, is outside the job
/// (std can't spawn suspended); `kill` still runs `kill_tree` for those.
/// Unix: the child leads its own process group (`process_group(0)`), and
/// `kill` signals the group; drop does nothing.
pub struct ProcessTree {
    pid: u32,
    #[cfg(windows)]
    job: Option<win::Handle>,
}

impl ProcessTree {
    pub fn adopt(child: &std::process::Child) -> ProcessTree {
        ProcessTree {
            pid: child.id(),
            #[cfg(windows)]
            job: win::job_for(child),
        }
    }

    /// Kill the child and every descendant now.  Best effort.
    pub fn kill(&self) {
        #[cfg(windows)]
        {
            if let Some(job) = &self.job {
                win::terminate(job);
            }
        }
        kill_tree(self.pid);
    }
}

#[cfg(windows)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        if let Some(job) = self.job.take() {
            win::close(job); // KILL_ON_JOB_CLOSE: the leftovers die here
        }
    }
}

/// Job objects through raw kernel32 calls (no windows-sys dependency).
#[cfg(windows)]
mod win {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;

    pub struct Handle(*mut c_void);
    // a job handle is a kernel object reference, usable from any thread
    unsafe impl Send for Handle {}

    #[repr(C)]
    #[derive(Default)]
    struct BasicLimit {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimit {
        basic: BasicLimit,
        io_counters: [u64; 6],
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
    // deliberately without JOB_OBJECT_LIMIT_BREAKAWAY_OK: Git Bash (MSYS)
    // breaks its children away from any job that allows it, which is exactly
    // the leak (measured: with it set, the orphaned `sleep` test survives)
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn CreateJobObjectW(attributes: *mut c_void, name: *const u16) -> *mut c_void;
        fn SetInformationJobObject(job: *mut c_void, class: i32, info: *mut c_void, len: u32) -> i32;
        fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
        fn TerminateJobObject(job: *mut c_void, exit_code: u32) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }

    /// A kill-on-close job holding `child`; None if any step fails (the
    /// caller falls back to `kill_tree`).
    pub fn job_for(child: &std::process::Child) -> Option<Handle> {
        unsafe {
            let job = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
            if job.is_null() {
                return None;
            }
            let job = Handle(job);
            let mut info = ExtendedLimit::default();
            info.basic.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(job.0, JOB_OBJECT_EXTENDED_LIMIT_INFORMATION, &mut info as *mut _ as *mut c_void, std::mem::size_of::<ExtendedLimit>() as u32) != 0
                && AssignProcessToJobObject(job.0, child.as_raw_handle() as *mut c_void) != 0;
            if !ok {
                close(job);
                return None;
            }
            Some(job)
        }
    }

    pub fn terminate(job: &Handle) {
        unsafe { TerminateJobObject(job.0, 1) };
    }

    pub fn close(job: Handle) {
        unsafe { CloseHandle(job.0) };
    }
}

/// A path as a `/`-separated string — the form every stored path, git's
/// output and the `{topdir}/…` tokens use.  Windows accepts `/` too, so the
/// result still opens; elsewhere it is `to_string_lossy` unchanged.
pub fn slash(p: &Path) -> String {
    let s = p.to_string_lossy();
    if cfg!(windows) { s.replace('\\', "/") } else { s.into_owned() }
}

/// A path as a POSIX shell reads it inside a double-quoted string (bash would
/// eat Windows backslashes): `slash`.
pub fn shell_path(p: &Path) -> String {
    slash(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn msys_gains_native_symlinks_unless_set() {
        assert_eq!(msys_with_symlinks(None).as_deref(), Some("winsymlinks:native"));
        assert_eq!(msys_with_symlinks(Some("  ")).as_deref(), Some("winsymlinks:native"));
        assert_eq!(msys_with_symlinks(Some("noglob")).as_deref(), Some("noglob winsymlinks:native"));
        assert_eq!(msys_with_symlinks(Some("winsymlinks:nativestrict")), None);
        assert_eq!(msys_with_symlinks(Some("noglob winsymlinks:lnk")), None);
    }

    #[test]
    fn path_with_puts_the_dir_first() {
        let d = std::env::temp_dir().join("iter-platform-test");
        let p = path_with(&d);
        let first = std::env::split_paths(&p).next().unwrap();
        assert_eq!(first, d);
    }

    #[test]
    fn canonicalize_keeps_slash_joins_working() {
        let d = std::env::temp_dir().join(format!("iter-platform-canon-{}", std::process::id()));
        std::fs::create_dir_all(d.join("a").join("b")).unwrap();
        let c = canonicalize(&d).unwrap();
        assert!(!c.to_string_lossy().starts_with(r"\\?\"));
        assert!(c.join("a/b").is_dir(), "a `/` join under the canonical path opens");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn bash_runs_a_posix_script() {
        let out = bash().arg("-c").arg("printf '%s' \"$((2+3))\"").output().expect("bash runs");
        assert_eq!(String::from_utf8_lossy(&out.stdout), "5");
    }

    /// The 2026-10-08 leak: a `sleep` whose parent subshell has exited is out
    /// of `taskkill /T`'s reach, yet dies when the tree is dropped after a
    /// normal exit.
    #[cfg(windows)]
    #[test]
    fn dropping_the_tree_kills_orphaned_descendants() {
        let alive = |pid: &str| {
            let out = Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/NH"]).output().unwrap();
            String::from_utf8_lossy(&out.stdout).split_whitespace().any(|w| w == pid)
        };
        let child = bash()
            .arg("-c")
            .arg("(sleep 300 >/dev/null 2>&1 & cat /proc/$!/winpid)")
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("bash runs");
        let tree = ProcessTree::adopt(&child);
        let out = child.wait_with_output().expect("bash exits");
        let pid = String::from_utf8_lossy(&out.stdout).trim().to_string();
        assert!(!pid.is_empty() && alive(&pid), "the orphaned sleep ({pid:?}) should still be running");
        drop(tree);
        std::thread::sleep(std::time::Duration::from_millis(500));
        let left = alive(&pid);
        if left {
            kill_tree(pid.parse().unwrap());
        }
        assert!(!left, "the orphaned sleep {pid} outlived its tree");
    }
}
