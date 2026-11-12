//! Finds the log folder and appends lines to the run log, with no call to git.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub const LOG: &str = ".agents/log";
pub const RUN_LOG: &str = "just.jsonl";

/// The folders whose children are home folders, and the home folders of root.
/// `/var` is a symlink on macOS, and getcwd gives `/private/var/root`.
pub const HOME_PARENTS: [&str; 2] = ["/Users", "/home"];
pub const ROOT_HOMES: [&str; 3] = ["/root", "/var/root", "/private/var/root"];

/// The folder that holds `.agents/`, found by going up from `cwd`.
///
/// The search stops at the first folder that holds `.git`, which is the top
/// of a repository or a worktree. It never takes a home folder, so a folder
/// such as `~/.agents` of the global skills is not a log folder. A home
/// folder is the folder in `HOME`, `/root`, or a folder right under `/Users`
/// or `/home`. If `.agents` is a symlink, the check uses its real path, and
/// refuses it when that path is inside the `.agents` of a home folder. The
/// check works when `HOME` is unset or wrong.
pub fn find_root(cwd: &Path, home: Option<&Path>) -> Option<PathBuf> {
    find_root_with(cwd, home, &HOME_PARENTS, &ROOT_HOMES)
}

/// True when `dir` is a home folder. `home` is the value of `HOME`. It is
/// compared by device and inode, so a `HOME` that is a symlink still counts.
fn is_home(dir: &Path, home: Option<&Path>, home_parents: &[&str], root_homes: &[&str]) -> bool {
    let parent = dir.parent();
    parent.is_none()
        || root_homes.iter().any(|r| dir == Path::new(r))
        || parent.is_some_and(|p| home_parents.iter().any(|h| p == Path::new(h)))
        || home.is_some_and(|h| h == dir || same_file(h, dir))
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (fs::metadata(a), fs::metadata(b)) {
        (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
        _ => false,
    }
}

/// True when `real`, a real path, is a home `.agents` or is inside one, for
/// example a project cloned under `~/.agents/skills`.
fn in_home_agents(real: &Path, homey: &dyn Fn(&Path) -> bool) -> bool {
    real.ancestors()
        .any(|a| a.file_name().is_some_and(|n| n == ".agents") && a.parent().is_some_and(homey))
}

pub fn find_root_with(
    cwd: &Path,
    home: Option<&Path>,
    home_parents: &[&str],
    root_homes: &[&str],
) -> Option<PathBuf> {
    let homey = |d: &Path| is_home(d, home, home_parents, root_homes);
    for dir in cwd.ancestors() {
        let agents = dir.join(".agents");
        let meta = fs::symlink_metadata(&agents).ok();
        let Some(meta) = meta.filter(|m| m.is_dir() || m.file_type().is_symlink()) else {
            if dir.join(".git").exists() {
                return None;
            }
            continue;
        };
        if homey(dir) {
            return None;
        }
        // The current folder is a real path already, so only a symlink needs
        // realpath. That keeps the cost of each line low.
        let real = if meta.file_type().is_symlink() {
            fs::canonicalize(&agents).ok().filter(|r| r.is_dir())?
        } else {
            agents
        };
        if in_home_agents(&real, &homey) {
            return None;
        }
        return Some(dir.to_path_buf());
    }
    None
}

/// Appends one line to the run log of `root`. On the first write it makes
/// the log folder and its `.gitignore`, which holds the one line `*`.
pub fn append(root: &Path, home: Option<&Path>, line: &str) -> io::Result<()> {
    LogDir::open(root, home)?.append(line)
}

pub fn append_with(
    root: &Path,
    home: Option<&Path>,
    home_parents: &[&str],
    root_homes: &[&str],
    line: &str,
) -> io::Result<()> {
    LogDir::open_with(root, home, home_parents, root_homes)?.append(line)
}

/// The open log folder, `.agents/log`.
///
/// jlog opens `.agents` once and checks the real path of that open folder,
/// so a symlink swapped in after `find_root` cannot send the log into a home
/// `.agents`. It makes the log folder and each file inside open folders,
/// with no symlink followed. Each file must be a regular file with one link,
/// so a FIFO cannot make jlog wait for ever, and a hard link cannot send a
/// write elsewhere.
pub struct LogDir {
    dir: fs::File,
}

impl LogDir {
    pub fn open(root: &Path, home: Option<&Path>) -> io::Result<LogDir> {
        Self::open_with(root, home, &HOME_PARENTS, &ROOT_HOMES)
    }

    pub fn open_with(
        root: &Path,
        home: Option<&Path>,
        home_parents: &[&str],
        root_homes: &[&str],
    ) -> io::Result<LogDir> {
        let agents = open_c(&root.join(".agents"), libc::O_RDONLY | libc::O_DIRECTORY, 0)?;
        let homey = |d: &Path| is_home(d, home, home_parents, root_homes);
        if in_home_agents(&fd_path(&agents)?, &homey) {
            return Err(io::Error::other(
                "the .agents folder is inside a home .agents",
            ));
        }
        let dir = subdir(&agents, c"log")
            .map_err(|e| with_context(e, "the log folder is not a plain folder"))?;
        match openat(
            &dir,
            c".gitignore",
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW,
            0o644,
        ) {
            Ok(f) => (&f).write_all(b"*\n")?,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        Ok(LogDir { dir })
    }

    /// Appends one line to the run log.
    pub fn append(&self, line: &str) -> io::Result<()> {
        let file = openat(
            &self.dir,
            c"just.jsonl",
            libc::O_WRONLY | libc::O_APPEND | libc::O_CREAT | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            0o644,
        )?;
        plain(&file, "the run log")?;
        let mut buf = Vec::with_capacity(line.len() + 1);
        buf.extend_from_slice(line.as_bytes());
        buf.push(b'\n');
        // One write with O_APPEND, so that lines of parallel recipes do not mix.
        (&file).write_all(&buf)
    }

    /// Makes the marker `sessions/<name>`. True when this call made it, so
    /// only the first line of a session gets true, even in parallel.
    pub fn mark_session(&self, name: &str) -> io::Result<bool> {
        let name = file_name(name)?;
        let sessions = match openat(&self.dir, c"sessions", DIR_FLAGS, 0) {
            Ok(d) => d,
            Err(e) if e.kind() == io::ErrorKind::NotFound => subdir(&self.dir, c"sessions")?,
            Err(e) => return Err(e),
        };
        match openat(
            &sessions,
            &name,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW,
            0o644,
        ) {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// The bytes of the file `name` in the log folder, or None when it does
    /// not exist. At most `max` bytes are read.
    pub fn read(&self, name: &str, max: u64) -> io::Result<Option<Vec<u8>>> {
        use std::io::Read;
        let name = file_name(name)?;
        let file = match openat(
            &self.dir,
            &name,
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK,
            0,
        ) {
            Ok(f) => f,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e),
        };
        plain(&file, name.to_str().unwrap_or("the file"))?;
        let mut buf = Vec::new();
        file.take(max).read_to_end(&mut buf)?;
        Ok(Some(buf))
    }

    /// Replaces the file `name` in the log folder with `bytes`, through a
    /// new file and a rename, so a reader never sees half of it.
    pub fn replace(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        use std::os::fd::AsRawFd;
        let final_name = file_name(name)?;
        let tmp = file_name(&format!(".{name}.{}", std::process::id()))?;
        // SAFETY: dir is an open folder and tmp is a valid C string. A
        // missing file is the usual case, so the result is ignored.
        unsafe { libc::unlinkat(self.dir.as_raw_fd(), tmp.as_ptr(), 0) };
        let file = openat(
            &self.dir,
            &tmp,
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW,
            0o644,
        )?;
        let written = (&file).write_all(bytes);
        drop(file);
        // SAFETY: as above; renameat replaces the name, never a target it follows.
        let renamed = written.and_then(|()| {
            let r = unsafe {
                libc::renameat(
                    self.dir.as_raw_fd(),
                    tmp.as_ptr(),
                    self.dir.as_raw_fd(),
                    final_name.as_ptr(),
                )
            };
            if r == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        });
        if renamed.is_err() {
            // SAFETY: as above.
            unsafe { libc::unlinkat(self.dir.as_raw_fd(), tmp.as_ptr(), 0) };
        }
        renamed
    }
}

const DIR_FLAGS: i32 = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW;

/// Opens the folder `name` inside `parent`, and makes it first if needed.
fn subdir(parent: &fs::File, name: &std::ffi::CStr) -> io::Result<fs::File> {
    // SAFETY: parent is an open folder and name is a valid C string.
    if unsafe { libc::mkdirat(raw(parent), name.as_ptr(), 0o755) } != 0 {
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::AlreadyExists {
            return Err(e);
        }
    }
    openat(parent, name, DIR_FLAGS, 0)
}

/// Fails unless `file` is a regular file with one link.
fn plain(file: &fs::File, what: &str) -> io::Result<()> {
    let meta = file.metadata()?;
    if !meta.is_file() || meta.nlink() != 1 {
        return Err(io::Error::other(format!(
            "{what} is not a regular file with one link"
        )));
    }
    Ok(())
}

/// A plain file name: no slash, not `.` or `..`, no NUL, not empty.
fn file_name(name: &str) -> io::Result<std::ffi::CString> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("not a plain file name: {name:?}"),
        ));
    }
    std::ffi::CString::new(name).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))
}

