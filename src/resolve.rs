//! The resolver: the PATH and the environment of a call of jlog, from the
//! `.jlog` folders above the current folder and the user defaults.
//!
//! A scope is a folder that holds `.jlog/bin` or `.jlog/environment.json`.
//! The search goes up from the current folder, crosses the top of a git
//! repository, and stops before a home folder or the root of the file
//! system, or after a scope whose configuration says `"root": true`. The
//! user defaults come after the scopes: `$XDG_DATA_HOME/jlog/bin` and
//! `$XDG_CONFIG_HOME/jlog/environment.json`.
//!
//! The PATH is each scope's `bin` and then its `path` entries, nearest scope
//! first, then the user's `bin` and `path` entries, then the inherited PATH.
//! For the environment, the nearest declaration of a name wins. The variable
//! `JLOG_RESOLVED` records what the last resolution added, so the next one,
//! for example in another project, takes back only that and keeps every
//! change that the caller made since.
//!
//! The resolver reads no git metadata, writes no file, installs nothing and
//! changes no folder.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::logdir;

/// The folder of a scope.
pub const DIR: &str = ".jlog";
/// The configuration file of a scope, and of the user.
pub const CONFIG: &str = "environment.json";
/// The variable that records what the last resolution added.
pub const RESOLVED: &str = "JLOG_RESOLVED";
/// The one version of the configuration that jlog reads.
pub const VERSION: u64 = 1;

/// The variables that the configuration may not set: PATH, which the
/// resolver builds, and the run and session identities, which jlog and the
/// harnesses own. Every name that starts with `JLOG_` is reserved too.
pub const RESERVED: [&str; 7] = [
    "PATH",
    "HOME",
    "XDG_DATA_HOME",
    "XDG_CONFIG_HOME",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
    "CODEX_THREAD_ID",
];

/// One value of the `env` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnvValue {
    /// A literal string, which `jlog path` does not print.
    Text(String),
    /// A path, absolute after the resolution, which may not exist yet.
    Path(PathBuf),
}

impl EnvValue {
    fn os(&self) -> OsString {
        match self {
            EnvValue::Text(s) => OsString::from(s),
            EnvValue::Path(p) => p.clone().into_os_string(),
        }
    }
}

/// One configuration file, with its paths absolute.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Config {
    pub root: bool,
    pub path: Vec<PathBuf>,
    pub env: Vec<(String, EnvValue)>,
}

/// A scope that the search found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scope {
    /// The folder that holds `.jlog`.
    pub dir: PathBuf,
    pub config: Option<Config>,
}

/// Where the PATH entry of a plan comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Scope(PathBuf),
    User,
}

/// What the resolver found, before it meets the inherited environment.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The scopes, nearest first.
    pub scopes: Vec<Scope>,
    /// `$XDG_DATA_HOME/jlog/bin`, or None with no usable data folder.
    pub user_bin: Option<PathBuf>,
    /// `$XDG_CONFIG_HOME/jlog/environment.json`, or None with no usable
    /// config folder.
    pub user_file: Option<PathBuf>,
    pub user: Option<Config>,
}

/// An error of a configuration file. Its text names the file and the key,
/// and never a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The folder of an XDG variable: its value when it is an absolute path, or
/// else `fallback` under an absolute HOME, or else None.
fn xdg(var: Option<OsString>, home: Option<&OsStr>, fallback: &str) -> Option<PathBuf> {
    if let Some(v) = var.filter(|v| !v.is_empty())
        && Path::new(&v).is_absolute()
    {
        return Some(PathBuf::from(v));
    }
    let home = Path::new(home.filter(|h| !h.is_empty())?);
    home.is_absolute().then(|| home.join(fallback))
}

/// `path` with `.` and `..` taken out by their text, so a path keeps its
/// symlinks, as a shell would see it.
pub fn clean(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other),
        }
    }
    out
}

