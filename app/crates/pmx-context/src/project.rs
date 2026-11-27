//! Find the project the person works in, from the front app.
//!
//! Order: pinned project; the window's document (`AXDocument`, Accessibility); terminals: the
//! working folder of the shell or agent under the front window; editors: the folder named in the
//! window title, resolved with the editor's own recent-folder list. Then up to the Git root.

use crate::app::window_document;
use crate::procs;
use crate::target::{app_class, split_title, AppClass};
use crate::{home_dir, AppInfo, Project};
use std::path::{Path, PathBuf};

/// The project for the front app, or None (other apps, home folder, nothing found).
pub fn detect_project(app: &AppInfo, window_title: Option<&str>, pinned: Option<&Path>) -> Option<Project> {
    detect_project_with_source(app, window_title, pinned).0
}

/// Like [`detect_project`], plus which method found it: "pinned", "window-document",
/// "applescript-tty", "process-tree", "process-tree-unsure" (several idle sessions: None),
/// "editor-title", or "none".
pub fn detect_project_with_source(app: &AppInfo, window_title: Option<&str>, pinned: Option<&Path>) -> (Option<Project>, &'static str) {
    if let Some(p) = pinned {
        return (project_from_dir(p), "pinned");
    }
    // Windows: another process's working folder cannot be read without reading its memory, and there is no
    // window document. No project unless one is pinned in settings.
    if cfg!(windows) {
        return (None, "none");
    }
    let class = app_class(app);
    if matches!(class, AppClass::Browser | AppClass::Other) {
        return (None, "none");
    }
    let home = home_dir();
    if let Some(doc) = window_document(app) {
        if let Some(p) = project_from_path(&doc, &home) {
            return (Some(p), "window-document");
        }
    }
    match class {
        AppClass::Terminal => {
            let procs_all = procs::all();
            let roots = procs::terminal_roots(&procs_all, app.pid, &app.bundle_id);
            let mut desc = procs::descendants(&procs_all, &roots);
            for p in desc.iter_mut().filter(|p| p.tdev.is_some()) {
                p.exe = procs::exe_path(p.pid);
            }
            let tty = procs::applescript_front_tty(&app.bundle_id);
            let source = if tty.is_some() { "applescript-tty" } else { "process-tree" };
            let ranked = procs::pick_terminal_procs(&desc, tty.as_deref(), &procs::tty_last_input);
            if tty.is_none() && is_stale_guess(&ranked) {
                // Several sessions and none typed in lately: a guess could put another
                // project's facts in the prompt. No project is the safer answer.
                return (None, "process-tree-unsure");
            }
            // The first candidate with a readable folder decides. Its folder may be home
            // (no project): that is the answer, not a reason to look at another tab.
            for cand in ranked {
                if let Some(cwd) = procs::proc_cwd(cand.pid) {
                    return (project_from_dir_in(&cwd, &home), source);
                }
            }
            (None, "none")
        }
        AppClass::Editor(support_dir) => {
            let folders = if support_dir.is_empty() { vec![] } else { vscode_recent_folders(&home, support_dir) };
            match window_title.and_then(|t| folder_from_editor_title(t, &app.name, &folders, &home)) {
                Some(dir) => (project_from_dir_in(&dir, &home), "editor-title"),
                None => (None, "none"),
            }
        }
        _ => (None, "none"),
    }
}

/// With more than one terminal session and no input in any of them for 15 minutes, the
/// "most recent" one is only a guess.
const STALE_INPUT_SECS: u128 = 15 * 60;

fn is_stale_guess(ranked: &[procs::Proc]) -> bool {
    let ttys: std::collections::HashSet<u32> = ranked.iter().filter_map(|p| p.tdev).collect();
    if ttys.len() < 2 {
        return false;
    }
    let best = ranked.first().and_then(|p| p.tdev).map(procs::tty_last_input).unwrap_or(0);
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    now.saturating_sub(best) > STALE_INPUT_SECS * 1_000_000_000
}

/// Project for a file or folder path (a file's folder is used).
fn project_from_path(path: &Path, home: &Path) -> Option<Project> {
    let dir = if path.is_dir() { path.to_path_buf() } else { path.parent()?.to_path_buf() };
    if !dir.is_dir() {
        return None;
    }
    project_from_dir_in(&dir, home)
}

/// Project for a folder: Git root above it, else the folder itself.
pub(crate) fn project_from_dir(dir: &Path) -> Option<Project> {
    project_from_dir_in(dir, &home_dir())
}

