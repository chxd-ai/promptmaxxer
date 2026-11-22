//! Process table (libproc), working folders and terminal ttys (macOS).

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub(crate) struct Proc {
    pub pid: i32,
    pub ppid: i32,
    pub pgid: i32,
    /// tty device, or None.
    pub tdev: Option<u32>,
    /// Foreground process group of the tty.
    pub tpgid: i32,
    pub name: String,
    pub start: u64,
    /// Executable path, filled only where needed (agent detection).
    pub exe: Option<String>,
}

const NODEV: u32 = u32::MAX;

/// Shell names (comm, without a leading "-").
pub(crate) fn is_shell(name: &str) -> bool {
    matches!(name.trim_start_matches('-'), "zsh" | "bash" | "fish" | "sh" | "nu" | "tcsh" | "csh" | "ksh" | "dash" | "elvish" | "xonsh" | "pwsh")
}

/// Coding agent CLIs, by process name.
pub(crate) fn is_agent(name: &str) -> bool {
    matches!(name, "claude" | "codex" | "gemini" | "aider" | "opencode" | "amp" | "goose" | "cursor-agent")
}

/// Coding agent CLIs, by name or executable. Claude Code's native build names its process after
/// its version ("2.1.329") and runs from `~/.local/share/claude/versions/<version>`.
pub(crate) fn is_agent_proc(p: &Proc) -> bool {
    if is_agent(&p.name) {
        return true;
    }
    match &p.exe {
        Some(exe) => {
            let base = exe.rsplit('/').next().unwrap_or("");
            is_agent(base) || exe.contains("/claude/versions/") || exe.contains("/codex/")
        }
        None => false,
    }
}

#[cfg(target_os = "macos")]
mod mac {
    use super::{Proc, NODEV};
    use std::ffi::{c_char, c_int, c_void, CStr};
    use std::path::PathBuf;

    extern "C" {
        fn devname(dev: libc::dev_t, kind: libc::mode_t) -> *const c_char;
    }