/// Finds the scopes and the user defaults for the folder `cwd`. `var` reads
/// a variable of the environment.
pub fn plan(cwd: &Path, var: &dyn Fn(&str) -> Option<OsString>) -> Result<Plan, Error> {
    let home = var("HOME");
    let home_path = home.as_deref().filter(|h| !h.is_empty()).map(PathBuf::from);
    // HOME is read once, so each folder costs one stat for the home check
    // and one for `.jlog`.
    let home_id = home_path
        .as_deref()
        .and_then(|h| fs::metadata(h).ok())
        .map(|m| (m.dev(), m.ino()));
    let is_home = |dir: &Path| {
        let parent = dir.parent();
        parent.is_none()
            || logdir::ROOT_HOMES.iter().any(|r| dir == Path::new(r))
            || parent.is_some_and(|p| logdir::HOME_PARENTS.iter().any(|h| p == Path::new(h)))
            || home_path.as_deref() == Some(dir)
            || home_id.is_some_and(|id| fs::metadata(dir).is_ok_and(|m| (m.dev(), m.ino()) == id))
    };
    let mut scopes = Vec::new();
    for dir in cwd.ancestors() {
        if is_home(dir) {
            break;
        }
        // A folder that others can write to, such as /tmp, ends the search,
        // because anyone could put a `.jlog` there.
        if fs::metadata(dir).is_ok_and(|m| m.mode() & 0o002 != 0) {
            break;
        }
        let jlog = dir.join(DIR);
        if !jlog.is_dir() {
            continue;
        }
        let file = jlog.join(CONFIG);
        // Each part of a scope must belong to this user or to root, and not
        // every user may write to it.
        if let Some(bad) = [&jlog, &jlog.join("bin"), &file]
            .into_iter()
            .find(|p| !trusted(p))
        {
            // The skipped scope may say `root: true`, so the search stops.
            warn_untrusted(bad);
            break;
        }
        let has_bin = jlog.join("bin").is_dir();
        let config = if file.is_file() {
            Some(read_config(&file, dir, false)?)
        } else {
            None
        };
        if !has_bin && config.is_none() {
            continue;
        }
        let root = config.as_ref().is_some_and(|c| c.root);
        scopes.push(Scope {
            dir: dir.to_path_buf(),
            config,
        });
        if root {
            break;
        }
    }
    let user_bin = xdg(var("XDG_DATA_HOME"), home.as_deref(), ".local/share")
        .map(|d| d.join("jlog").join("bin"));
    let user_file = xdg(var("XDG_CONFIG_HOME"), home.as_deref(), ".config")
        .map(|d| d.join("jlog").join(CONFIG));
    let user_bin = user_bin.filter(|b| {
        let ok = trusted(b);
        if !ok {
            warn_untrusted(b);
        }
        ok
    });
    let user = match &user_file {
        Some(f) if f.is_file() && !trusted(f) => {
            warn_untrusted(f);
            None
        }
        Some(f) if f.is_file() => Some(read_config(f, Path::new("/"), true)?),
        _ => None,
    };
    Ok(Plan {
        scopes,
        user_bin,
        user_file,
        user,
    })
}

/// True when `path` is missing, or belongs to this user or to root and not
/// every user may write to it. A group may write, as git allows.
fn trusted(path: &Path) -> bool {
    let Ok(m) = fs::metadata(path) else {
        return true;
    };
    // SAFETY: geteuid has no preconditions.
    let me = unsafe { libc::geteuid() };
    (m.uid() == me || m.uid() == 0) && m.mode() & 0o002 == 0
}

fn warn_untrusted(path: &Path) {
    use std::io::Write;
    // SAFETY: geteuid has no preconditions.
    let me = unsafe { libc::geteuid() };
    let why = match fs::metadata(path) {
        Ok(m) if m.uid() != me && m.uid() != 0 => "it belongs to another user",
        _ => "every user can write to it",
    };
    let _ = writeln!(
        std::io::stderr(),
        "jlog: skipped {} and the folders above it: {why}",
        path.display()
    );
}