fn raw(f: &fs::File) -> i32 {
    use std::os::fd::AsRawFd;
    f.as_raw_fd()
}

/// The real path of an open folder.
#[cfg(target_os = "macos")]
fn fd_path(f: &fs::File) -> io::Result<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let mut buf = [0u8; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH writes at most PATH_MAX bytes, NUL included.
    if unsafe { libc::fcntl(raw(f), libc::F_GETPATH, buf.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(&buf[..end])))
}

#[cfg(target_os = "linux")]
fn fd_path(f: &fs::File) -> io::Result<PathBuf> {
    fs::read_link(format!("/proc/self/fd/{}", raw(f)))
}

fn with_context(e: io::Error, what: &str) -> io::Error {
    io::Error::new(e.kind(), format!("{what}: {e}"))
}

fn open_c(path: &Path, flags: i32, mode: libc::c_uint) -> io::Result<fs::File> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes())?;
    // SAFETY: c is a valid C string, and the result is checked.
    let fd = unsafe { libc::open(c.as_ptr(), flags | libc::O_CLOEXEC, mode) };
    fd_to_file(fd)
}

fn openat(
    dir: &fs::File,
    name: &std::ffi::CStr,
    flags: i32,
    mode: libc::c_uint,
) -> io::Result<fs::File> {
    use std::os::fd::AsRawFd;
    // SAFETY: dir is an open folder, name is a valid C string, and the result is checked.
    let fd = unsafe {
        libc::openat(
            dir.as_raw_fd(),
            name.as_ptr(),
            flags | libc::O_CLOEXEC,
            mode,
        )
    };
    fd_to_file(fd)
}

