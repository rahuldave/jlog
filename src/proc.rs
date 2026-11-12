//! Reads facts about other processes: the parent, the name, the start time,
//! the argv and the stdout. macOS uses `proc_pidinfo` and `sysctl`, and Linux
//! uses `/proc`. jlog reads only the argv of a process, never its environment.

/// One process, as jlog sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proc {
    pub pid: i32,
    pub ppid: i32,
    pub name: String,
    /// The start time, in seconds since 1970.
    pub start: u64,
}

/// The nearest process named `just`, from `pid` up the tree, `pid` included.
pub fn nearest_just(pid: i32) -> Option<Proc> {
    let mut pid = pid;
    for _ in 0..64 {
        if pid <= 0 {
            return None;
        }
        let p = info(pid)?;
        if p.name == "just" {
            return Some(p);
        }
        if p.ppid == pid {
            return None;
        }
        pid = p.ppid;
    }
    None
}

/// True when the stdout of this process is a pipe that `parent` does not
/// share. `just` runs a backtick with such a pipe, to read its output. A line
/// of a recipe gets the stdout of `just` itself.
pub fn stdout_is_own_pipe(parent: i32) -> bool {
    match (
        pipe_of_stdout(std::process::id() as i32),
        pipe_of_stdout(parent),
    ) {
        (Some(mine), Some(theirs)) => mine != theirs,
        (Some(_), None) => true,
        _ => false,
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::Proc;
    use std::ffi::CStr;
    use std::mem::{size_of, zeroed};

    pub fn info(pid: i32) -> Option<Proc> {
        // SAFETY: proc_bsdinfo is plain data, and the call writes at most its size.
        let mut bi: libc::proc_bsdinfo = unsafe { zeroed() };
        let size = size_of::<libc::proc_bsdinfo>() as i32;
        let n = unsafe {
            libc::proc_pidinfo(
                pid,
                libc::PROC_PIDTBSDINFO,
                0,
                (&mut bi as *mut libc::proc_bsdinfo).cast(),
                size,
            )
        };
        if n != size {
            return None;
        }
        let comm: Vec<u8> = bi.pbi_comm.iter().map(|&c| c as u8).collect();
        let name = CStr::from_bytes_until_nul(&comm)
            .map(|c| c.to_string_lossy().into_owned())
            .unwrap_or_else(|_| String::from_utf8_lossy(&comm).into_owned());
        Some(Proc {
            pid,
            ppid: bi.pbi_ppid as i32,
            name,
            start: bi.pbi_start_tvsec,
        })
    }

    pub fn argv(pid: i32) -> Option<Vec<String>> {
        let mut mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid];
        let mut size: libc::size_t = 0;
        // SAFETY: a call with a null buffer only reports the size.
        let rc = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                3,
                std::ptr::null_mut(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc != 0 || size < 4 {
            return None;
        }
        let mut buf = vec![0u8; size];
        // SAFETY: buf has `size` bytes, and the call writes at most `size`.
        let rc = unsafe {
            libc::sysctl(
                mib.as_mut_ptr(),
                3,
                buf.as_mut_ptr().cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if rc != 0 {
            return None;
        }
        parse_procargs2(&buf[..size])
    }

    /// The buffer holds argc in 4 bytes, the path of the program and its NUL,
    /// padding NULs, then argc strings. The path with its NUL and the padding
    /// fill a whole number of 8-byte words, as a test of macOS 15 shows. The
    /// environment follows, and jlog does not read it. The boundary, not the
    /// first byte that is not NUL, marks argv[0], so an empty argv[0] stays
    /// empty and the environment never shifts into the argv.
    pub fn parse_procargs2(buf: &[u8]) -> Option<Vec<String>> {
        let argc = i32::from_ne_bytes(buf.get(..4)?.try_into().ok()?);
        let path_len = buf[4..].iter().position(|&b| b == 0)?;
        let start = 4 + (path_len + 1).div_ceil(8) * 8;
        let padding = buf.get(4 + path_len..start)?;
        let start = if padding.iter().all(|&b| b == 0) {
            start
        } else {
            // Another layout: skip the NULs after the path instead.
            let after = 4 + path_len;
            after + buf[after..].iter().position(|&b| b != 0)?
        };
        let mut rest = buf.get(start..)?;
        let mut out = Vec::new();
        for _ in 0..argc.max(0) {
            let end = rest.iter().position(|&b| b == 0).unwrap_or(rest.len());
            out.push(String::from_utf8_lossy(&rest[..end]).into_owned());
            rest = rest.get(end + 1..).unwrap_or(&[]);
        }
        Some(out)
    }

    // From <sys/proc_info.h>: struct pipe_fdinfo is a proc_fileinfo of 24
    // bytes, then a pipe_info whose vinfo_stat has 136 bytes. So pipe_handle,
    // a u64, starts at byte 160, and the struct has 184 bytes.
    const PROC_PIDFDPIPEINFO: i32 = 6;
    const PIPE_FDINFO_SIZE: usize = 184;
    const PIPE_HANDLE_AT: usize = 160;

    pub fn pipe_of_stdout(pid: i32) -> Option<u64> {
        let mut buf = [0u64; PIPE_FDINFO_SIZE / 8];
        // SAFETY: buf has PIPE_FDINFO_SIZE bytes, and the call writes at most that.
        let n = unsafe {
            libc::proc_pidfdinfo(
                pid,
                1,
                PROC_PIDFDPIPEINFO,
                buf.as_mut_ptr().cast(),
                PIPE_FDINFO_SIZE as i32,
            )
        };
        (n as usize == PIPE_FDINFO_SIZE).then(|| buf[PIPE_HANDLE_AT / 8])
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::Proc;
    use std::fs;

    pub fn info(pid: i32) -> Option<Proc> {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // The name is in parentheses and can hold spaces, so split after the last ')'.
        let open = stat.find('(')?;
        let close = stat.rfind(')')?;
        let name = stat[open + 1..close].to_string();
        let fields: Vec<&str> = stat[close + 1..].split_whitespace().collect();
        let ppid = fields.get(1)?.parse().ok()?;
        let ticks: u64 = fields.get(19)?.parse().ok()?;
        Some(Proc {
            pid,
            ppid,
            name,
            start: boot_time()? + ticks / clock_ticks(),
        })
    }

    fn boot_time() -> Option<u64> {
        let stat = fs::read_to_string("/proc/stat").ok()?;
        stat.lines()
            .find_map(|l| l.strip_prefix("btime "))?
            .trim()
            .parse()
            .ok()
    }

    fn clock_ticks() -> u64 {
        // SAFETY: sysconf has no preconditions.
        let t = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        if t > 0 { t as u64 } else { 100 }
    }

    pub fn argv(pid: i32) -> Option<Vec<String>> {
        let raw = fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        let raw = raw.strip_suffix(&[0]).unwrap_or(&raw);
        Some(
            raw.split(|&b| b == 0)
                .map(|a| String::from_utf8_lossy(a).into_owned())
                .collect(),
        )
    }

    pub fn pipe_of_stdout(pid: i32) -> Option<u64> {
        let link = fs::read_link(format!("/proc/{pid}/fd/1")).ok()?;
        let s = link.to_str()?;
        s.strip_prefix("pipe:[")?.strip_suffix(']')?.parse().ok()
    }
}

use imp::pipe_of_stdout;
pub use imp::{argv, info};

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn reads_this_process() {
        let me = info(std::process::id() as i32).unwrap();
        assert!(me.ppid > 0);
        assert!(me.start > 1_600_000_000);
        let a = argv(std::process::id() as i32).unwrap();
        assert!(!a.is_empty());
    }

    /// A buffer as the kernel lays it out: argv[0] starts at the next 8-byte
    /// boundary after the NUL of the path.
    fn procargs(path: &str, args: &[&str]) -> Vec<u8> {
        let mut buf = (args.len() as i32).to_ne_bytes().to_vec();
        buf.extend_from_slice(path.as_bytes());
        buf.push(0);
        while !(buf.len() - 4).is_multiple_of(8) {
            buf.push(0);
        }
        for a in args {
            buf.extend_from_slice(a.as_bytes());
            buf.push(0);
        }
        buf.extend_from_slice(b"SECRET=value\0");
        buf
    }

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn parses_procargs2() {
        for path in ["/j", "/usr/bin/just", "/opt/homebrew/bin/just"] {
            let buf = procargs(path, &["just", "check"]);
            assert_eq!(
                imp::parse_procargs2(&buf).unwrap(),
                strings(&["just", "check"])
            );
        }
    }

    #[test]
    fn keeps_empty_arguments() {
        let buf = procargs("/j", &["just", "", "x"]);
        assert_eq!(
            imp::parse_procargs2(&buf).unwrap(),
            strings(&["just", "", "x"])
        );
    }

    #[test]
    fn an_empty_argv0_does_not_pull_in_the_environment() {
        for path in ["/j", "/usr/bin/just", "/abcdef", "/abcdefg"] {
            let buf = procargs(path, &["", "a"]);
            assert_eq!(
                imp::parse_procargs2(&buf).unwrap(),
                strings(&["", "a"]),
                "{path}"
            );
        }
    }

    #[test]
    fn reads_an_empty_argv0_of_a_real_process() {
        use std::os::unix::process::CommandExt;
        let mut child = std::process::Command::new("/bin/sleep")
            .arg0("")
            .arg("5")
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        let got = argv(child.id() as i32);
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(got.unwrap(), strings(&["", "5"]));
    }
}