/// Reads one configuration file. Relative paths are based on `base`, the
/// folder that holds `.jlog`. In the user file, every path must be absolute.
pub fn read_config(file: &Path, base: &Path, user: bool) -> Result<Config, Error> {
    let at = file.display();
    let bytes = fs::read(file).map_err(|e| Error(format!("{at}: cannot read it: {e}")))?;
    // The message of a syntax error names a line and a column, never a value.
    let value: Value =
        serde_json::from_slice(&bytes).map_err(|e| Error(format!("{at}: not valid JSON: {e}")))?;
    let Value::Object(map) = value else {
        return Err(Error(format!("{at}: the file must hold one JSON object")));
    };
    for key in map.keys() {
        if !matches!(key.as_str(), "version" | "root" | "path" | "env") {
            return Err(Error(format!(
                "{at}: unknown key {key:?}; the keys are version, root, path and env"
            )));
        }
    }
    match map.get("version") {
        Some(Value::Number(n)) if n.as_u64() == Some(VERSION) => {}
        Some(Value::Number(_)) => {
            return Err(Error(format!(
                "{at}: version {} is not supported; this jlog reads version {VERSION}",
                map["version"]
            )));
        }
        Some(_) => return Err(Error(format!("{at}: version must be the number {VERSION}"))),
        None => return Err(Error(format!("{at}: the key version is missing"))),
    }
    let root = match map.get("root") {
        None => false,
        Some(Value::Bool(b)) => *b,
        Some(_) => return Err(Error(format!("{at}: root must be true or false"))),
    };
    let place = |text: &str, what: &str| -> Result<PathBuf, Error> {
        if text.is_empty() {
            return Err(Error(format!("{at}: {what} is an empty path")));
        }
        let p = Path::new(text);
        if user && !p.is_absolute() {
            return Err(Error(format!(
                "{at}: {what} must be an absolute path in the user file"
            )));
        }
        Ok(clean(&base.join(p)))
    };
    let mut path = Vec::new();
    match map.get("path") {
        None => {}
        Some(Value::Array(items)) => {
            for (i, item) in items.iter().enumerate() {
                let Value::String(s) = item else {
                    return Err(Error(format!("{at}: path[{i}] must be a string")));
                };
                path.push(place(s, &format!("path[{i}]"))?);
            }
        }
        Some(_) => return Err(Error(format!("{at}: path must be a list of strings"))),
    }
    let mut env = Vec::new();
    match map.get("env") {
        None => {}
        Some(Value::Object(vars)) => {
            for (name, v) in vars {
                check_name(name).map_err(|why| {
                    // A name with `=` may hold a value, so it is not shown.
                    if valid_name(name) {
                        Error(format!("{at}: env {name:?}: {why}"))
                    } else {
                        Error(format!("{at}: env: a key is {why}"))
                    }
                })?;
                let value = match v {
                    Value::String(s) => EnvValue::Text(s.clone()),
                    Value::Object(o) if o.len() == 1 && o.contains_key("path") => {
                        let Value::String(s) = &o["path"] else {
                            return Err(Error(format!(
                                "{at}: env {name:?}: path must be a string"
                            )));
                        };
                        EnvValue::Path(place(s, &format!("env {name:?}"))?)
                    }
                    _ => {
                        return Err(Error(format!(
                            "{at}: env {name:?}: the value must be a string or an object with one key, path"
                        )));
                    }
                };
                if value.os().as_bytes().contains(&0) {
                    return Err(Error(format!("{at}: env {name:?}: the value holds a NUL")));
                }
                env.push((name.clone(), value));
            }
        }
        Some(_) => return Err(Error(format!("{at}: env must be an object"))),
    }
    Ok(Config { root, path, env })
}

/// Ok when the configuration may set the variable `name`.
fn check_name(name: &str) -> Result<(), &'static str> {
    if !valid_name(name) {
        return Err("not a valid name of a variable");
    }
    if RESERVED.contains(&name) || name.starts_with("JLOG_") {
        return Err(
            "the name is reserved; PATH, the JLOG_ variables and the session variables are set elsewhere",
        );
    }
    Ok(())
}

