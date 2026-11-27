//! History: one JSON line per rewrite in `~/.local/share/promptmaxxer/history.jsonl`, file mode 600.
//! Same keys the runner (`bin/promptmaxx`) writes (`model` as a list), plus `id`, `target`, `project`, `thumb`.
//! Old lines without an `id` get one when read: `<ts>-<line index>`.
use serde_json::{json, Value};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

/// Rows kept: the newest this many. Older rows are dropped when a row is added.
pub const MAX_ROWS: usize = 2000;

/// One writer at a time in this process: a thumb or delete rewrite that overlapped an append would drop the
/// appended row.
static WRITE: Mutex<()> = Mutex::new(());

fn write_lock() -> MutexGuard<'static, ()> {
    WRITE.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct Row<'a> {
    pub app: &'a str,
    pub level: &'a str,
    pub engine: &'a str,
    pub model: Option<&'a str>,
    pub ms: u64,
    pub ok: bool,
    pub retried: Option<bool>,
    pub draft: &'a str,
    pub output: Option<&'a str>,
    pub error: Option<&'a str>,
    pub target: Option<&'a str>,
    /// What the engine was told, e.g. "coding agent: Claude Code" (for re-runs).
    pub target_hint: Option<&'a str>,
    pub project: Option<&'a str>,
}

pub fn path() -> PathBuf {
    crate::paths::history_file()
}

fn new_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    format!("{}-{}", chrono::Local::now().format("%Y%m%d%H%M%S%3f"), SEQ.fetch_add(1, Ordering::Relaxed))
}

pub fn row_json(r: &Row) -> Value {
    json!({
        "id": new_id(),
        "ts": chrono::Local::now().format("%Y-%m-%dT%H:%M:%S").to_string(),
        "ok": r.ok, "app": r.app, "level": r.level, "engine": r.engine,
        // The runner stores the models it used as a list.
        "model": r.model.map(|m| vec![m]).unwrap_or_default(),
        "ms": r.ms, "retried": r.retried, "draft": r.draft, "output": r.output, "error": r.error,
        "target": r.target, "target_hint": r.target_hint, "project": r.project, "thumb": Value::Null,
    })
}

/// Append one row; returns its id. Nothing is written when history is off (the caller checks).
pub fn append(r: &Row) -> Option<String> {
    let v = row_json(r);
    match append_value(&path(), &v) {
        Ok(()) => v["id"].as_str().map(str::to_string),
        Err(e) => {
            crate::log::log(&format!("history: could not write ({e})"));
            None
        }
    }
}

fn set_600(path: &Path) -> std::io::Result<()> {
    crate::paths::set_private(path)
}

pub fn append_value(path: &Path, v: &Value) -> std::io::Result<()> {
    append_capped(path, v, MAX_ROWS)
}

fn append_capped(path: &Path, v: &Value, max: usize) -> std::io::Result<()> {
    let _w = write_lock();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    {
        let mut f = crate::paths::private_file(path, false)?;
        f.write_all(format!("{v}\n").as_bytes())?;
    }
    set_600(path)?;
    // Cheap line count first; the file is only parsed and rewritten when it is over the cap.
    let lines = std::fs::read(path).map(|b| b.iter().filter(|&&c| c == b'\n').count()).unwrap_or(0);
    if lines > max {
        let rows = read_all(path);
        let keep = rows.len().saturating_sub(max);
        write_all(path, &rows[keep..])?;
    }
    Ok(())
}

/// Every line as JSON (bad lines skipped), oldest first, each with an `id`.
pub fn read_all(path: &Path) -> Vec<Value> {
    let Ok(text) = std::fs::read_to_string(path) else { return vec![] };
    text.lines()
        .enumerate()
        .filter_map(|(i, line)| {
            let mut v: Value = serde_json::from_str(line).ok()?;
            if !v.is_object() {
                return None;
            }
            if v.get("id").and_then(Value::as_str).is_none_or(str::is_empty) {
                let ts = v.get("ts").and_then(Value::as_str).unwrap_or("");
                v["id"] = json!(format!("{ts}-{i}"));
            }
            Some(v)
        })
        .collect()
}

fn write_all(path: &Path, rows: &[Value]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("jsonl.tmp{}", std::process::id()));
    {
        let mut f = crate::paths::private_file(&tmp, true)?;
        for r in rows {
            f.write_all(format!("{r}\n").as_bytes())?;
        }
    }
    set_600(&tmp)?;
    std::fs::rename(&tmp, path)
}

/// HistoryItem for the UI: `model` as a string.
fn item(v: &Value) -> Value {
    let model = match v.get("model") {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(", "),
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    };
    let s = |k: &str| v.get(k).cloned().unwrap_or(Value::Null);
    json!({
        "id": s("id"), "ts": s("ts"), "app": s("app"), "level": s("level"),
        "engine": v.get("engine").cloned().unwrap_or_else(|| {
            // Rows from the old Python runner have no engine; it called the API with a key.
            if model.starts_with("claude-") { json!("anthropic-key") } else { json!("") }
        }),
        "model": model, "ms": s("ms"), "ok": v.get("ok").and_then(Value::as_bool).unwrap_or(false),
        "draft": s("draft"), "output": v.get("output").cloned().filter(|o| !o.is_null()).unwrap_or(json!("")),
        "target": s("target"), "project": s("project"), "thumb": s("thumb"),
    })
}

