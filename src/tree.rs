//! The worktree tree: the tree that `git commit -a` with every new file would
//! make. jlog computes it with a temporary index, so the real index does not
//! change. Files that git ignores are not in the tree.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The worktree tree of the repository at `root`, as a hex ID.
///
/// The temporary index starts as a copy of the real index, so `git add -A`
/// hashes only the files that changed, and a file that is ignored but
/// tracked stays in the tree, as in the commit. When an entry of the real
/// index is marked assume-unchanged or skip-worktree, git would not look at
/// that file, so the index starts from `HEAD` instead, as the spec says.
pub fn worktree_tree(root: &Path) -> Result<String, String> {
    // git reads the alternates from the top of the worktree, so a relative
    // root would point them at the wrong folder.
    let root =
        &std::path::absolute(root).map_err(|e| format!("no absolute path for the root: {e}"))?;
    let temp = TempTree::new()?;
    let objects = real_objects(root)?;
    let index = temp.0.join("index");
    let git = |args: &[&str]| -> Result<String, String> {
        let out = Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_INDEX_FILE", &index)
            // New objects go to the temporary store, and git reads the old
            // ones from the real store, so `.git/objects` does not change.
            .env("GIT_OBJECT_DIRECTORY", temp.0.join("objects"))
            .env("GIT_ALTERNATE_OBJECT_DIRECTORIES", &objects)
            .stdin(Stdio::null())
            .output()
            .map_err(|e| format!("could not run git: {e}"))?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            return Err(format!("git {} failed: {}", args.join(" "), err.trim()));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    };
    let real = real_index(root);
    let copied = real
        .as_ref()
        .is_some_and(|r| std::fs::copy(r, &index).is_ok());
    let flagged = copied && git(&["ls-files", "-v"]).map_or(true, |l| has_flags(&l));
    if !copied || flagged {
        let _ = std::fs::remove_file(&index);
        // A new repository has no HEAD, so its index starts empty.
        if git(&["rev-parse", "--verify", "--quiet", "HEAD^{tree}"]).is_ok() {
            git(&["read-tree", "HEAD"])?;
        }
    }
    git(&["add", "-A", "--", ":/"])?;
    let tree = git(&["write-tree"])?;
    if tree.len() >= 40 && tree.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(tree)
    } else {
        Err(format!("git write-tree gave {tree:?}"))
    }
}

/// The value of `GIT_ALTERNATE_OBJECT_DIRECTORIES` for the temporary store:
/// the real object store, and any alternates that the caller already set.
/// git follows the alternates that the real store lists itself.
fn real_objects(root: &Path) -> Result<OsString, String> {
    // `--path-format=absolute` needs git 2.31, so join a relative path here.
    let out = Command::new("git")
        .args(["rev-parse", "--git-path", "objects"])
        .current_dir(root)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("could not run git: {e}"))?;
    let path = root.join(OsStr::from_bytes(strip_newline(&out.stdout)));
    if !out.status.success() || out.stdout.is_empty() || !path.is_dir() {
        return Err("no git object store here".into());
    }
    let mut value = OsString::from_vec(quote_alternate(path.as_os_str().as_bytes()));
    if let Some(more) =
        std::env::var_os("GIT_ALTERNATE_OBJECT_DIRECTORIES").filter(|v| !v.is_empty())
    {
        value.push(":");
        value.push(more);
    }
    Ok(value)
}

fn strip_newline(b: &[u8]) -> &[u8] {
    b.strip_suffix(b"\n").unwrap_or(b)
}

/// git splits alternates at `:`, and reads an entry that starts with `"` as
/// a C string. So a path with `:`, `"`, `\` or a control byte goes in
/// quotes. Other bytes, such as UTF-8 or invalid UTF-8, stay as they are.
fn quote_alternate(path: &[u8]) -> Vec<u8> {
    let special = |b: u8| b == b':' || b == b'"' || b == b'\\' || b < 0x20 || b == 0x7f;
    if !path.iter().any(|&b| special(b)) {
        return path.to_vec();
    }
    let mut q = vec![b'"'];
    for &b in path {
        match b {
            b'"' | b'\\' => q.extend([b'\\', b]),
            b if b < 0x20 || b == 0x7f => q.extend(format!("\\{b:03o}").bytes()),
            b => q.push(b),
        }
    }
    q.push(b'"');
    q
}

