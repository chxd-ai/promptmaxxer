//! `anthropic-key` against a local mock HTTP server: checks the request, streams a canned SSE reply.

use pmx_engines::{run_with, EngineError, EngineId, Event, Level, Request, RunOptions};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

const FW: &str = "You are Promptmaxxer. Rewrite the draft.";

#[derive(Debug, Clone)]
struct Seen {
    head: String,
    body: Value,
}

fn sse(text_parts: &[&str]) -> String {
    let mut s = String::from(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_1\",\"usage\":{\"input_tokens\":10,\"cache_creation_input_tokens\":5000}}}\n\n\
         event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n\
         event: ping\ndata: {\"type\": \"ping\"}\n\n",
    );
    for p in text_parts {
        let d = serde_json::json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":p}});
        s.push_str(&format!("event: content_block_delta\ndata: {d}\n\n"));
    }
    s.push_str(
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n\
         event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":20}}\n\n\
         event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
    );
    s
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

fn opts(base: String) -> RunOptions {
    RunOptions { anthropic_base_url: Some(base), anthropic_key: Some("sk-ant-test-not-real".into()), ..Default::default() }
}

#[tokio::test]
async fn streams_and_sends_the_right_request() {
    let good = sse(&["The login page ", "is broken. Find the cause, fix it, ", "and tell me what you changed."]);
    let (base, seen) = mock(vec![(200, good, Duration::from_millis(30))]).await;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let out = run_with(EngineId::AnthropicKey, req(Level::Standard), tx, CancellationToken::new(), opts(base)).await.unwrap();
    assert_eq!(out.text, "The login page is broken. Find the cause, fix it, and tell me what you changed.");
    assert_eq!(out.model, "claude-haiku-5-5");
    assert!(!out.retried);

    let s = seen.lock().unwrap()[0].clone();
    let head = s.head.to_ascii_lowercase();
    assert!(head.starts_with("post /v1/messages "), "{head}");
    assert!(head.contains("x-api-key: sk-ant-test-not-real"));
    assert!(head.contains("anthropic-version: 2023-06-01"));
    assert_eq!(s.body["model"], "claude-haiku-5-5");
    assert_eq!(s.body["stream"], true);
    assert_eq!(s.body["system"][0]["text"], FW);
    assert_eq!(s.body["system"][0]["cache_control"]["type"], "ephemeral");
    let user = s.body["messages"][0]["content"].as_str().unwrap();
    assert!(user.starts_with("<context>App: Ghostty. Level: standard.</context>\n<target>coding agent: Claude Code</target>\n<draft>\n"));

    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }
    assert!(matches!(&events[0], Event::Started { engine: EngineId::AnthropicKey, model } if model == "claude-haiku-5-5"));
    let deltas: String = events.iter().filter_map(|e| if let Event::Delta(d) = e { Some(d.as_str()) } else { None }).collect();
    assert_eq!(deltas, out.text);
    assert!(events.iter().filter(|e| matches!(e, Event::Delta(_))).count() >= 2);
    assert!(matches!(events.last().unwrap(), Event::Done(o) if o.text == out.text));
}

#[tokio::test]
async fn max_level_uses_sonnet() {
    let (base, seen) = mock(vec![(200, sse(&["Audit the login flow end to end and fix the root cause."]), Duration::ZERO)]).await;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let out = run_with(EngineId::AnthropicKey, req(Level::Max), tx, CancellationToken::new(), opts(base)).await.unwrap();
    assert_eq!(out.model, "claude-sonnet-5-5");
    assert_eq!(seen.lock().unwrap()[0].body["max_tokens"], 8192);
}

#[tokio::test]
async fn refusal_gets_one_firm_retry() {
    let bad = sse(&["I'm sorry, but I can't rewrite this draft without more detail."]);
    let good = sse(&["The login page is broken. Find the cause and fix it."]);
    let (base, seen) = mock(vec![(200, bad, Duration::ZERO), (200, good, Duration::ZERO)]).await;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let out = run_with(EngineId::AnthropicKey, req(Level::Standard), tx, CancellationToken::new(), opts(base)).await.unwrap();
    assert!(out.retried);
    assert_eq!(out.text, "The login page is broken. Find the cause and fix it.");
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert!(!seen[0].body["messages"][0]["content"].as_str().unwrap().contains("Your previous reply was a comment"));
    assert!(seen[1].body["messages"][0]["content"].as_str().unwrap().contains("Your previous reply was a comment"));
    let mut starts = 0;
    while let Ok(e) = rx.try_recv() {
        starts += matches!(e, Event::Started { .. }) as usize;
    }
    assert_eq!(starts, 2, "a second Started marks the retry");
}

#[tokio::test]
async fn two_bad_replies_fail_safe() {
    let bad = sse(&["Here is the improved prompt: I need the draft first."]);
    let (base, _) = mock(vec![(200, bad.clone(), Duration::ZERO), (200, bad, Duration::ZERO)]).await;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let err = run_with(EngineId::AnthropicKey, req(Level::Standard), tx, CancellationToken::new(), opts(base)).await.unwrap_err();
    assert!(matches!(err, EngineError::BadOutput(_)));
    let mut last = None;
    while let Ok(e) = rx.try_recv() {
        last = Some(e);
    }
    assert!(matches!(last, Some(Event::Failed(_))));
}

#[tokio::test]
async fn http_error_is_reported_without_the_key() {
    let body = r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#.to_string();
    let (base, _) = mock(vec![(401, body, Duration::ZERO)]).await;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let err = run_with(EngineId::AnthropicKey, req(Level::Standard), tx, CancellationToken::new(), opts(base)).await.unwrap_err();
    let msg = err.to_string();
    assert_eq!(msg, "HTTP 401: invalid x-api-key");
    assert!(!msg.contains("sk-ant"));
}

#[tokio::test]
async fn cancel_stops_a_slow_stream() {
    let (base, _) = mock(vec![(200, sse(&["slow part one ", "slow part two that never comes"]), Duration::from_secs(30))]).await;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        c2.cancel();
    });
    let t = std::time::Instant::now();
    let err = run_with(EngineId::AnthropicKey, req(Level::Standard), tx, cancel, opts(base)).await.unwrap_err();
    assert_eq!(err, EngineError::Cancelled);
    assert!(t.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn timeout_applies() {
    let (base, _) = mock(vec![(200, sse(&["slow"]), Duration::from_secs(30))]).await;
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut o = opts(base);
    o.timeout = Some(Duration::from_millis(400));
    let err = run_with(EngineId::AnthropicKey, req(Level::Standard), tx, CancellationToken::new(), o).await.unwrap_err();
    assert!(matches!(err, EngineError::Timeout(_)));
}

#[tokio::test]
async fn empty_draft_never_calls_the_api() {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut r = req(Level::Standard);
    r.draft = "  \n ".into();
    let err = run_with(EngineId::AnthropicKey, r, tx, CancellationToken::new(), opts("http://127.0.0.1:9".into())).await.unwrap_err();
    assert_eq!(err, EngineError::EmptyDraft);
}
