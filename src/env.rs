//! `jlog env`: the names of all variables, the values of the safe list only,
//! and for each variable whether its value changed since the last snapshot.
//!
//! The change check compares keyed hashes, HMAC-SHA256 with a key that stays
//! on the machine, in `~/.local/share/jlog/key`. The hashes stay in the file
//! `.agents/log/env-hashes.json`, not in the run log. Without the key, a
//! reader of that file cannot test a guess of a value.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::logdir::LogDir;
use crate::redact::SAFE;
use crate::session::hex;

pub const STATE: &str = "env-hashes.json";
const KEY_LEN: usize = 32;
/// The word in place of each value that is not on the safe list.
pub const REDACTED: &str = "redacted";

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Var {
    pub name: String,
    /// The value for a name on the safe list, and `redacted` for any other.
    pub value: String,
    /// True when the value changed since the last snapshot, false when it
    /// did not, and null when there is nothing to compare with.
    pub changed: Option<bool>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Snapshot {
    pub vars: Vec<Var>,
    /// The names that the last snapshot had and this one has not.
    pub gone: Vec<String>,
}

#[derive(Serialize, Deserialize, Default)]
struct State {
    /// A hash that names the key, so a new key does not count as a change.
    key: String,
    vars: BTreeMap<String, String>,
}

/// The snapshot of `vars`, compared with the state in `dir`, which it then
/// replaces. With no key, each `changed` is null and no state is written.
pub fn snapshot(
    vars: impl IntoIterator<Item = (OsString, OsString)>,
    key: Option<&[u8]>,
    dir: Option<&LogDir>,
) -> Snapshot {
    let mut vars: Vec<(String, OsString)> = vars
        .into_iter()
        .map(|(k, v)| (k.to_string_lossy().into_owned(), v))
        .collect();
    vars.sort_by(|a, b| a.0.cmp(&b.0));
    vars.dedup_by(|a, b| a.0 == b.0);

    let Some(key) = key else {
        let vars = vars.iter().map(|(n, v)| var(n, v, None)).collect();
        return Snapshot {
            vars,
            gone: Vec::new(),
        };
    };
    let key_id = hex(&mac(key, b"jlog key id", b"")[..16]);
    let old: Option<State> = dir
        .and_then(|d| d.read(STATE, 1 << 22).ok().flatten())
        .and_then(|b| serde_json::from_slice::<State>(&b).ok())
        .filter(|s| s.key == key_id);
    let mut new = State {
        key: key_id,
        vars: BTreeMap::new(),
    };
    let mut out = Vec::with_capacity(vars.len());
    for (name, value) in &vars {
        let hash = hex(&mac(key, name.as_bytes(), value.as_bytes()));
        let changed = old.as_ref().map(|o| o.vars.get(name) != Some(&hash));
        out.push(var(name, value, changed));
        new.vars.insert(name.clone(), hash);
    }
    let gone = old
        .as_ref()
        .map(|o| {
            o.vars
                .keys()
                .filter(|k| !new.vars.contains_key(*k))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if let Some(dir) = dir {
        let json = serde_json::to_vec(&new).expect("the state always serializes");
        let _ = dir.replace(STATE, &json);
    }
    Snapshot { vars: out, gone }
}

fn var(name: &str, value: &OsString, changed: Option<bool>) -> Var {
    let value = if SAFE.contains(&name) {
        value.to_string_lossy().into_owned()
    } else {
        REDACTED.to_string()
    };
    Var {
        name: name.to_string(),
        value,
        changed,
    }
}

/// HMAC-SHA256 of the name and the value, with a NUL between them, so the
/// same value under two names gives two hashes.
fn mac(key: &[u8], name: &[u8], value: &[u8]) -> Vec<u8> {
    let mut m = Hmac::<Sha256>::new_from_slice(key).expect("HMAC takes any key");
    m.update(name);
    m.update(&[0]);
    m.update(value);
    m.finalize().into_bytes().to_vec()
}

/// The folder of the key: `~/.local/share/jlog`.
pub fn key_dir(home: &Path) -> PathBuf {
    home.join(".local/share/jlog")
}

/// The key in `dir`, made on the first call. None when it cannot be read
/// or made, for example with a read-only home.
pub fn key(dir: &Path) -> Option<Vec<u8>> {
    let path = dir.join("key");
    if let Some(k) = read_key(&path) {
        return Some(k);
    }
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .ok()?;
    let mut fresh = [0u8; KEY_LEN];
    fs::File::open("/dev/urandom")
        .ok()?
        .read_exact(&mut fresh)
        .ok()?;
    // Write a whole new file, then link it in place, so a reader in another
    // jlog never sees half a key, and the first link wins.
    let tmp = dir.join(format!(".key.{}.{}", std::process::id(), hex(&fresh[..8])));
    let _ = fs::remove_file(&tmp);
    let written = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&tmp)
        .and_then(|mut f| f.write_all(&fresh));
    let linked = written.and_then(|()| fs::hard_link(&tmp, &path));
    let _ = fs::remove_file(&tmp);
    match linked {
        Ok(()) => Some(fresh.to_vec()),
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => read_key(&path),
        Err(_) => None,
    }
}

/// A key file counts only if it is a regular file of 32 bytes, with one link
/// and no access for others.
fn read_key(path: &Path) -> Option<Vec<u8>> {
    let mut f = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .ok()?;
    let meta = f.metadata().ok()?;
    if !meta.is_file()
        || meta.nlink() != 1
        || meta.len() != KEY_LEN as u64
        || meta.mode() & 0o077 != 0
    {
        return None;
    }
    let mut k = vec![0u8; KEY_LEN];
    f.read_exact(&mut k).ok()?;
    Some(k)
}

/// The snapshot of this process, with the key from `HOME`.
pub fn snapshot_here(dir: Option<&LogDir>) -> Snapshot {
    let key = std::env::var_os("HOME").and_then(|h| key_for_home(Path::new(&h)));
    snapshot(std::env::vars_os(), key.as_deref(), dir)
}

/// The key under `home`, only for an absolute home that this user owns. A
/// relative `HOME` would put the key inside the repository. Under `sudo`,
/// which keeps `HOME` on macOS, root would make a folder that the user can
/// then not read.
pub fn key_for_home(home: &Path) -> Option<Vec<u8>> {
    // SAFETY: geteuid has no preconditions.
    let me = unsafe { libc::geteuid() };
    let owned = fs::metadata(home).is_ok_and(|m| m.uid() == me);
    (home.is_absolute() && owned)
        .then(|| key(&key_dir(home)))
        .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(list: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        list.iter().map(|(k, v)| (k.into(), v.into())).collect()
    }

    fn log_dir(t: &tempfile::TempDir) -> LogDir {
        fs::create_dir(t.path().join(".agents")).unwrap();
        LogDir::open(t.path(), None).unwrap()
    }

    #[test]
    fn only_the_safe_list_shows_values() {
        let s = snapshot(
            vars(&[("FAKE_API_KEY", "sk-not-a-real-key"), ("PATH", "/bin")]),
            None,
            None,
        );
        assert_eq!(
            s.vars[0],
            Var {
                name: "FAKE_API_KEY".into(),
                value: "redacted".into(),
                changed: None
            }
        );
        assert_eq!(
            s.vars[1],
            Var {
                name: "PATH".into(),
                value: "/bin".into(),
                changed: None
            }
        );
        let json = serde_json::to_string(&s).unwrap();
        assert!(!json.contains("sk-not-a-real-key"));
    }

    #[test]
    fn a_change_shows_on_the_next_snapshot_and_the_state_holds_no_value() {
        let t = tempfile::tempdir().unwrap();
        let dir = log_dir(&t);
        let key = [7u8; 32];
        let first = snapshot(
            vars(&[("A", "one-secret"), ("B", "same"), ("C", "x")]),
            Some(&key),
            Some(&dir),
        );
        assert!(first.vars.iter().all(|v| v.changed.is_none()));
        let next = snapshot(
            vars(&[("A", "two-secret"), ("B", "same"), ("D", "y")]),
            Some(&key),
            Some(&dir),
        );
        let changed: Vec<_> = next
            .vars
            .iter()
            .map(|v| (v.name.as_str(), v.changed))
            .collect();
        assert_eq!(
            changed,
            [("A", Some(true)), ("B", Some(false)), ("D", Some(true))]
        );
        assert_eq!(next.gone, ["C"]);
        let state = fs::read_to_string(t.path().join(".agents/log").join(STATE)).unwrap();
        assert!(!state.contains("two-secret") && !state.contains("same\""));
    }

    #[test]
    fn a_new_key_does_not_count_as_a_change() {
        let t = tempfile::tempdir().unwrap();
        let dir = log_dir(&t);
        snapshot(vars(&[("A", "v")]), Some(&[1u8; 32]), Some(&dir));
        let s = snapshot(vars(&[("A", "v")]), Some(&[2u8; 32]), Some(&dir));
        assert_eq!(s.vars[0].changed, None);
    }

    #[test]
    fn the_same_value_under_two_names_has_two_hashes() {
        assert_ne!(mac(b"k", b"A", b"v"), mac(b"k", b"B", b"v"));
        assert_ne!(mac(b"k", b"A", b"Bv"), mac(b"k", b"AB", b"v"));
    }

    #[test]
    fn a_relative_home_or_a_home_of_another_user_gets_no_key() {
        let t = tempfile::tempdir().unwrap();
        let home = t.path().join("home");
        fs::create_dir(&home).unwrap();
        assert!(key_for_home(&home).is_some());
        assert!(home.join(".local/share/jlog/key").is_file());
        assert_eq!(key_for_home(Path::new("relhome")), None);
        assert!(!Path::new("relhome").exists());
        // SAFETY: geteuid has no preconditions.
        if unsafe { libc::geteuid() } != 0 {
            assert_eq!(key_for_home(Path::new("/")), None, "/ belongs to root");
        }
    }

    #[test]
    fn the_key_is_made_once_and_kept_private() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("share/jlog");
        let a = key(&dir).unwrap();
        assert_eq!(key(&dir).unwrap(), a);
        let meta = fs::metadata(dir.join("key")).unwrap();
        assert_eq!(meta.mode() & 0o777, 0o600);
        assert_eq!(fs::metadata(&dir).unwrap().mode() & 0o777, 0o700);
        assert_eq!(
            fs::read_dir(&dir).unwrap().count(),
            1,
            "no temporary file is left"
        );
    }

    #[test]
    fn a_key_that_others_can_read_or_a_symlink_is_refused() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("jlog");
        fs::create_dir(&dir).unwrap();
        fs::write(t.path().join("other"), [0u8; 32]).unwrap();
        std::os::unix::fs::symlink(t.path().join("other"), dir.join("key")).unwrap();
        assert_eq!(read_key(&dir.join("key")), None);
        assert_eq!(
            key(&dir),
            None,
            "jlog does not replace a key it cannot trust"
        );
        fs::remove_file(dir.join("key")).unwrap();
        fs::write(dir.join("key"), [0u8; 32]).unwrap();
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir.join("key"), fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(read_key(&dir.join("key")), None);
    }

    #[test]
    fn many_jlogs_at_once_agree_on_one_key() {
        let t = tempfile::tempdir().unwrap();
        let dir = t.path().join("jlog");
        let keys: Vec<_> = (0..8)
            .map(|_| {
                let dir = dir.clone();
                std::thread::spawn(move || key(&dir))
            })
            .map(|h| h.join().unwrap())
            .collect();
        let disk = read_key(&dir.join("key")).unwrap();
        assert!(keys.iter().all(|k| k.as_ref() == Some(&disk)));
    }
}