/// The real index: `GIT_INDEX_FILE` when a git hook sets it, or else the
/// index of the repository. git resolves a relative `GIT_INDEX_FILE` from
/// the top of the worktree, and `--git-path` prints it from the current
/// folder, so joining it to `root` works when `root` is a subfolder.
fn real_index(root: &Path) -> Option<PathBuf> {
    let out = Command::new("git")
        .args(["rev-parse", "--git-path", "index"])
        .current_dir(root)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    // `--path-format=absolute` needs git 2.31, so join a relative path here.
    let path = strip_newline(&out.stdout);
    (out.status.success() && !path.is_empty()).then(|| root.join(OsStr::from_bytes(path)))
}

/// True when `git ls-files -v` shows an entry that is assume-unchanged (a
/// small letter) or skip-worktree (`S`).
fn has_flags(listing: &str) -> bool {
    listing.lines().any(|l| {
        l.bytes()
            .next()
            .is_some_and(|b| b.is_ascii_lowercase() || b == b'S')
    })
}

/// The prefix of the temporary folders, followed by the process ID.
const PREFIX: &str = "jlog-tree-";

/// The sweep leaves a folder younger than this, because a process in another
/// PID namespace that shares the temporary folder can look dead from here.
const STALE: Duration = Duration::from_secs(3600);

/// A temporary folder that holds the index and the object store. Dropping it
/// removes the folder. A jlog that is killed, or that panics in a release
/// build, cannot drop it, so each new folder first sweeps the old folders of
/// processes that no longer run.
struct TempTree(PathBuf, #[allow(dead_code)] std::fs::File);

impl TempTree {
    fn new() -> Result<TempTree, String> {
        let root = temp_root();
        // Another user can take the name of the private folder in a shared
        // /tmp. Then the folder goes straight into /tmp, without the sweep.
        let home = match own_folder(&root) {
            Ok(home) => {
                sweep(&home, STALE);
                home
            }
            Err(_) => root,
        };
        let mut last = String::new();
        for _ in 0..8 {
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.subsec_nanos());
            let path = home.join(format!("{PREFIX}{}-{nanos}", std::process::id()));
            match std::fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => {}
                Err(e) => {
                    last = format!("could not make {}: {e}", path.display());
                    continue;
                }
            }
            // The lock tells the sweep that the folder is in use, also to a
            // process in another PID namespace on the same machine.
            let dir = match std::fs::File::open(&path) {
                Ok(f) if try_lock(&f) => f,
                _ => {
                    let _ = std::fs::remove_dir(&path);
                    return Err(format!("could not lock {}", path.display()));
                }
            };
            let temp = TempTree(path, dir);
            std::fs::create_dir(temp.0.join("objects"))
                .map_err(|e| format!("could not make the object store: {e}"))?;
            return Ok(temp);
        }
        Err(last)
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Takes an exclusive flock on `f` without waiting.
fn try_lock(f: &std::fs::File) -> bool {
    use std::os::fd::AsRawFd;
    loop {
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return true;
        }
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
            return false;
        }
    }
}

/// The temporary folder of the system. A relative TMPDIR would put the
/// folder inside the repository, so it falls back to `/tmp`.
fn temp_root() -> PathBuf {
    Some(std::env::temp_dir())
        .filter(|d| d.is_absolute())
        .unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// The folder `jlog-<euid>` in `root`, made with mode 0700 when it is
/// missing. It holds only the folders of this user, so the sweep reads a
/// small folder, not the whole temporary folder. A link, or a folder that
/// another user owns or can write, is refused.
fn own_folder(root: &Path) -> Result<PathBuf, String> {
    let me = unsafe { libc::geteuid() };
    let dir = root.join(format!("jlog-{me}"));
    match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(format!("could not make {}: {e}", dir.display())),
    }
    let meta = std::fs::symlink_metadata(&dir).map_err(|e| e.to_string())?;
    if !meta.is_dir() || meta.uid() != me || meta.mode() & 0o022 != 0 {
        return Err(format!(
            "{} is not a private folder of this user",
            dir.display()
        ));
    }
    Ok(dir)
}