    fn cstr(buf: &[c_char]) -> String {
        let bytes: Vec<u8> = buf.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    pub fn all() -> Vec<Proc> {
        let n = unsafe { libc::proc_listallpids(std::ptr::null_mut(), 0) };
        if n <= 0 {
            return vec![];
        }
        let mut pids = vec![0i32; n as usize + 256];
        let got = unsafe { libc::proc_listallpids(pids.as_mut_ptr() as *mut c_void, (pids.len() * 4) as c_int) };
        if got <= 0 {
            return vec![];
        }
        pids.truncate(got as usize);
        pids.into_iter().filter(|&p| p > 0).filter_map(info).collect()
    }

    pub fn info(pid: i32) -> Option<Proc> {
        let mut bi: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as c_int;
        let r = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDTBSDINFO, 0, &mut bi as *mut _ as *mut c_void, size) };
        if r != size {
            // Other users' processes (root-owned `login` between a terminal and its shell)
            // only give the short info: enough to link the process tree, no tty.
            return short_info(pid);
        }
        let name = {
            let n = cstr(&bi.pbi_name);
            if n.is_empty() {
                cstr(&bi.pbi_comm)
            } else {
                n
            }
        };
        Some(Proc {
            pid,
            ppid: bi.pbi_ppid as i32,
            pgid: bi.pbi_pgid as i32,
            tdev: if bi.e_tdev == NODEV || bi.e_tdev == 0 { None } else { Some(bi.e_tdev) },
            tpgid: bi.e_tpgid as i32,
            name,
            start: bi.pbi_start_tvsec,
            exe: None,
        })
    }

    fn short_info(pid: i32) -> Option<Proc> {
        let mut si: libc::proc_bsdshortinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdshortinfo>() as c_int;
        let r = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDT_SHORTBSDINFO, 0, &mut si as *mut _ as *mut c_void, size) };
        if r != size {
            return None;
        }
        Some(Proc { pid, ppid: si.pbsi_ppid as i32, pgid: si.pbsi_pgid as i32, tdev: None, tpgid: 0, name: cstr(&si.pbsi_comm), start: 0, exe: None })
    }

    pub fn cwd(pid: i32) -> Option<PathBuf> {
        let mut vi: libc::proc_vnodepathinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_vnodepathinfo>() as c_int;
        let r = unsafe { libc::proc_pidinfo(pid, libc::PROC_PIDVNODEPATHINFO, 0, &mut vi as *mut _ as *mut c_void, size) };
        if r != size {
            return None;
        }
        let raw: &[c_char] = unsafe { std::slice::from_raw_parts(vi.pvi_cdir.vip_path.as_ptr() as *const c_char, 1024) };
        let s = cstr(raw);
        if s.is_empty() {
            None
        } else {
            Some(PathBuf::from(s))
        }
    }

    pub fn exe_path(pid: i32) -> Option<String> {
        let mut buf = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
        let n = unsafe { libc::proc_pidpath(pid, buf.as_mut_ptr() as *mut c_void, buf.len() as u32) };
        if n <= 0 {
            return None;
        }
        buf.truncate(n as usize);
        String::from_utf8(buf).ok()
    }

    /// "/dev/ttys003" for a tty device number.
    pub fn tty_path(dev: u32) -> Option<String> {
        let p = unsafe { devname(dev as libc::dev_t, libc::S_IFCHR) };
        if p.is_null() {
            return None;
        }
        let name = unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned();
        if name.is_empty() || name.starts_with('?') {
            None
        } else {
            Some(format!("/dev/{name}"))
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod mac {
    use super::Proc;
    use std::path::PathBuf;
    pub fn all() -> Vec<Proc> {
        vec![]
    }
    pub fn cwd(_pid: i32) -> Option<PathBuf> {
        None
    }
    pub fn exe_path(_pid: i32) -> Option<String> {
        None
    }
    pub fn tty_path(_dev: u32) -> Option<String> {
        None
    }
}

pub(crate) use mac::{all, cwd, exe_path, tty_path};

/// Last input time on a tty (its device's access time, as `w` uses for idle), in nanoseconds.
pub(crate) fn tty_last_input(dev: u32) -> u128 {
    use std::os::unix::fs::MetadataExt;
    let Some(p) = tty_path(dev) else { return 0 };
    match std::fs::metadata(p) {
        Ok(m) => (m.atime() as u128) * 1_000_000_000 + m.atime_nsec() as u128,
        Err(_) => 0,
    }
}

/// All processes whose ancestor chain reaches one of `roots`.
pub(crate) fn descendants(procs: &[Proc], roots: &[i32]) -> Vec<Proc> {
    use std::collections::{HashMap, HashSet};
    let mut kids: HashMap<i32, Vec<usize>> = HashMap::new();
    for (i, p) in procs.iter().enumerate() {
        kids.entry(p.ppid).or_default().push(i);
    }
    let mut seen: HashSet<i32> = HashSet::new();
    let mut stack: Vec<i32> = roots.to_vec();
    let mut out = Vec::new();
    while let Some(pid) = stack.pop() {
        if let Some(ch) = kids.get(&pid) {
            for &i in ch {
                let p = &procs[i];
                if p.pid != pid && seen.insert(p.pid) {
                    out.push(p.clone());
                    stack.push(p.pid);
                }
            }
        }
    }
    out
}

/// Pick the process whose folder best stands for what the person sees in the terminal, from
/// the terminal's descendant processes. `want_tty` (from AppleScript) picks the tty; else the
/// tty with the most recent input. On that tty: an agent CLI (claude, codex) wins, then the
/// foreground process, then the newest shell. Returns candidates best first.
pub(crate) fn pick_terminal_procs(desc: &[Proc], want_tty: Option<&str>, last_input: &dyn Fn(u32) -> u128) -> Vec<Proc> {
    use std::collections::BTreeMap;
    let mut by_tty: BTreeMap<u32, Vec<&Proc>> = BTreeMap::new();
    for p in desc {
        if let Some(t) = p.tdev {
            by_tty.entry(t).or_default().push(p);
        }
    }
    // Only ttys that have a shell or an agent on them.
    by_tty.retain(|_, ps| ps.iter().any(|p| is_shell(&p.name) || is_agent_proc(p)));
    let mut ttys: Vec<u32> = by_tty.keys().copied().collect();
    let wanted: Option<u32> = want_tty.and_then(|w| ttys.iter().copied().find(|&t| tty_path(t).as_deref() == Some(w)));
    ttys.sort_by_key(|&t| std::cmp::Reverse((Some(t) == wanted, last_input(t))));
    let mut out = Vec::new();
    for t in ttys {
        let ps = &by_tty[&t];
        let mut ranked: Vec<&Proc> = ps.to_vec();
        ranked.sort_by_key(|p| {
            let rank = if is_agent_proc(p) {
                0
            } else if p.pgid == p.tpgid && p.tpgid > 0 {
                1
            } else if is_shell(&p.name) {
                2
            } else {
                3
            };
            (rank, std::cmp::Reverse(p.start))
        });
        out.extend(ranked.into_iter().cloned());
    }
    out
}

/// The tty of the front tab, asked from the app by AppleScript, only for Terminal.app and
/// iTerm2 and only when this process already has the Automation permission for that app
/// (checked without prompting). Times out after 1.5 s.
pub(crate) fn applescript_front_tty(bundle_id: &str) -> Option<String> {
    let script = match bundle_id.to_lowercase().as_str() {
        "com.apple.terminal" => "tell application \"Terminal\" to get tty of selected tab of front window",
        "com.googlecode.iterm2" => "tell application \"iTerm2\" to get tty of current session of current window",
        _ => return None,
    };
    if !automation_allowed(bundle_id) {
        return None;
    }
    let mut child = std::process::Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(1500);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let mut s = String::new();
                use std::io::Read;
                child.stdout.take()?.read_to_string(&mut s).ok()?;
                let s = s.trim().to_string();
                return if s.starts_with("/dev/") { Some(s) } else { None };
            }
            Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// True when Apple Events to `bundle_id` are already allowed. Never shows the consent prompt.
#[cfg(target_os = "macos")]
pub(crate) fn automation_allowed(bundle_id: &str) -> bool {
    use std::ffi::c_void;
    #[repr(C)]
    struct AEDesc {
        descriptor_type: u32,
        data_handle: *mut c_void,
    }
    #[link(name = "CoreServices", kind = "framework")]
    extern "C" {
        fn AECreateDesc(type_code: u32, data_ptr: *const c_void, data_size: isize, result: *mut AEDesc) -> i16;
        fn AEDisposeDesc(desc: *mut AEDesc) -> i16;
        fn AEDeterminePermissionToAutomateTarget(target: *const AEDesc, event_class: u32, event_id: u32, ask_user_if_needed: u8) -> i32;
    }
    const TYPE_APPLICATION_BUNDLE_ID: u32 = u32::from_be_bytes(*b"bund");
    const TYPE_WILDCARD: u32 = u32::from_be_bytes(*b"****");
    let mut desc = AEDesc { descriptor_type: 0, data_handle: std::ptr::null_mut() };
    let bytes = bundle_id.as_bytes();
    unsafe {
        if AECreateDesc(TYPE_APPLICATION_BUNDLE_ID, bytes.as_ptr() as *const c_void, bytes.len() as isize, &mut desc) != 0 {
            return false;
        }
        let status = AEDeterminePermissionToAutomateTarget(&desc, TYPE_WILDCARD, TYPE_WILDCARD, 0);
        AEDisposeDesc(&mut desc);
        status == 0
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn automation_allowed(_bundle_id: &str) -> bool {
    false
}

/// Process ids that host the terminal sessions of the app with `app_pid`: the app itself, plus
/// helper processes launched from inside its bundle (iTerm2 server) or Orca's terminal daemons
/// (`~/Library/Application Support/orca/daemon-host/...`), which are not children of the app.
pub(crate) fn terminal_roots(procs: &[Proc], app_pid: i32, bundle_id: &str) -> Vec<i32> {
    let mut roots = vec![app_pid];
    let bundle_prefix = exe_path(app_pid).and_then(|p| p.find(".app/").map(|i| p[..i + 5].to_string()));
    let orca = bundle_id.eq_ignore_ascii_case("com.stablyai.orca");
    for p in procs {
        if p.pid == app_pid || p.ppid != 1 {
            continue;
        }
        let Some(exe) = exe_path(p.pid) else { continue };
        let helper = bundle_prefix.as_deref().is_some_and(|b| exe.starts_with(b));
        let orca_daemon = orca && exe.contains("/orca/daemon-host/");
        if helper || orca_daemon {
            roots.push(p.pid);
        }
    }
    roots
}

pub(crate) fn proc_cwd(pid: i32) -> Option<PathBuf> {
    cwd(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: i32, ppid: i32, tty: Option<u32>, name: &str, pgid: i32, tpgid: i32, start: u64) -> Proc {
        Proc { pid, ppid, pgid, tdev: tty, tpgid, name: name.into(), start, exe: None }
    }

    #[test]
    fn descendants_walks_tree() {
        let procs = vec![p(10, 1, None, "Terminal", 10, 0, 1), p(11, 10, Some(1), "login", 11, 12, 2), p(12, 11, Some(1), "zsh", 12, 12, 3), p(13, 12, Some(1), "claude", 13, 13, 4), p(99, 1, None, "other", 99, 0, 1)];
        let d = descendants(&procs, &[10]);
        let mut pids: Vec<i32> = d.iter().map(|p| p.pid).collect();
        pids.sort();
        assert_eq!(pids, vec![11, 12, 13]);
    }

    #[test]
    fn picks_recent_tty_and_agent_first() {
        let desc = vec![
            p(11, 10, Some(1), "login", 11, 12, 2),
            p(12, 11, Some(1), "zsh", 12, 12, 3),
            p(21, 10, Some(2), "login", 21, 23, 2),
            p(22, 21, Some(2), "zsh", 22, 23, 3),
            p(23, 22, Some(2), "claude", 23, 23, 4),
        ];
        let recent_two = |t: u32| if t == 2 { 200 } else { 100 };
        let got = pick_terminal_procs(&desc, None, &recent_two);
        assert_eq!(got[0].pid, 23, "agent on the most recent tty");
        assert_eq!(got[1].pid, 22);
        let recent_one = |t: u32| if t == 1 { 200 } else { 100 };
        let got = pick_terminal_procs(&desc, None, &recent_one);
        assert_eq!(got[0].pid, 12, "foreground shell on tty 1");
        // Claude Code named after its version, found by its executable.
        let mut versioned = desc.clone();
        versioned[4].name = "2.1.329".into();
        versioned[4].exe = Some("/Users/x/.local/share/claude/versions/2.1.329".into());
        versioned.push(p(24, 23, Some(2), "node", 23, 23, 5));
        let got = pick_terminal_procs(&versioned, None, &recent_two);
        assert_eq!(got[0].pid, 23, "versioned claude still wins over its foreground children");
        // A tty with no shell or agent is skipped.
        let only = vec![p(31, 10, Some(3), "tail", 31, 31, 1)];
        assert!(pick_terminal_procs(&only, None, &|_| 0).is_empty());
    }

    #[test]
    fn names() {
        assert!(is_shell("-zsh"));
        assert!(is_shell("bash"));
        assert!(!is_shell("login"));
        assert!(is_agent("claude"));
        assert!(is_agent("codex"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn own_process_is_listed_with_cwd() {
        let me = std::process::id() as i32;
        let all = all();
        assert!(all.iter().any(|p| p.pid == me), "own pid in table of {}", all.len());
        let cwd = proc_cwd(me).unwrap();
        assert_eq!(std::fs::canonicalize(cwd).unwrap(), std::fs::canonicalize(std::env::current_dir().unwrap()).unwrap());
    }
}