impl Plan {
    /// The managed PATH entries, in order: each scope's `bin` and `path`,
    /// nearest first, then the user's. Only folders that exist and can sit in
    /// PATH, with no `:`, and each folder once.
    pub fn path(&self) -> Vec<(PathBuf, Source)> {
        let mut out: Vec<(PathBuf, Source)> = Vec::new();
        let mut add = |p: PathBuf, s: Source| {
            if p.as_os_str().as_bytes().contains(&b':') || !p.is_dir() {
                return;
            }
            if !out.iter().any(|(q, _)| *q == p) {
                out.push((p, s));
            }
        };
        for scope in &self.scopes {
            let src = Source::Scope(scope.dir.clone());
            add(scope.dir.join(DIR).join("bin"), src.clone());
            for p in scope.config.iter().flat_map(|c| &c.path) {
                add(p.clone(), src.clone());
            }
        }
        if let Some(b) = &self.user_bin {
            add(b.clone(), Source::User);
        }
        for p in self.user.iter().flat_map(|c| &c.path) {
            add(p.clone(), Source::User);
        }
        out
    }

    /// The managed variables: the user's first, then the outer scopes, then
    /// the inner ones, so the nearest declaration wins.
    pub fn env(&self) -> BTreeMap<String, (EnvValue, Source)> {
        let mut out = BTreeMap::new();
        for (n, v) in self.user.iter().flat_map(|c| &c.env) {
            out.insert(n.clone(), (v.clone(), Source::User));
        }
        for scope in self.scopes.iter().rev() {
            for (n, v) in scope.config.iter().flat_map(|c| &c.env) {
                out.insert(n.clone(), (v.clone(), Source::Scope(scope.dir.clone())));
            }
        }
        out
    }
}

/// What a resolution added, kept in `JLOG_RESOLVED` as JSON.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// The PATH entries that the resolution added, each as the hex of its
    /// bytes, so a name that is not UTF-8 still matches.
    pub path: Vec<String>,
    /// True when PATH was unset before the first resolution, so it goes
    /// back to unset when nothing is left in it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub path_unset: bool,
    /// For each variable that it set: the value that it set, and the value
    /// before, or null when there was none. A value that is not UTF-8 is
    /// kept as hex in `prev_hex`.
    pub env: BTreeMap<String, Entry>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub set: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prev_hex: Option<String>,
}

/// The bytes of a hex string, or None when it is not hex.
fn unhex(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            hex.get(i..i + 2)
                .and_then(|h| u8::from_str_radix(h, 16).ok())
        })
        .collect()
}

impl Entry {
    fn prev_os(&self) -> Option<OsString> {
        if let Some(p) = &self.prev {
            return Some(OsString::from(p));
        }
        unhex(self.prev_hex.as_ref()?).map(OsString::from_vec)
    }
}

/// The values in the record of `JLOG_RESOLVED`: each value that it set and
/// each value that it keeps to put back. The redaction treats each one as a
/// secret, because the old value of a variable now lives only here.
pub fn record_values(value: &OsStr) -> Vec<String> {
    let Ok(record) = serde_json::from_slice::<Record>(value.as_bytes()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in record.env.values() {
        out.push(e.set.clone());
        if let Some(p) = e.prev_os() {
            out.push(p.to_string_lossy().into_owned());
        }
    }
    out
}

/// True when `name` can be the name of a variable at all.
fn valid_name(name: &str) -> bool {
    !name.is_empty() && !name.contains('=') && !name.contains('\0')
}

/// The changes to make to the environment: each variable to set, and each to
/// remove.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Changes {
    pub set: Vec<(String, OsString)>,
    pub remove: Vec<String>,
}

