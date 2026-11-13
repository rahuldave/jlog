//! The worktree tree: the tree that `git commit -a` with every new file would
//! make. jlog computes it with a temporary index, so the real index does not
//! change. Files that git ignores are not in the tree.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The worktree tree of the repository at `root`, as a hex ID.
///
/// The temporary index starts as a copy of the real index, so `git add -A`
/// hashes only the files that changed, and a file that is ignored but
/// tracked stays in the tree, as in the commit. When an entry of the real
/// index is marked assume-unchanged or skip-worktree, git would not look at
/// that file, so the index starts from `HEAD` instead, as the spec says.
pub fn worktree_tree(root: &Path) -> Result<String, String> {
    let index = TempIndex::new();
    let git = |args: &[&str]| -> Result<String, String> {
        let out = Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_INDEX_FILE", &index.0)
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
        .is_some_and(|r| std::fs::copy(r, &index.0).is_ok());
    let flagged = copied && git(&["ls-files", "-v"]).map_or(true, |l| has_flags(&l));
    if !copied || flagged {
        let _ = std::fs::remove_file(&index.0);
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

/// The real index: `GIT_INDEX_FILE` when a git hook sets it, or else the
/// index of the repository.
fn real_index(root: &Path) -> Option<PathBuf> {
    if let Some(i) = std::env::var_os("GIT_INDEX_FILE").filter(|v| !v.is_empty()) {
        return Some(root.join(i));
    }
    let out = Command::new("git")
        .args(["rev-parse", "--path-format=absolute", "--git-path", "index"])
        .current_dir(root)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !path.is_empty()).then(|| PathBuf::from(path))
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

/// The path of a temporary index, removed with its lock file at the end.
struct TempIndex(PathBuf);

impl TempIndex {
    fn new() -> TempIndex {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.subsec_nanos());
        let name = format!("jlog-index-{}-{nanos}", std::process::id());
        // A relative TMPDIR would put the index inside the repository.
        let dir = Some(std::env::temp_dir())
            .filter(|d| d.is_absolute())
            .unwrap_or_else(|| PathBuf::from("/tmp"));
        TempIndex(dir.join(name))
    }
}

impl Drop for TempIndex {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let mut lock = self.0.clone().into_os_string();
        lock.push(".lock");
        let _ = std::fs::remove_file(lock);
    }
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
}