/// The home folder and `/` are not projects. A Git repo at the home folder itself does not
/// count as a project root (it would swallow every folder under home).
pub(crate) fn project_from_dir_in(dir: &Path, home: &Path) -> Option<Project> {
    let cwd = std::fs::canonicalize(dir).ok()?;
    if !cwd.is_dir() {
        return None;
    }
    let home = std::fs::canonicalize(home).unwrap_or_else(|_| home.to_path_buf());
    if cwd == home || cwd.parent().is_none() {
        return None;
    }
    let root = git_root(&cwd, &home).unwrap_or_else(|| cwd.clone());
    let name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Some(Project { root, cwd, name })
}

/// Walk up from `dir` to the first folder holding `.git` (a folder, or a file in worktrees).
/// Stops before `home` and `/`.
pub(crate) fn git_root(dir: &Path, home: &Path) -> Option<PathBuf> {
    let mut cur = Some(dir);
    while let Some(d) = cur {
        if d == home || d.parent().is_none() {
            return None;
        }
        if d.join(".git").exists() {
            return Some(d.to_path_buf());
        }
        cur = d.parent();
    }
    None
}

/// Folders VS Code style editors know, newest window first, from
/// `~/Library/Application Support/<app>/User/globalStorage/storage.json`.
pub(crate) fn vscode_recent_folders(home: &Path, support_dir: &str) -> Vec<PathBuf> {
    let file = home.join("Library/Application Support").join(support_dir).join("User/globalStorage/storage.json");
    let Ok(raw) = std::fs::read(&file) else { return vec![] };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&raw) else { return vec![] };
    folders_from_storage(&v)
}

pub(crate) fn folders_from_storage(v: &serde_json::Value) -> Vec<PathBuf> {
    let mut urls: Vec<String> = Vec::new();
    // Open windows first: they are what the person sees.
    if let Some(ws) = v.get("windowsState") {
        if let Some(f) = ws.pointer("/lastActiveWindow/folder").and_then(|x| x.as_str()) {
            urls.push(f.to_string());
        }
        if let Some(arr) = ws.get("openedWindows").and_then(|x| x.as_array()) {
            for w in arr {
                if let Some(f) = w.get("folder").and_then(|x| x.as_str()) {
                    urls.push(f.to_string());
                }
            }
        }
    }
    // Then every other file:// folder in the file (backups, profile associations, ...).
    fn walk(v: &serde_json::Value, out: &mut Vec<String>) {
        match v {
            serde_json::Value::String(s) if s.starts_with("file://") => out.push(s.clone()),
            serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            serde_json::Value::Object(o) => {
                for (k, x) in o {
                    if k.starts_with("file://") {
                        out.push(k.clone());
                    }
                    walk(x, out);
                }
            }
            _ => {}
        }
    }
    walk(v, &mut urls);
    let mut seen = std::collections::HashSet::new();
    urls.into_iter()
        .filter_map(|u| crate::app::file_url_to_path(&u))
        .filter(|p| p.extension().is_none_or(|e| e != "code-workspace"))
        .filter(|p| seen.insert(p.clone()))
        .collect()
}

