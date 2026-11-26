//! `custom-api` (OpenAI-compatible) against a local mock HTTP server: checks the request, streams a canned reply.

use pmx_engines::{run_with, CustomApi, EngineId, Event, Level, Request, RunOptions};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

const FW: &str = "You are Promptmaxxer. Rewrite the draft.";

fn sse(parts: &[&str], finish: &str) -> String {
    let mut s = String::from("data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}]}\n\n");
    for p in parts {
        let d = serde_json::json!({"choices":[{"delta":{"content":p},"finish_reason":null}]});
        s.push_str(&format!("data: {d}\n\n"));
    }
    s.push_str(&format!("data: {{\"choices\":[{{\"delta\":{{}},\"finish_reason\":\"{finish}\"}}]}}\n\ndata: [DONE]\n\n"));
    s
}

#[derive(Debug, Clone)]
struct Seen {
    head: String,
    body: Value,
}

/// Serves one canned reply per connection, in order. Records each request.
async fn mock(replies: Vec<(u16, String, Duration)>) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen2 = seen.clone();
    tokio::spawn(async move {
        for (status, body, delay) in replies {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut buf = Vec::new();
            let mut tmp = [0u8; 8192];
            let (head, body_bytes) = loop {
                let n = sock.read(&mut tmp).await.unwrap();
                buf.extend_from_slice(&tmp[..n]);
                if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&buf[..i]).to_string();
                    let len: usize = head
                        .lines()
                        .find_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap()))
                        .unwrap_or(0);
                    while buf.len() < i + 4 + len {
                        let n = sock.read(&mut tmp).await.unwrap();
                        buf.extend_from_slice(&tmp[..n]);
                    }
                    break (head, buf[i + 4..i + 4 + len].to_vec());
                }
            };
            seen2.lock().unwrap().push(Seen { head, body: serde_json::from_slice(&body_bytes).unwrap() });
            let ctype = if status == 200 { "text/event-stream" } else { "application/json" };
            let resp = format!("HTTP/1.1 {status} X\r\ncontent-type: {ctype}\r\nconnection: close\r\n\r\n");
            sock.write_all(resp.as_bytes()).await.unwrap();
            // Stream in two halves with a pause, like a real SSE reply.
            let mid = body.len() / 2;
            let mid = (0..=mid).rev().find(|&i| body.is_char_boundary(i)).unwrap();
            let _ = sock.write_all(body[..mid].as_bytes()).await;
            tokio::time::sleep(delay).await;
            let _ = sock.write_all(body[mid..].as_bytes()).await;
            let _ = sock.shutdown().await;
        }
    });
    (format!("http://{addr}"), seen)
}

fn req(level: Level) -> Request {
    Request {
        draft: "the login page is broken fix it".into(),
        level,
        app_name: "Ghostty".into(),
        target: Some("coding agent: Claude Code".into()),
        context_brief: None,
        framework: FW.into(),
    }
}

fn opts(base: String, max_model: &str, key: Option<&str>) -> RunOptions {
    RunOptions {
        custom: Some(CustomApi { base_url: format!("{base}/v1"), fast_model: "qwen3:8b".into(), max_model: max_model.into() }),
        custom_key: key.map(str::to_string),
        ..Default::default()
    }
}

#[tokio::test]
async fn streams_and_sends_the_right_request() {
    let good = sse(&["The login page ", "is broken. Find the cause, fix it, ", "and tell me what you changed."], "stop");
    let (base, seen) = mock(vec![(200, good, Duration::from_millis(30))]).await;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let out = run_with(EngineId::CustomApi, req(Level::Standard), tx, CancellationToken::new(), opts(base, "", Some("sk-test-not-real"))).await.unwrap();
    assert_eq!(out.text, "The login page is broken. Find the cause, fix it, and tell me what you changed.");
    assert_eq!(out.model, "qwen3:8b");

    let s = seen.lock().unwrap()[0].clone();
    let head = s.head.to_ascii_lowercase();
    assert!(head.starts_with("post /v1/chat/completions "), "{head}");
    assert!(head.contains("authorization: bearer sk-test-not-real"));
    assert_eq!(s.body["model"], "qwen3:8b");
    assert_eq!(s.body["messages"][0]["content"], FW);
    assert!(s.body["messages"][1]["content"].as_str().unwrap().contains("<draft>"));

    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }
    assert!(matches!(&events[0], Event::Started { engine: EngineId::CustomApi, model } if model == "qwen3:8b"));
    assert!(matches!(events.last().unwrap(), Event::Done(o) if o.text == out.text));
}

#[tokio::test]
async fn no_key_sends_no_auth_and_max_uses_max_model() {
    let (base, seen) = mock(vec![(200, sse(&["<think>plan</think>\n\nAudit the login flow and fix the root cause."], "stop"), Duration::ZERO)]).await;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let out = run_with(EngineId::CustomApi, req(Level::Max), tx, CancellationToken::new(), opts(base, "qwen3:32b", None)).await.unwrap();
    assert_eq!(out.model, "qwen3:32b");
    assert_eq!(out.text, "Audit the login flow and fix the root cause.");
    let s = seen.lock().unwrap()[0].clone();
    assert!(!s.head.to_ascii_lowercase().contains("authorization:"));
    assert_eq!(s.body["model"], "qwen3:32b");
}

#[tokio::test]
async fn http_error_and_cut_off_fail() {
    let err = r#"{"error":{"message":"model 'nope' not found"}}"#.to_string();
    let (base, _) = mock(vec![(404, err, Duration::ZERO)]).await;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let e = run_with(EngineId::CustomApi, req(Level::Standard), tx, CancellationToken::new(), opts(base, "", None)).await.unwrap_err();
    assert_eq!(e.to_string(), "HTTP 404: model 'nope' not found");

    let (base, _) = mock(vec![(200, sse(&["Half a prompt"], "length"), Duration::ZERO)]).await;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let e = run_with(EngineId::CustomApi, req(Level::Standard), tx, CancellationToken::new(), opts(base, "", None)).await.unwrap_err();
    assert!(e.to_string().contains("cut off"), "{e}");
}

#[tokio::test]
async fn not_set_up_is_not_ready() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let o = RunOptions { custom: Some(CustomApi::default()), ..Default::default() };
    let e = run_with(EngineId::CustomApi, req(Level::Standard), tx, CancellationToken::new(), o).await.unwrap_err();
    assert!(e.to_string().contains("Settings"), "{e}");
}