fn fd_to_file(fd: i32) -> io::Result<fs::File> {
    use std::os::fd::FromRawFd;
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fd is a new descriptor that nothing else owns.
    Ok(unsafe { fs::File::from_raw_fd(fd) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_symlink_into_the_agents_folder_of_home_does_not_count() {
        let t = tempfile::tempdir().unwrap();
        let home = real(t.path()).join("home");
        fs::create_dir_all(home.join(".agents/sub")).unwrap();
        let repo = real(t.path()).join("repo");
        fs::create_dir_all(&repo).unwrap();
        std::os::unix::fs::symlink(home.join(".agents/sub"), repo.join(".agents")).unwrap();
        assert_eq!(find_root(&repo, Some(&home)), None);
    }

    #[test]
    fn a_home_given_through_a_symlink_with_another_name_still_counts() {
        let t = tempfile::tempdir().unwrap();
        let top = real(t.path());
        fs::create_dir_all(top.join("realhome/.agents")).unwrap();
        fs::create_dir_all(top.join("realhome/proj")).unwrap();
        std::os::unix::fs::symlink(top.join("realhome"), top.join("me")).unwrap();
        assert_eq!(
            find_root(&top.join("realhome/proj"), Some(&top.join("me"))),
            None
        );
    }

    #[test]
    fn a_symlinked_log_folder_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let top = real(t.path());
        fs::create_dir_all(top.join("elsewhere")).unwrap();
        fs::create_dir_all(top.join("proj/.agents")).unwrap();
        std::os::unix::fs::symlink(top.join("elsewhere"), top.join("proj/.agents/log")).unwrap();
        assert!(append(&top.join("proj"), None, "{}").is_err());
        assert!(!top.join("elsewhere/just.jsonl").exists());
        assert!(!top.join("elsewhere/.gitignore").exists());
    }

    #[test]
    fn a_symlinked_run_log_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let top = real(t.path());
        fs::create_dir_all(top.join("proj/.agents/log")).unwrap();
        fs::write(top.join("target.jsonl"), "").unwrap();
        std::os::unix::fs::symlink(
            top.join("target.jsonl"),
            top.join("proj/.agents/log/just.jsonl"),
        )
        .unwrap();
        assert!(append(&top.join("proj"), None, "{}").is_err());
        assert_eq!(fs::read_to_string(top.join("target.jsonl")).unwrap(), "");
    }

    #[test]
    fn a_project_inside_the_agents_folder_of_home_gets_no_log() {
        let t = tempfile::tempdir().unwrap();
        let home = real(t.path()).join("home");
        let skill = home.join(".agents/skills/foo");
        fs::create_dir_all(skill.join(".agents")).unwrap();
        fs::create_dir_all(skill.join(".git")).unwrap();
        assert_eq!(find_root(&skill, Some(&home)), None);
        assert!(append(&skill, Some(&home), "{}").is_err());
        assert!(!skill.join(".agents/log").exists());
    }

    #[test]
    fn a_symlink_swapped_in_after_the_search_gets_no_write() {
        let t = tempfile::tempdir().unwrap();
        let top = real(t.path());
        let home = top.join("home");
        fs::create_dir_all(home.join(".agents")).unwrap();
        let proj = top.join("proj");
        fs::create_dir_all(proj.join(".agents")).unwrap();
        assert_eq!(find_root(&proj, Some(&home)), Some(proj.clone()));
        fs::remove_dir(proj.join(".agents")).unwrap();
        std::os::unix::fs::symlink(home.join(".agents"), proj.join(".agents")).unwrap();
        assert!(append(&proj, Some(&home), "{}").is_err());
        assert!(!home.join(".agents/log").exists());
    }

    #[test]
    fn a_hard_link_in_place_of_the_run_log_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let log = t.path().join(LOG);
        fs::create_dir_all(&log).unwrap();
        fs::write(t.path().join("other.jsonl"), "").unwrap();
        fs::hard_link(t.path().join("other.jsonl"), log.join(RUN_LOG)).unwrap();
        assert!(append(t.path(), None, "{}").is_err());
        assert_eq!(
            fs::read_to_string(t.path().join("other.jsonl")).unwrap(),
            ""
        );
    }

    #[test]
    fn a_fifo_in_place_of_the_run_log_does_not_block() {
        let t = tempfile::tempdir().unwrap();
        let log = t.path().join(LOG);
        fs::create_dir_all(&log).unwrap();
        let fifo = std::ffi::CString::new(log.join(RUN_LOG).to_str().unwrap()).unwrap();
        // SAFETY: fifo is a valid C string.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        assert!(append(t.path(), None, "{}").is_err());
    }

    #[test]
    fn a_file_named_agents_is_not_a_log_folder() {
        let t = tempfile::tempdir().unwrap();
        let top = real(t.path());
        fs::create_dir_all(top.join("a/.agents")).unwrap();
        fs::create_dir_all(top.join("a/b")).unwrap();
        fs::write(top.join("a/b/.agents"), "").unwrap();
        assert_eq!(find_root(&top.join("a/b"), None), Some(top.join("a")));
    }

    #[test]
    fn finds_the_nearest_agents_folder() {
        let t = tempfile::tempdir().unwrap();
        let deep = t.path().join("a/b");
        fs::create_dir_all(&deep).unwrap();
        fs::create_dir_all(t.path().join("a/.agents")).unwrap();
        assert_eq!(find_root(&deep, None), Some(t.path().join("a")));
    }

    #[test]
    fn stops_at_the_top_of_a_repository() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join(".agents")).unwrap();
        fs::create_dir_all(t.path().join("repo/.git")).unwrap();
        assert_eq!(find_root(&t.path().join("repo"), None), None);
    }

    #[test]
    fn a_git_file_of_a_worktree_also_stops_the_search() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join(".agents")).unwrap();
        fs::create_dir_all(t.path().join("wt")).unwrap();
        fs::write(t.path().join("wt/.git"), "gitdir: x\n").unwrap();
        assert_eq!(find_root(&t.path().join("wt"), None), None);
    }

    fn real(p: &Path) -> PathBuf {
        fs::canonicalize(p).unwrap()
    }

    #[test]
    fn never_takes_the_home_folder() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir_all(t.path().join(".agents")).unwrap();
        fs::create_dir_all(t.path().join("p")).unwrap();
        // jlog gets the current folder from getcwd, which gives a real path.
        let home = real(t.path());
        assert_eq!(find_root(&home.join("p"), Some(&home)), None);
        assert_eq!(
            find_root(&home.join("p"), Some(t.path())),
            None,
            "HOME through a symlink"
        );
    }

    #[test]
    fn never_takes_a_home_folder_when_home_is_unset() {
        // A temporary "Users" folder stands for /Users.
        let t = tempfile::tempdir().unwrap();
        let users = real(t.path()).join("Users");
        fs::create_dir_all(users.join("someone/.agents")).unwrap();
        fs::create_dir_all(users.join("someone/proj")).unwrap();
        let parents = [users.to_str().unwrap()];
        let got = find_root_with(&users.join("someone/proj"), None, &parents, &[]);
        assert_eq!(got, None);
        let root_home = users.join("someone");
        let got = find_root_with(
            &users.join("someone/proj"),
            None,
            &[],
            &[root_home.to_str().unwrap()],
        );
        assert_eq!(got, None);
    }

    #[test]
    fn a_symlink_to_the_home_agents_folder_does_not_count() {
        let t = tempfile::tempdir().unwrap();
        let home = real(t.path()).join("home");
        fs::create_dir_all(home.join(".agents")).unwrap();
        let repo = real(t.path()).join("repo");
        fs::create_dir_all(&repo).unwrap();
        std::os::unix::fs::symlink(home.join(".agents"), repo.join(".agents")).unwrap();
        assert_eq!(find_root(&repo, Some(&home)), None);
        assert_eq!(find_root(&repo, None), Some(repo.clone()));
    }

    #[test]
    fn a_project_below_a_home_folder_is_not_a_home() {
        let t = tempfile::tempdir().unwrap();
        let users = real(t.path()).join("Users");
        fs::create_dir_all(users.join("someone/proj/.agents")).unwrap();
        let parents = [users.to_str().unwrap()];
        let proj = users.join("someone/proj");
        assert_eq!(
            find_root_with(&proj, None, &parents, &[]),
            Some(proj.clone())
        );
    }

    #[test]
    fn first_append_writes_the_gitignore() {
        let t = tempfile::tempdir().unwrap();
        fs::create_dir(t.path().join(".agents")).unwrap();
        append(t.path(), None, "{}").unwrap();
        append(t.path(), None, "{\"a\":1}").unwrap();
        let log = t.path().join(LOG);
        assert_eq!(fs::read_to_string(log.join(".gitignore")).unwrap(), "*\n");
        assert_eq!(
            fs::read_to_string(log.join(RUN_LOG)).unwrap(),
            "{}\n{\"a\":1}\n"
        );
    }
}
