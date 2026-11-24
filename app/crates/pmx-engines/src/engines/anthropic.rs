//! `anthropic-key`: Messages API with an API key from the macOS Keychain. SSE streaming, and prompt
//! caching on the system block (the framework is the same on every call).

use crate::{EngineError, Event, Level};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

pub(crate) const DEFAULT_BASE_URL: &str = "https://api.anthropic.com";
pub(crate) const API_VERSION: &str = "2023-06-01";

pub(crate) fn max_tokens(level: Level) -> u32 {
    match level {
        Level::Standard => 4096,
        Level::Max => 8192,
    }
}

pub(crate) fn body(model: &str, level: Level, framework: &str, user: &str) -> Value {
    json!({
        "model": model,
        "max_tokens": max_tokens(level),
        "stream": true,
        "system": [{ "type": "text", "text": framework, "cache_control": { "type": "ephemeral" } }],
        "messages": [{ "role": "user", "content": user }],
    })
}

/// Incremental SSE parser: feed raw bytes, get text deltas; tracks stop and errors.
#[derive(Default)]
pub(crate) struct Sse {
    buf: String,
    pub text: String,
    pub error: Option<String>,
    pub stopped: bool,
    pub stop_reason: Option<String>,
}

impl Sse {
    pub fn feed(&mut self, chunk: &str) -> Vec<String> {
        self.buf.push_str(&chunk.replace("\r\n", "\n"));
        let mut out = Vec::new();
        while let Some(end) = self.buf.find("\n\n") {
            let block: String = self.buf.drain(..end + 2).collect();
            let data: String = block
                .lines()
                .filter_map(|l| l.strip_prefix("data:"))
                .map(|d| d.strip_prefix(' ').unwrap_or(d))
                .collect::<Vec<_>>()
                .join("\n");
            let Ok(v) = serde_json::from_str::<Value>(&data) else { continue };
            match v["type"].as_str() {
                Some("content_block_delta") if v["delta"]["type"] == "text_delta" => {
                    let t = v["delta"]["text"].as_str().unwrap_or("").to_string();
                    self.text.push_str(&t);
                    out.push(t);
                }
                Some("message_delta") => {
                    if let Some(r) = v["delta"]["stop_reason"].as_str() {
                        self.stop_reason = Some(r.to_string());
                    }
                }
                Some("message_stop") => self.stopped = true,
                Some("error") => {
                    self.error = Some(v["error"]["message"].as_str().unwrap_or("Anthropic API error").to_string())
                }
                _ => {}
            }
        }
        out
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn call(
    base_url: &str,
    key: &str,
    model: &str,
    level: Level,
    framework: &str,
    user: &str,
    tx: &UnboundedSender<Event>,
    cancel: &CancellationToken,
    timeout: Duration,
) -> Result<String, EngineError> {
    let work = async {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| EngineError::Http(e.to_string()))?;
        let resp = client
            .post(format!("{}/v1/messages", base_url.trim_end_matches('/')))
            .header("x-api-key", key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .body(body(model, level, framework, user).to_string())
            .send()
            .await
            .map_err(|e| EngineError::Http(e.without_url().to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            let msg = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| v["error"]["message"].as_str().map(str::to_string))
                .unwrap_or_else(|| text.chars().take(200).collect());
            return Err(EngineError::Http(format!("HTTP {}: {msg}", status.as_u16())));
        }
        let mut sse = Sse::default();
        let mut stream = resp.bytes_stream();
        let mut pending: Vec<u8> = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| EngineError::Http(e.without_url().to_string()))?;
            pending.extend_from_slice(&chunk);
            // Only hand complete UTF-8 to the parser.
            // A cut-off character at the end waits for the next chunk; a truly invalid byte
            // (error_len is Some) is replaced, so it cannot block every later chunk.
            let valid = match std::str::from_utf8(&pending) {
                Ok(_) => pending.len(),
                Err(e) if e.error_len().is_some() => pending.len(),
                Err(e) => e.valid_up_to(),
            };
            let s = String::from_utf8_lossy(&pending[..valid]).into_owned();
            pending.drain(..valid);
            for d in sse.feed(&s) {
                if !d.is_empty() {
                    let _ = tx.send(Event::Delta(d));
                }
            }
            if sse.stopped {
                break;
            }
        }
        if let Some(e) = sse.error {
            return Err(EngineError::Http(e));
        }
        if sse.text.trim().is_empty() {
            return Err(EngineError::Http("Anthropic API returned no text".into()));
        }
        // A stream that ends early, or a reply cut at the token limit, is half a prompt: never paste it.
        if !sse.stopped {
            return Err(EngineError::Http("Anthropic API stream ended early".into()));
        }
        if sse.stop_reason.as_deref() == Some("max_tokens") {
            return Err(EngineError::Http("The answer hit the length limit and was cut off".into()));
        }
        Ok(sse.text)
    };
    tokio::select! {
        r = work => r,
        _ = cancel.cancelled() => Err(EngineError::Cancelled),
        _ = tokio::time::sleep(timeout) => Err(EngineError::Timeout(timeout.as_secs())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_caches_system_block() {
        let b = body("claude-haiku-5-5", Level::Standard, "FW", "USER");
        assert_eq!(b["system"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(b["system"][0]["text"], "FW");
        assert_eq!(b["stream"], true);
        assert_eq!(b["messages"][0]["content"], "USER");
        assert_eq!(b["max_tokens"], 4096);
    }

    #[test]
    fn sse_split_across_chunks() {
        let mut s = Sse::default();
        let mut d = s.feed("event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_de");
        assert!(d.is_empty());
        d.extend(s.feed("lta\",\"text\":\"Hi \"}}\n\nevent: content_block_delta\r\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"there\"}}\r\n\r\n"));
        d.extend(s.feed("event: ping\ndata: {\"type\":\"ping\"}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"));
        assert_eq!(d, vec!["Hi ", "there"]);
        assert!(s.stopped);
        assert_eq!(s.text, "Hi there");
    }

    #[test]
    fn sse_error_event() {
        let mut s = Sse::default();
        s.feed("event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n");
        assert_eq!(s.error.as_deref(), Some("Overloaded"));
    }
}
