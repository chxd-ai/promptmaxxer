//! `custom-api`: any service that speaks the OpenAI Chat Completions API, with streaming.
//! Works with OpenAI, OpenRouter, Groq, DeepSeek, Mistral, Together, Gemini's OpenAI endpoint,
//! and local servers (Ollama, LM Studio, llama.cpp, vLLM). The key is optional: local servers need none.

use crate::{EngineError, Event};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

/// No `max_tokens`: newer OpenAI models refuse it (they want `max_completion_tokens`), and other
/// servers refuse that one. A reply cut at the server's own limit still fails on `finish_reason`.
pub(crate) fn body(model: &str, system: &str, user: &str) -> Value {
    json!({
        "model": model,
        "stream": true,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user },
        ],
    })
}

/// Incremental SSE parser for chat completion chunks.
#[derive(Default)]
pub(crate) struct Sse {
    buf: String,
    pub text: String,
    pub error: Option<String>,
    /// `[DONE]` seen, or a chunk with a finish reason.
    pub stopped: bool,
    pub finish_reason: Option<String>,
}

impl Sse {
    pub fn feed(&mut self, chunk: &str) -> Vec<String> {
        self.buf.push_str(&chunk.replace("\r\n", "\n"));
        let mut out = Vec::new();
        while let Some(end) = self.buf.find('\n') {
            let line: String = self.buf.drain(..end + 1).collect();
            let Some(data) = line.trim_end().strip_prefix("data:") else { continue };
            let data = data.trim();
            if data == "[DONE]" {
                self.stopped = true;
                continue;
            }
            let Ok(v) = serde_json::from_str::<Value>(data) else { continue };
            if let Some(e) = v.get("error") {
                self.error = Some(e["message"].as_str().or(e.as_str()).unwrap_or("API error").to_string());
                continue;
            }
            let c = &v["choices"][0];
            if let Some(t) = c["delta"]["content"].as_str() {
                if !t.is_empty() {
                    self.text.push_str(t);
                    out.push(t.to_string());
                }
            }
            if let Some(r) = c["finish_reason"].as_str() {
                self.finish_reason = Some(r.to_string());
                self.stopped = true;
            }
        }
        out
    }
}

/// Local reasoning models (DeepSeek R1, Qwen) put their thinking in `<think>…</think>` before the
/// answer. Only the answer is the prompt.
pub(crate) fn drop_thinking(text: &str) -> String {
    let t = text.trim_start();
    if let Some(rest) = t.strip_prefix("<think>") {
        if let Some(i) = rest.find("</think>") {
            return rest[i + "</think>".len()..].trim().to_string();
        }
    }
    text.to_string()
}

#[allow(clippy::too_many_arguments)]
pub(crate) async fn call(
    endpoint: &str,
    key: Option<&str>,
    model: &str,
    system: &str,
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
        let mut rb = client
            .post(endpoint)
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .body(body(model, system, user).to_string());
        if let Some(k) = key {
            rb = rb.bearer_auth(k);
        }
        let resp = rb.send().await.map_err(|e| EngineError::Http(e.without_url().to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            let msg = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| v["error"]["message"].as_str().or(v["error"].as_str()).map(str::to_string))
                .unwrap_or_else(|| text.chars().take(200).collect());
            return Err(EngineError::Http(format!("HTTP {}: {msg}", status.as_u16())));
        }
        let mut sse = Sse::default();
        let mut stream = resp.bytes_stream();
        let mut pending: Vec<u8> = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| EngineError::Http(e.without_url().to_string()))?;
            pending.extend_from_slice(&chunk);
            let valid = match std::str::from_utf8(&pending) {
                Ok(_) => pending.len(),
                Err(e) if e.error_len().is_some() => pending.len(),
                Err(e) => e.valid_up_to(),
            };
            let s = String::from_utf8_lossy(&pending[..valid]).into_owned();
            pending.drain(..valid);
            for d in sse.feed(&s) {
                let _ = tx.send(Event::Delta(d));
            }
            if sse.error.is_some() {
                break;
            }
        }
        // A last line without a newline.
        for d in sse.feed("\n") {
            let _ = tx.send(Event::Delta(d));
        }
        if let Some(e) = sse.error {
            return Err(EngineError::Http(e));
        }
        if sse.finish_reason.as_deref() == Some("length") {
            return Err(EngineError::Http("The answer hit the length limit and was cut off".into()));
        }
        if !sse.stopped {
            return Err(EngineError::Http("The API stream ended early".into()));
        }
        let text = drop_thinking(&sse.text);
        if text.trim().is_empty() {
            return Err(EngineError::Http("The API returned no text".into()));
        }
        Ok(text)
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
    fn body_has_system_and_user() {
        let b = body("gpt-x", "FW", "USER");
        assert_eq!(b["messages"][0]["role"], "system");
        assert_eq!(b["messages"][0]["content"], "FW");
        assert_eq!(b["messages"][1]["content"], "USER");
        assert_eq!(b["stream"], true);
        assert!(b.get("max_tokens").is_none());
    }

    #[test]
    fn sse_split_across_chunks() {
        let mut s = Sse::default();
        let mut d = s.feed("data: {\"choices\":[{\"delta\":{\"role\":\"assistant\",\"content\":\"\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"cont");
        d.extend(s.feed("ent\":\"Hi \"}}]}\r\n\r\n: keep-alive\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"there\"},\"finish_reason\":null}]}\n\n"));
        assert!(!s.stopped);
        d.extend(s.feed("data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n"));
        assert_eq!(d, vec!["Hi ", "there"]);
        assert!(s.stopped);
        assert_eq!(s.text, "Hi there");
        assert_eq!(s.finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn sse_error_and_length() {
        let mut s = Sse::default();
        s.feed("data: {\"error\":{\"message\":\"Rate limited\"}}\n\n");
        assert_eq!(s.error.as_deref(), Some("Rate limited"));
        let mut s = Sse::default();
        s.feed("data: {\"choices\":[{\"delta\":{\"content\":\"half\"},\"finish_reason\":\"length\"}]}\n\n");
        assert_eq!(s.finish_reason.as_deref(), Some("length"));
    }

    #[test]
    fn thinking_is_dropped() {
        assert_eq!(drop_thinking("<think>\nhmm\n</think>\n\nThe prompt"), "The prompt");
        assert_eq!(drop_thinking("No thinking here"), "No thinking here");
        assert_eq!(drop_thinking("<think>never closed"), "<think>never closed");
    }
}