/// Removes the folders in `dir` whose process no longer runs, that have not
/// changed for `stale`, and whose lock no one holds.
fn sweep(dir: &Path, stale: Duration) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(rest) = name.to_str().and_then(|n| n.strip_prefix(PREFIX)) else {
            continue;
        };
        let Some(pid) = rest
            .split('-')
            .next()
            .and_then(|p| p.parse::<libc::pid_t>().ok())
        else {
            continue;
        };
        if pid <= 0 || runs(pid) {
            continue;
        }
        let path = entry.path();
        // Never follow a link.
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        let old = meta
            .modified()
            .ok()
            .and_then(|m| m.elapsed().ok())
            .is_some_and(|age| age >= stale);
        // A folder whose lock is held is in use, whatever its process ID.
        let free = meta.is_dir() && std::fs::File::open(&path).is_ok_and(|f| try_lock(&f));
        if old && free {
            let _ = std::fs::remove_dir_all(&path);
        }
    }
}

/// True unless the process `pid` surely does not exist.
fn runs(pid: libc::pid_t) -> bool {
    let alive = unsafe { libc::kill(pid, 0) } == 0;
    alive || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@example.com")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@example.com")
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    #[test]
    fn the_tree_holds_unstaged_and_new_files_and_leaves_the_index_alone() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        git(d, &["init", "-q"]);
        fs::write(d.join("a"), "1").unwrap();
        fs::write(d.join(".gitignore"), "ignored\n").unwrap();
        git(d, &["add", "-A"]);
        git(d, &["commit", "-q", "-m", "one"]);
        let head = git(d, &["rev-parse", "HEAD^{tree}"]);
        assert_eq!(worktree_tree(d).unwrap(), head);

        fs::write(d.join("ignored"), "x").unwrap();
        assert_eq!(
            worktree_tree(d).unwrap(),
            head,
            "an ignored file is not in the tree"
        );

        let index_before = fs::read(d.join(".git/index")).unwrap();
        fs::write(d.join("a"), "2").unwrap();
        fs::write(d.join("new"), "n").unwrap();
        let tree = worktree_tree(d).unwrap();
        assert_ne!(tree, head);
        assert_eq!(fs::read(d.join(".git/index")).unwrap(), index_before);
        assert_eq!(git(d, &["status", "--porcelain"]), "M a\n?? new");

        // The tree is what a commit of every change would give.
        git(d, &["add", "-A"]);
        assert_eq!(git(d, &["write-tree"]), tree);
    }

    #[test]
    fn a_new_repository_and_a_subfolder_work() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        git(d, &["init", "-q"]);
        fs::create_dir(d.join("sub")).unwrap();
        fs::write(d.join("top"), "t").unwrap();
        fs::write(d.join("sub/f"), "f").unwrap();
        let from_top = worktree_tree(d).unwrap();
        assert_eq!(
            worktree_tree(&d.join("sub")).unwrap(),
            from_top,
            "the whole worktree, from any folder"
        );
    }

    #[test]
    fn a_tracked_file_that_is_ignored_stays_in_the_tree_as_in_a_commit() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        git(d, &["init", "-q"]);
        fs::write(d.join(".gitignore"), "gen.txt\n").unwrap();
        fs::write(d.join("gen.txt"), "g").unwrap();
        git(d, &["add", "-f", "gen.txt", ".gitignore"]);
        git(d, &["commit", "-q", "-m", "one"]);
        fs::write(d.join("gen.txt"), "changed").unwrap();
        let tree = worktree_tree(d).unwrap();
        git(d, &["add", "-A"]);
        git(d, &["add", "-f", "gen.txt"]);
        assert_eq!(tree, git(d, &["write-tree"]));
    }

    fn set_mtime(path: &Path, t: SystemTime) {
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }

    /// git looks again at a file whose mtime is not older than the index,
    /// because it may have changed in the same second. The copy of the index
    /// must keep that mtime, or a change of the same size in that second
    /// looks clean and drops out of the tree.
    #[test]
    fn a_change_in_the_second_of_the_index_is_in_the_tree() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        git(d, &["init", "-q"]);
        // Many git builds compare the ctime only to the second, so a write in
        // the same second keeps it. Turning it off makes the test the same on
        // every build.
        git(d, &["config", "core.trustctime", "false"]);
        let then = SystemTime::now() - Duration::from_secs(10);
        fs::write(d.join("a.txt"), "aaa").unwrap();
        set_mtime(&d.join("a.txt"), then);
        git(d, &["add", "a.txt"]);
        set_mtime(&d.join(".git/index"), then);
        fs::write(d.join("a.txt"), "bbb").unwrap();
        set_mtime(&d.join("a.txt"), then);
        let tree = worktree_tree(d).unwrap();
        git(d, &["add", "a.txt"]);
        assert_eq!(tree, git(d, &["write-tree"]), "the tree holds bbb");
        let blob = git(d, &["rev-parse", &format!("{tree}:a.txt")]);
        assert_eq!(git(d, &["cat-file", "blob", &blob]), "bbb");
    }

    #[test]
    fn a_file_marked_assume_unchanged_still_counts() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        git(d, &["init", "-q"]);
        fs::write(d.join("a"), "1").unwrap();
        git(d, &["add", "a"]);
        git(d, &["commit", "-q", "-m", "one"]);
        let head = git(d, &["rev-parse", "HEAD^{tree}"]);
        git(d, &["update-index", "--assume-unchanged", "a"]);
        fs::write(d.join("a"), "2").unwrap();
        assert_ne!(
            worktree_tree(d).unwrap(),
            head,
            "the change in `a` is in the tree"
        );
    }

    #[test]
    fn outside_a_repository_is_an_error() {
        let t = tempfile::tempdir().unwrap();
        assert!(worktree_tree(t.path()).is_err());
    }

    /// The files in `dir` and its subfolders. A test compares the sets, not
    /// the counts, because git may pack or prune old objects meanwhile.
    fn files(dir: &Path) -> std::collections::BTreeSet<PathBuf> {
        let mut out = std::collections::BTreeSet::new();
        for e in fs::read_dir(dir).unwrap().flatten() {
            if e.file_type().unwrap().is_dir() {
                out.extend(files(&e.path()));
            } else {
                out.insert(e.path());
            }
        }
        out
    }

    /// The files that are in `after` but not in `before`.
    fn new_files(
        before: &std::collections::BTreeSet<PathBuf>,
        after: &std::collections::BTreeSet<PathBuf>,
    ) -> Vec<PathBuf> {
        after.difference(before).cloned().collect()
    }

    #[test]
    fn the_tree_writes_nothing_into_the_object_store_of_the_repository() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        git(d, &["init", "-q"]);
        fs::write(d.join("a"), "1").unwrap();
        git(d, &["add", "a"]);
        git(d, &["commit", "-q", "-m", "one"]);
        fs::write(d.join("a"), "an edit that no object holds yet").unwrap();
        fs::create_dir(d.join("sub")).unwrap();
        fs::write(d.join("sub/new"), "a new file").unwrap();
        let before = files(&d.join(".git/objects"));
        let tree = worktree_tree(d).unwrap();
        assert_eq!(
            new_files(&before, &files(&d.join(".git/objects"))),
            Vec::<PathBuf>::new()
        );
        // The tree is still the one a commit of every change would give.
        git(d, &["add", "-A"]);
        assert_eq!(git(d, &["write-tree"]), tree);
    }

    #[test]
    fn a_repository_path_with_a_colon_or_a_quote_works() {
        let t = tempfile::tempdir().unwrap();
        for name in ["a:b", "a\"b", "a\\b"] {
            let d = t.path().join(name);
            fs::create_dir(&d).unwrap();
            git(&d, &["init", "-q"]);
            fs::write(d.join("a"), "1").unwrap();
            git(&d, &["add", "a"]);
            git(&d, &["commit", "-q", "-m", "one"]);
            let head = git(&d, &["rev-parse", "HEAD^{tree}"]);
            assert_eq!(worktree_tree(&d).unwrap(), head, "{name}");
            fs::write(d.join("a"), "2").unwrap();
            let before = files(&d.join(".git/objects"));
            let tree = worktree_tree(&d).unwrap();
            assert_ne!(tree, head, "{name}");
            assert_eq!(
                new_files(&before, &files(&d.join(".git/objects"))),
                Vec::<PathBuf>::new(),
                "{name}"
            );
        }
    }

    #[test]
    fn quoting_follows_the_c_style_of_git() {
        assert_eq!(quote_alternate(b"/a/b c"), b"/a/b c");
        assert_eq!(quote_alternate(b"/a:b"), b"\"/a:b\"");
        assert_eq!(quote_alternate(b"/a\"b\\c"), b"\"/a\\\"b\\\\c\"");
        assert_eq!(quote_alternate(b"/a\nb"), b"\"/a\\012b\"");
        assert_eq!(quote_alternate(b"/a\xff:b"), b"\"/a\xff:b\"");
    }

    /// A process ID above the limit of Linux and macOS, so it never runs.
    const DEAD: u32 = 2_000_000_000;

    #[test]
    fn the_sweep_removes_the_old_folders_of_dead_processes_only() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        assert!(!runs(DEAD as libc::pid_t));
        let live = std::process::id();
        let gone = d.join(format!("{PREFIX}{DEAD}-1"));
        fs::create_dir_all(gone.join("objects/ab")).unwrap();
        fs::write(gone.join("objects/ab/cd"), "x").unwrap();
        fs::write(gone.join("index"), "x").unwrap();
        let kept = d.join(format!("{PREFIX}{live}-3"));
        fs::create_dir(&kept).unwrap();
        let other = d.join("jlog-tree-x-4");
        fs::create_dir(&other).unwrap();
        // A link with the name of a dead folder: the sweep must not follow it.
        let target = d.join("target");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("keep"), "k").unwrap();
        let link = d.join(format!("{PREFIX}{DEAD}-5"));
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let locked = d.join(format!("{PREFIX}{DEAD}-6"));
        fs::create_dir(&locked).unwrap();
        let held = fs::File::open(&locked).unwrap();
        assert!(try_lock(&held));

        sweep(d, STALE);
        assert!(
            gone.exists(),
            "a young folder stays: its process may run in another PID namespace"
        );
        sweep(d, Duration::ZERO);
        assert!(!gone.exists());
        assert!(kept.exists(), "the folder of a live process stays");
        assert!(other.exists(), "a name without a process ID stays");
        assert!(link.symlink_metadata().is_ok(), "a link stays");
        assert!(locked.exists(), "a folder whose lock is held stays");
        // Unlock, not just close: a child that another test forks at this
        // moment shares the open file, so a close alone can keep the lock.
        use std::os::fd::AsRawFd;
        assert_eq!(unsafe { libc::flock(held.as_raw_fd(), libc::LOCK_UN) }, 0);
        sweep(d, Duration::ZERO);
        assert!(!locked.exists());
        assert!(
            target.join("keep").exists(),
            "the sweep does not follow a link"
        );
    }

    #[test]
    fn the_folder_of_the_user_must_be_private() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path();
        let own = own_folder(d).unwrap();
        assert_eq!(fs::metadata(&own).unwrap().mode() & 0o777, 0o700);
        assert_eq!(own_folder(d).unwrap(), own, "a second call reuses it");

        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&own, fs::Permissions::from_mode(0o777)).unwrap();
        assert!(
            own_folder(d).is_err(),
            "a folder others can write is refused"
        );
        fs::remove_dir(&own).unwrap();

        let target = d.join("elsewhere");
        fs::create_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, &own).unwrap();
        assert!(own_folder(d).is_err(), "a link is refused");
    }
}