/// The changes that turn the inherited environment, which `var` reads, into
/// the resolved one. First it takes back what the last resolution added, as
/// `JLOG_RESOLVED` records it: its PATH entries, and each variable whose value
/// is still the one it set. A change that the caller made since stays. Then
/// it adds the plan.
pub fn changes(plan: &Plan, var: &dyn Fn(&str) -> Option<OsString>) -> Changes {
    let old: Record = var(RESOLVED)
        .and_then(|v| serde_json::from_slice(v.as_bytes()).ok())
        .unwrap_or_default();

    let had_record = var(RESOLVED).is_some() && old != Record::default();
    // The PATH: the inherited entries, less one copy of each entry that the
    // last resolution added, after the new managed entries.
    let inherited = var("PATH");
    let path_unset = if had_record {
        old.path_unset
    } else {
        inherited.is_none()
    };
    let inherited = inherited.unwrap_or_default();
    let mut rest: Vec<&[u8]> = if inherited.is_empty() {
        Vec::new()
    } else {
        inherited.as_bytes().split(|b| *b == b':').collect()
    };
    for added in old.path.iter().filter_map(|h| unhex(h)) {
        if let Some(i) = rest.iter().position(|e| *e == added.as_slice()) {
            rest.remove(i);
        }
    }
    let managed = plan.path();
    // Each managed entry goes to the front and is recorded. A copy that the
    // caller had stays where it was, so a move to another project gives
    // the caller back the PATH it had.
    let mut added_now = Vec::new();
    let mut parts: Vec<&[u8]> = Vec::new();
    for (p, _) in &managed {
        let b = p.as_os_str().as_bytes();
        added_now.push(crate::session::hex(b));
        parts.push(b);
    }
    parts.extend(rest);
    let path = OsString::from_vec(parts.join(&b':'));
    let path_empty = path.is_empty();

    // The variables: take back the old ones, then set the new ones.
    let mut now: BTreeMap<String, Option<OsString>> = BTreeMap::new();
    // A record written by hand cannot take back PATH or a reserved name.
    for (name, entry) in old.env.iter().filter(|(n, _)| check_name(n).is_ok()) {
        let current = var(name);
        if current.as_deref() == Some(OsStr::new(&entry.set)) {
            now.insert(name.clone(), entry.prev_os());
        }
    }
    let mut record = Record {
        path: added_now,
        path_unset,
        env: BTreeMap::new(),
    };
    for (name, (value, _)) in plan.env() {
        let before = match now.get(&name) {
            Some(restored) => restored.clone(),
            None => var(&name),
        };
        let value = value.os();
        let mut entry = Entry {
            set: value.to_string_lossy().into_owned(),
            ..Entry::default()
        };
        if let Some(b) = before {
            match b.to_str() {
                Some(s) => entry.prev = Some(s.to_string()),
                None => entry.prev_hex = Some(crate::session::hex(b.as_bytes())),
            }
        }
        // A value that is not UTF-8 cannot be compared later, so it is set
        // but not recorded, and the next resolution leaves it as it is.
        if value.to_str().is_some() {
            record.env.insert(name.clone(), entry);
        }
        now.insert(name, Some(value));
    }

    let mut out = Changes::default();
    // A PATH that was unset goes back to unset when nothing is left in it,
    // because an empty PATH would mean another search.
    if path_unset && path_empty {
        if var("PATH").is_some() {
            out.remove.push("PATH".to_string());
        }
    } else if var("PATH").as_deref() != Some(path.as_os_str()) {
        out.set.push(("PATH".to_string(), path));
    }
    for (name, value) in now {
        match value {
            Some(v) => {
                if var(&name).as_deref() != Some(v.as_os_str()) {
                    out.set.push((name, v));
                }
            }
            None => {
                if var(&name).is_some() {
                    out.remove.push(name);
                }
            }
        }
    }
    // The record stays while it holds something to take back, or while a
    // PATH that was unset holds entries.
    if record.path.is_empty() && record.env.is_empty() && (!path_unset || path_empty) {
        if var(RESOLVED).is_some() {
            out.remove.push(RESOLVED.to_string());
        }
    } else {
        let json = serde_json::to_string(&record).expect("the record always serializes");
        if var(RESOLVED).as_deref() != Some(OsStr::new(&json)) {
            out.set.push((RESOLVED.to_string(), OsString::from(json)));
        }
    }
    out
}