/// Newest first, filtered by a case-insensitive search over draft, output, target, project and app.
pub fn list(path: &Path, query: &str) -> Vec<Value> {
    let q = query.trim().to_lowercase();
    let mut out: Vec<Value> = read_all(path)
        .iter()
        .map(item)
        .filter(|it| {
            q.is_empty()
                || ["draft", "output", "target", "project", "app"]
                    .iter()
                    .filter_map(|k| it.get(*k).and_then(Value::as_str))
                    .any(|t| t.to_lowercase().contains(&q))
        })
        .collect();
    out.reverse();
    out
}

pub fn get(path: &Path, id: &str) -> Option<Value> {
    read_all(path).into_iter().find(|v| v["id"] == id)
}

/// `None` deletes every line.
pub fn delete(path: &Path, id: Option<&str>) -> std::io::Result<()> {
    if !path.exists() {
        return Ok(());
    }
    let _w = write_lock();
    let rows: Vec<Value> = match id {
        None => vec![],
        Some(id) => read_all(path).into_iter().filter(|v| v["id"] != id).collect(),
    };
    write_all(path, &rows)
}

pub fn thumb(path: &Path, id: &str, up: Option<bool>) -> std::io::Result<bool> {
    let _w = write_lock();
    let mut rows = read_all(path);
    let mut found = false;
    for v in rows.iter_mut().filter(|v| v["id"] == id) {
        v["thumb"] = up.map(Value::Bool).unwrap_or(Value::Null);
        found = true;
    }
    if found {
        write_all(path, &rows)?;
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn tmp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("pmx-hist-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("history.jsonl")
    }

    fn row<'a>(draft: &'a str) -> Row<'a> {
        Row { app: "Notes", level: "standard", engine: "claude-plan", model: Some("haiku"), ms: 1234, ok: true,
            retried: Some(false), draft, output: Some("Fix the login page."), error: None, target: Some("Claude"), target_hint: Some("chat: Claude"), project: None }
    }

    #[test]
    fn writes_runner_keys_plus_new_ones_mode_600() {
        let p = tmp("keys");
        append_value(&p, &row_json(&row("fix it"))).unwrap();
        append_value(&p, &row_json(&row("fix it"))).unwrap();
        let text = std::fs::read_to_string(&p).unwrap();
        assert_eq!(text.lines().count(), 2);
        let v: Value = serde_json::from_str(text.lines().next().unwrap()).unwrap();
        for k in ["id", "ts", "app", "level", "model", "ms", "ok", "retried", "draft", "output", "error", "target", "project", "thumb"] {
            assert!(v.get(k).is_some(), "missing key {k}");
        }
        assert_eq!(v["model"], json!(["haiku"]));
        #[cfg(unix)]
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        let ids: Vec<_> = read_all(&p).iter().map(|v| v["id"].clone()).collect();
        assert_ne!(ids[0], ids[1]);
    }

    #[test]
    fn append_keeps_the_newest_rows_only() {
        let p = tmp("cap");
        for i in 0..7 {
            append_capped(&p, &row_json(&row(&format!("draft {i}"))), 5).unwrap();
        }
        let all = list(&p, "");
        assert_eq!(all.len(), 5);
        assert_eq!(all[0]["draft"], "draft 6");
        assert_eq!(all[4]["draft"], "draft 2");
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn rewrites_and_appends_at_once_lose_no_row() {
        let p = tmp("race");
        append_value(&p, &row_json(&row("first"))).unwrap();
        let id = read_all(&p)[0]["id"].as_str().unwrap().to_string();
        std::thread::scope(|s| {
            s.spawn(|| {
                for i in 0..40 {
                    append_value(&p, &row_json(&row(&format!("row {i}")))).unwrap();
                }
            });
            s.spawn(|| {
                for i in 0..40 {
                    thumb(&p, &id, Some(i % 2 == 0)).unwrap();
                }
            });
        });
        assert_eq!(read_all(&p).len(), 41);
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }

    #[test]
    fn old_runner_lines_get_ids_and_list_search_delete_thumb_work() {
        let p = tmp("ops");
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        // Two runner lines (no id), one bad line, one app line.
        std::fs::write(&p, concat!(
            r#"{"ts":"2026-11-22T10:00:00","ok":true,"app":"Ghostty","level":"standard","model":["haiku"],"ms":900,"draft":"fix login","output":"Fix the login."}"#, "\n",
            "not json\n",
            r#"{"ts":"2026-11-22T10:05:00","ok":false,"app":"Notes","level":"max","model":[],"ms":10,"draft":"email","output":null,"error":"x"}"#, "\n",
        )).unwrap();
        append_value(&p, &row_json(&row("moody sauna image"))).unwrap();
        let all = list(&p, "");
        assert_eq!(all.len(), 3);
        assert_eq!(all[0]["draft"], "moody sauna image", "newest first");
        assert_eq!(all[2]["id"], "2026-11-22T10:00:00-0");
        assert_eq!(all[2]["model"], "haiku");
        assert_eq!(all[1]["output"], "");
        assert_eq!(list(&p, "LOGIN").len(), 2); // draft "fix login" and the new row's output
        assert_eq!(list(&p, "ghostty").len(), 1);
        assert!(thumb(&p, "2026-11-22T10:00:00-0", Some(false)).unwrap());
        // Ids stay stable after a rewrite of the file (the bad line is dropped, ids were saved).
        assert_eq!(get(&p, "2026-11-22T10:00:00-0").unwrap()["thumb"], json!(false));
        delete(&p, Some("2026-11-22T10:00:00-0")).unwrap();
        assert_eq!(list(&p, "").len(), 2);
        #[cfg(unix)]
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        delete(&p, None).unwrap();
        assert!(list(&p, "").is_empty());
        let _ = std::fs::remove_dir_all(p.parent().unwrap());
    }
}
