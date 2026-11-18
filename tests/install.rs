//! The tests of scripts/install.sh. Each one makes a fake release in a temporary folder, points the script at it with
//! file:// URLs, and puts a stub uname and a stub uv first in the PATH. So no test reads the network or a real uv.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const TAG: &str = "v9.8.7";

/// The four targets, by the output of `uname -s` and `uname -m`.
const PLATFORMS: [(&str, &str, &str); 6] = [
    ("Darwin", "arm64", "aarch64-apple-darwin"),
    ("Darwin", "x86_64", "x86_64-apple-darwin"),
    ("Linux", "x86_64", "x86_64-unknown-linux-musl"),
    ("Linux", "aarch64", "aarch64-unknown-linux-musl"),
    ("Linux", "arm64", "aarch64-unknown-linux-musl"),
    ("Linux", "amd64", "x86_64-unknown-linux-musl"),
];

struct Lab {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Lab {
    fn new() -> Lab {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        for d in ["home", "stub", "release", "tmp"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        // uname prints the platform of the test, sysctl says whether the Mac is Apple Silicon, and uv writes down
        // each call, with the value of a UV_ variable that the script must unset.
        write_exec(
            &root.join("stub/uname"),
            "#!/bin/sh\ncase $1 in -s) echo \"$FAKE_S\" ;; -m) echo \"$FAKE_M\" ;; *) exit 1 ;; esac\n",
        );
        write_exec(
            &root.join("stub/sysctl"),
            "#!/bin/sh\n[ \"$*\" = \"-n hw.optional.arm64\" ] || exit 1\necho \"${FAKE_ARM64:-0}\"\n",
        );
        write_exec(
            &root.join("stub/uv"),
            "#!/bin/sh\necho \"$* env=${UV_PROJECT_ENVIRONMENT:-unset}\" >> \"$FAKE_UV_LOG\"\n",
        );
        Lab { _dir: dir, root }
    }

    /// Make the asset of TARGET for TAG, with a jlog that prints its version, the two plugins and the hash file.
    fn release(&self, target: &str) -> PathBuf {
        let stage = self.root.join("stage").join(target);
        write_exec(&stage.join("jlog"), "#!/bin/sh\necho \"jlog 9.8.7\"\n");
        for p in ["claude", "codex"] {
            for f in ["pyproject.toml", "uv.lock", "session.py"] {
                let path = stage.join("plugins").join(p).join(f);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(&path, format!("{p} {f}\n")).unwrap();
            }
        }
        let out = self.root.join("release").join(TAG);
        fs::create_dir_all(&out).unwrap();
        let asset = out.join(format!("jlog-{target}.tar.gz"));
        let tar = Command::new("tar")
            .arg("-czf")
            .arg(&asset)
            .arg("-C")
            .arg(&stage)
            .args(["jlog", "plugins"])
            .status()
            .unwrap();
        assert!(tar.success());
        let hash = Command::new("shasum")
            .args(["-a", "256"])
            .arg(&asset)
            .output()
            .unwrap();
        assert!(hash.status.success());
        let line = String::from_utf8(hash.stdout).unwrap();
        let sum = line.split_whitespace().next().unwrap();
        fs::write(
            out.join(format!("jlog-{target}.tar.gz.sha256")),
            format!("{sum}  jlog-{target}.tar.gz\n"),
        )
        .unwrap();
        asset
    }

    /// Run install.sh on the platform (os, arch) with the arguments. With `uv` false, the PATH has no uv.
    fn install(&self, os: &str, arch: &str, args: &[&str], uv: bool) -> Output {
        self.install_with(os, arch, args, uv, &[])
    }

    /// The same, with more variables. The PATH holds only the stubs and links to the system tools that the script
    /// uses, so a real uv or sysctl of the machine never counts.
    fn install_with(
        &self,
        os: &str,
        arch: &str,
        args: &[&str],
        uv: bool,
        vars: &[(&str, &str)],
    ) -> Output {
        let tools = self.root.join(if uv { "tools" } else { "tools-no-uv" });
        if !tools.exists() {
            fs::create_dir_all(&tools).unwrap();
            let mut stubs = vec!["uname", "sysctl"];
            if uv {
                stubs.push("uv");
            }
            for s in stubs {
                std::os::unix::fs::symlink(self.root.join("stub").join(s), tools.join(s)).unwrap();
            }
            for tool in TOOLS {
                if let Some(found) = system_tool(tool) {
                    std::os::unix::fs::symlink(found, tools.join(tool)).unwrap();
                }
            }
        }
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/install.sh");
        let mut cmd = Command::new(system_tool("sh").unwrap());
        cmd.arg(script)
            .args(args)
            .env_clear()
            .env("PATH", &tools)
            .env("HOME", self.root.join("home"))
            .env("TMPDIR", self.root.join("tmp"))
            .env("FAKE_S", os)
            .env("FAKE_M", arch)
            .env("FAKE_UV_LOG", self.root.join("uv-calls"))
            .env(
                "JLOG_RELEASE_BASE",
                format!("file://{}", self.root.join("release").display()),
            )
            .env(
                "JLOG_RELEASE_API",
                format!("file://{}", self.root.join("latest.json").display()),
            );
        for (k, v) in vars {
            cmd.env(k, v);
        }
        cmd.output().unwrap()
    }

    fn bin(&self) -> PathBuf {
        self.root.join("home/.local/bin/jlog")
    }

    fn plugins(&self) -> PathBuf {
        self.root.join("home/.local/share/jlog/plugins")
    }

    fn uv_calls(&self) -> String {
        fs::read_to_string(self.root.join("uv-calls")).unwrap_or_default()
    }

    /// The temporary folder must be empty after each run, so the script cleaned up after itself.
    fn assert_no_leftover(&self) {
        let left: Vec<_> = fs::read_dir(self.root.join("tmp")).unwrap().collect();
        assert!(left.is_empty(), "install.sh left {left:?} in TMPDIR");
    }
}

/// The system tools that install.sh and the stubs run. tar needs gzip for -z on Linux.
const TOOLS: [&str; 24] = [
    "sh",
    "curl",
    "tar",
    "gzip",
    "shasum",
    "sha256sum",
    "awk",
    "sed",
    "grep",
    "head",
    "mktemp",
    "mkdir",
    "cp",
    "chmod",
    "mv",
    "rm",
    "basename",
    "env",
    "cat",
    "dirname",
    "wc",
    "tr",
    "sort",
    "perl",
];

/// The tool in the system folders, never in a folder of the user.
fn system_tool(name: &str) -> Option<PathBuf> {
    ["/usr/bin", "/bin", "/usr/sbin", "/sbin"]
        .iter()
        .map(|d| Path::new(d).join(name))
        .find(|p| p.is_file())
}

fn write_exec(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn text(out: &Output) -> String {
    format!(
        "status {:?}\nstdout:\n{}\nstderr:\n{}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn each_platform_gets_the_asset_of_its_target() {
    for (os, arch, target) in PLATFORMS {
        let lab = Lab::new();
        lab.release(target);
        let out = lab.install(os, arch, &["--tag", TAG], true);
        assert!(out.status.success(), "{os} {arch}: {}", text(&out));
        assert!(
            String::from_utf8_lossy(&out.stdout).contains("Installed jlog 9.8.7"),
            "{}",
            text(&out)
        );
        let version = Command::new(lab.bin()).arg("--version").output().unwrap();
        assert_eq!(String::from_utf8_lossy(&version.stdout), "jlog 9.8.7\n");
        for p in ["claude", "codex"] {
            for f in ["pyproject.toml", "uv.lock", "session.py"] {
                let got = fs::read_to_string(lab.plugins().join(p).join(f)).unwrap();
                assert_eq!(got, format!("{p} {f}\n"));
            }
        }
        let calls = lab.uv_calls();
        for p in ["claude", "codex"] {
            assert!(
                calls.contains(&format!(
                    "sync --quiet --frozen --project {}/{p}/",
                    lab.plugins().display()
                )),
                "uv calls: {calls}"
            );
        }
        lab.assert_no_leftover();
    }
}

#[test]
fn a_hash_that_does_not_match_installs_nothing() {
    let lab = Lab::new();
    let asset = lab.release("aarch64-apple-darwin");
    // Change the archive after its hash was written.
    let mut bytes = fs::read(&asset).unwrap();
    bytes.push(0);
    fs::write(&asset, bytes).unwrap();
    let out = lab.install("Darwin", "arm64", &["--tag", TAG], true);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("SHA-256"),
        "{}",
        text(&out)
    );
    assert!(!lab.bin().exists());
    assert!(!lab.plugins().exists());
    lab.assert_no_leftover();
}

#[test]
fn a_hash_file_with_no_hash_installs_nothing() {
    let lab = Lab::new();
    lab.release("aarch64-apple-darwin");
    let sha = lab
        .root
        .join("release")
        .join(TAG)
        .join("jlog-aarch64-apple-darwin.tar.gz.sha256");
    fs::write(&sha, "").unwrap();
    let out = lab.install("Darwin", "arm64", &["--tag", TAG], true);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(!lab.bin().exists());
}

#[test]
fn an_unknown_platform_fails() {
    for (os, arch) in [
        ("Linux", "riscv64"),
        ("FreeBSD", "amd64"),
        ("Darwin", "ppc"),
    ] {
        let lab = Lab::new();
        lab.release("x86_64-unknown-linux-musl");
        let out = lab.install(os, arch, &["--tag", TAG], true);
        assert!(!out.status.success(), "{os} {arch}: {}", text(&out));
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("no release build"),
            "{}",
            text(&out)
        );
        assert!(!lab.bin().exists());
    }
}

#[test]
fn a_missing_asset_fails() {
    let lab = Lab::new();
    lab.release("x86_64-apple-darwin");
    let out = lab.install("Linux", "x86_64", &["--tag", TAG], true);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(!lab.bin().exists());
    lab.assert_no_leftover();
}

#[test]
fn with_no_tag_the_latest_release_is_installed() {
    let lab = Lab::new();
    lab.release("x86_64-unknown-linux-musl");
    fs::write(
        lab.root.join("latest.json"),
        format!("{{\n  \"url\": \"x\",\n  \"tag_name\": \"{TAG}\",\n  \"name\": \"jlog\"\n}}\n"),
    )
    .unwrap();
    let out = lab.install("Linux", "x86_64", &[], true);
    assert!(out.status.success(), "{}", text(&out));
    assert!(lab.bin().exists());
}

#[test]
fn a_tag_that_is_not_a_release_tag_fails() {
    let lab = Lab::new();
    for tag in ["0.3.0", "v1.2", "v1.2.3-rc1", "../v1.2.3", "v01.2.3"] {
        let out = lab.install("Linux", "x86_64", &["--tag", tag], true);
        assert!(!out.status.success(), "{tag}: {}", text(&out));
        assert!(
            String::from_utf8_lossy(&out.stderr).contains("not a release tag"),
            "{tag}: {}",
            text(&out)
        );
    }
}

#[test]
fn the_dir_option_puts_jlog_there() {
    let lab = Lab::new();
    lab.release("aarch64-unknown-linux-musl");
    let dir = lab.root.join("bin dir");
    let out = lab.install(
        "Linux",
        "aarch64",
        &["--tag", TAG, "--dir", dir.to_str().unwrap()],
        true,
    );
    assert!(out.status.success(), "{}", text(&out));
    assert!(dir.join("jlog").is_file());
    assert!(!lab.bin().exists());
}

#[test]
fn without_uv_jlog_is_installed_and_the_sync_is_left_to_the_user() {
    let lab = Lab::new();
    lab.release("aarch64-apple-darwin");
    let out = lab.install("Darwin", "arm64", &["--tag", TAG], false);
    assert!(out.status.success(), "{}", text(&out));
    assert!(lab.bin().exists());
    assert!(lab.plugins().join("claude/session.py").is_file());
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("uv is not in the PATH"),
        "{}",
        text(&out)
    );
    assert_eq!(lab.uv_calls(), "");
}

#[test]
fn a_shell_under_rosetta_gets_the_arm64_build() {
    let lab = Lab::new();
    lab.release("aarch64-apple-darwin");
    let out = lab.install_with(
        "Darwin",
        "x86_64",
        &["--tag", TAG],
        true,
        &[("FAKE_ARM64", "1")],
    );
    assert!(out.status.success(), "{}", text(&out));
    assert!(lab.bin().exists());
}

#[test]
fn the_latest_release_is_found_in_minified_json() {
    let lab = Lab::new();
    lab.release("x86_64-unknown-linux-musl");
    fs::write(
        lab.root.join("latest.json"),
        format!("{{\"url\":\"x\",\"tag_name\":\"{TAG}\",\"body\":\"see \\\"tag_name\\\": \\\"v0.0.1\\\"\"}}"),
    )
    .unwrap();
    let out = lab.install("Linux", "x86_64", &[], true);
    assert!(out.status.success(), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stdout).contains(TAG),
        "{}",
        text(&out)
    );
}