/// The folder an editor window shows, from its title.
/// VS Code / Cursor: "file \u{2014} folder \u{2014} Visual Studio Code" (folder matched by name against the
/// editor's known folders). Any editor: a part that is an absolute or `~/` path to a folder.
pub(crate) fn folder_from_editor_title(title: &str, app_name: &str, known: &[PathBuf], home: &Path) -> Option<PathBuf> {
    let parts: Vec<&str> = split_title(title)
        .into_iter()
        .map(|p| p.trim_start_matches(['\u{25cf}', '\u{2022}', '*']).trim())
        .filter(|p| !p.is_empty())
        .collect();
    // Explicit paths first.
    for part in &parts {
        let expanded = if let Some(rest) = part.strip_prefix("~/") { home.join(rest) } else { PathBuf::from(part) };
        if expanded.is_absolute() {
            if expanded.is_dir() {
                return Some(expanded);
            }
            if let Some(parent) = expanded.parent() {
                if expanded.is_file() && parent.is_dir() {
                    return Some(parent.to_path_buf());
                }
            }
        }
    }
    let app_low = app_name.to_lowercase();
    let is_app_part = |p: &str| {
        let l = p.to_lowercase();
        l == app_low || l == "visual studio code" || l == "cursor" || l == "windsurf" || l == "vscodium" || l.starts_with("visual studio code")
    };
    // Folder names, last part first (the app name is last, the folder just before it).
    for part in parts.iter().rev() {
        if is_app_part(part) {
            continue;
        }
        let name = part.trim_end_matches(" (Workspace)").trim();
        let name = name.split(" [").next().unwrap_or(name).trim();
        if let Some(dir) = known.iter().find(|d| d.file_name().is_some_and(|n| n.to_string_lossy() == name) && d.is_dir()) {
            return Some(dir.clone());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn git_root_and_home_rules() {
        let tmp = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(tmp.path()).unwrap();
        let home = base.join("home");
        let repo = home.join("code/shop");
        let deep = repo.join("src/app");
        fs::create_dir_all(&deep).unwrap();
        fs::create_dir_all(repo.join(".git")).unwrap();
        // A repo at home itself must not count.
        fs::create_dir_all(home.join(".git")).unwrap();
        let loose = home.join("notes/ideas");
        fs::create_dir_all(&loose).unwrap();

        let p = project_from_dir_in(&deep, &home).unwrap();
        assert_eq!(p.root, repo);
        assert_eq!(p.cwd, deep);
        assert_eq!(p.name, "shop");

        let p = project_from_dir_in(&loose, &home).unwrap();
        assert_eq!(p.root, loose, "no git: the folder itself");
        assert_eq!(p.name, "ideas");

        assert!(project_from_dir_in(&home, &home).is_none(), "home is not a project");
        assert!(project_from_dir_in(Path::new("/"), &home).is_none());
        assert!(project_from_dir_in(&base.join("missing"), &home).is_none());

        // Worktree: .git is a file.
        let wt = home.join("code/shop-wt");
        fs::create_dir_all(&wt).unwrap();
        fs::write(wt.join(".git"), "gitdir: /x").unwrap();
        assert_eq!(project_from_dir_in(&wt, &home).unwrap().root, wt);

        // A file path uses its folder.
        fs::write(deep.join("main.rs"), "").unwrap();
        assert_eq!(project_from_path(&deep.join("main.rs"), &home).unwrap().cwd, deep);
    }

    #[test]
    fn vscode_storage_folders() {
        let v: serde_json::Value = serde_json::json!({
            "windowsState": {
                "lastActiveWindow": {"folder": "file:///Users/x/shop"},
                "openedWindows": [{"folder": "file:///Users/x/My%20Site"}]
            },
            "profileAssociations": {"workspaces": {"file:///Users/x/old": "__default__profile__", "file:///Users/x/shop": "__default__profile__"}},
            "backupWorkspaces": {"folders": [{"folderUri": "file:///Users/x/blog"}], "workspaces": [{"configURIPath": "file:///Users/x/a.code-workspace"}]}
        });
        let got = folders_from_storage(&v);
        let names: Vec<String> = got.iter().map(|p| p.to_string_lossy().into_owned()).collect();
        assert_eq!(names[0], "/Users/x/shop");
        assert_eq!(names[1], "/Users/x/My Site");
        assert!(names.contains(&"/Users/x/old".to_string()));
        assert!(names.contains(&"/Users/x/blog".to_string()));
        assert!(!names.iter().any(|n| n.ends_with(".code-workspace")));
        assert_eq!(names.iter().filter(|n| *n == "/Users/x/shop").count(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn editor_titles() {
        let tmp = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(tmp.path()).unwrap();
        let shop = base.join("shop");
        let site = base.join("My Site");
        fs::create_dir_all(&shop).unwrap();
        fs::create_dir_all(&site).unwrap();
        let known = vec![site.clone(), shop.clone()];
        let home = base.join("home");
        let em = "\u{2014}";
        assert_eq!(folder_from_editor_title(&format!("main.rs {em} shop {em} Visual Studio Code"), "Code", &known, &home), Some(shop.clone()));
        assert_eq!(folder_from_editor_title(&format!("\u{25cf} index.html {em} My Site {em} Cursor"), "Cursor", &known, &home), Some(site.clone()));
        assert_eq!(folder_from_editor_title(&format!("shop {em} Visual Studio Code"), "Code", &known, &home), Some(shop.clone()));
        assert_eq!(folder_from_editor_title(&format!("Welcome {em} Visual Studio Code"), "Code", &known, &home), None);
        // Absolute path in the title (Zed and others).
        assert_eq!(folder_from_editor_title(&format!("{} {em} Zed", shop.display()), "Zed", &[], &home), Some(shop.clone()));
    }

    #[test]
    fn other_apps_have_no_project() {
        let finder = AppInfo { name: "Finder".into(), bundle_id: "com.apple.finder".into(), pid: 1 };
        assert_eq!(detect_project(&finder, Some("Documents"), None), None);
        let brave = AppInfo { name: "Brave Browser".into(), bundle_id: "com.brave.Browser".into(), pid: 1 };
        assert_eq!(detect_project(&brave, Some("Claude"), None), None);
    }

    #[test]
    fn pinned_wins() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = fs::canonicalize(tmp.path()).unwrap().join("pinned-repo");
        fs::create_dir_all(repo.join(".git")).unwrap();
        let finder = AppInfo { name: "Finder".into(), bundle_id: "com.apple.finder".into(), pid: 1 };
        let p = detect_project(&finder, None, Some(&repo)).unwrap();
        assert_eq!(p.root, repo);
        assert_eq!(p.name, "pinned-repo");
    }
}
