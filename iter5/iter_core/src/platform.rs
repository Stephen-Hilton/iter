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
}