#[test]
fn a_failed_read_of_the_latest_release_fails() {
    let lab = Lab::new();
    lab.release("x86_64-unknown-linux-musl");
    // There is no latest.json, so curl fails.
    let out = lab.install("Linux", "x86_64", &[], true);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("latest release"),
        "{}",
        text(&out)
    );
    assert!(!lab.bin().exists());
}

#[test]
fn a_tag_with_a_newline_fails() {
    let lab = Lab::new();
    let out = lab.install("Linux", "x86_64", &["--tag", "v1.2.3\nx"], true);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("newline"),
        "{}",
        text(&out)
    );
}

#[test]
fn the_sync_runs_without_the_uv_variables_of_the_caller() {
    let lab = Lab::new();
    lab.release("x86_64-unknown-linux-musl");
    let out = lab.install_with(
        "Linux",
        "x86_64",
        &["--tag", TAG],
        true,
        &[("UV_PROJECT_ENVIRONMENT", ".venv")],
    );
    assert!(out.status.success(), "{}", text(&out));
    let calls = lab.uv_calls();
    assert!(calls.contains("env=unset"), "uv calls: {calls}");
    assert!(!calls.contains("env=.venv"), "uv calls: {calls}");
}

#[test]
fn a_plugin_without_a_file_fails_before_jlog_is_replaced() {
    let lab = Lab::new();
    let stage = lab.root.join("stage/aarch64-apple-darwin");
    lab.release("aarch64-apple-darwin");
    // Pack the archive again without plugins/codex/uv.lock.
    fs::remove_file(stage.join("plugins/codex/uv.lock")).unwrap();
    let asset = lab
        .root
        .join("release")
        .join(TAG)
        .join("jlog-aarch64-apple-darwin.tar.gz");
    assert!(
        Command::new("tar")
            .arg("-czf")
            .arg(&asset)
            .arg("-C")
            .arg(&stage)
            .args(["jlog", "plugins"])
            .status()
            .unwrap()
            .success()
    );
    let hash = Command::new("shasum")
        .args(["-a", "256"])
        .arg(&asset)
        .output()
        .unwrap();
    let sum = String::from_utf8(hash.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();
    fs::write(asset.with_extension("gz.sha256"), format!("{sum}  x\n")).unwrap();
    let old = lab.bin();
    write_exec(&old, "#!/bin/sh\necho \"jlog 0.0.1\"\n");
    let out = lab.install("Darwin", "arm64", &["--tag", TAG], true);
    assert!(!out.status.success(), "{}", text(&out));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("uv.lock"),
        "{}",
        text(&out)
    );
    assert_eq!(
        fs::read_to_string(&old).unwrap(),
        "#!/bin/sh\necho \"jlog 0.0.1\"\n"
    );
    let left: Vec<_> = fs::read_dir(old.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(left.len(), 1, "files in the folder of jlog: {left:?}");
    lab.assert_no_leftover();
}