/// Resolves the environment of this process for its current folder, and
/// applies it. jlog calls it once, at its start, before any thread starts.
/// Without a current folder, only the user defaults apply.
pub fn apply_here() -> Result<(), Error> {
    let cwd = std::env::current_dir().unwrap_or_default();
    let var = |k: &str| std::env::var_os(k);
    let plan = plan(&cwd, &var)?;
    let changes = changes(&plan, &var);
    for (k, v) in &changes.set {
        // SAFETY: jlog calls this at its start, before it starts a thread,
        // so no other thread reads the environment.
        unsafe { std::env::set_var(k, v) };
    }
    for k in &changes.remove {
        // SAFETY: as above.
        unsafe { std::env::remove_var(k) };
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn vars(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let m: HashMap<String, OsString> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), OsString::from(v)))
            .collect();
        move |k| m.get(k).cloned()
    }

    #[test]
    fn xdg_takes_an_absolute_value_else_the_fallback_under_home() {
        let h = Some(OsStr::new("/h"));
        assert_eq!(xdg(Some("/x".into()), h, ".config"), Some("/x".into()));
        assert_eq!(
            xdg(Some("rel".into()), h, ".config"),
            Some("/h/.config".into())
        );
        assert_eq!(
            xdg(Some("".into()), h, ".config"),
            Some("/h/.config".into())
        );
        assert_eq!(xdg(None, Some(OsStr::new("rel")), ".config"), None);
        assert_eq!(xdg(None, None, ".config"), None);
    }

    #[test]
    fn clean_takes_out_dots_by_text() {
        assert_eq!(clean(Path::new("/a/./b/../c")), PathBuf::from("/a/c"));
    }

    #[test]
    fn reserved_names_are_refused() {
        for n in [
            "PATH",
            "JLOG_RUN",
            "JLOG_RESOLVED",
            "CODEX_THREAD_ID",
            "",
            "A=B",
        ] {
            assert!(check_name(n).is_err(), "{n}");
        }
        assert!(check_name("CARGO_HOME").is_ok());
    }

    #[test]
    fn a_hex_prev_comes_back_as_bytes() {
        let e = Entry {
            set: "x".into(),
            prev: None,
            prev_hex: Some("ff00".into()),
        };
        assert_eq!(e.prev_os().unwrap().as_bytes(), &[0xff, 0x00]);
    }

    #[test]
    fn a_non_utf8_entry_that_jlog_added_is_taken_back() {
        use std::os::unix::ffi::OsStringExt;
        let path = OsString::from_vec(b"\xff/bin:/usr/bin".to_vec());
        let rec = r#"{"path":["ff2f62696e"],"env":{}}"#;
        let var = move |k: &str| match k {
            "PATH" => Some(path.clone()),
            RESOLVED => Some(OsString::from(rec)),
            _ => None,
        };
        let c = changes(&Plan::default(), &var);
        assert_eq!(c.set, [("PATH".to_string(), OsString::from("/usr/bin"))]);
        assert_eq!(c.remove, [RESOLVED.to_string()]);
    }

    #[test]
    fn the_values_in_the_record_count_as_secrets() {
        let rec = r#"{"path":[],"env":{"T":{"set":"sandbox-1","prev":"prod-secret-1"}}}"#;
        let got = crate::redact::secrets([(RESOLVED, rec)]);
        assert!(got.contains(&"prod-secret-1".to_string()));
        assert!(got.contains(&"sandbox-1".to_string()));
    }

    #[test]
    fn an_empty_plan_changes_nothing() {
        let v = vars(&[("PATH", "/usr/bin:/bin")]);
        assert_eq!(changes(&Plan::default(), &v), Changes::default());
    }
}
