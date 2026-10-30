//! A scripted fake model server speaking the OpenAI chat-completions protocol on localhost.
//!
//! Tests and `aura-cli --mock` point Aura at it, so the real HTTP path, schema handling, retry
//! feedback and the whole agent loop can be exercised without a real model. Replies are queued
//! per request kind (the schema name Aura sends: `aura_proposal`, `aura_verification`,
//! `aura_done_check`, `aura_final_outcome`); when a queue runs dry its last reply repeats.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Default)]
struct Shared {
    queues: HashMap<String, VecDeque<String>>,
    last: HashMap<String, String>,
    /// Every request body received, in order.
    requests: Vec<String>,
}

pub struct MockServer {
    /// Base URL to give Aura, for example `http://127.0.0.1:50321/v1`.
    pub base_url: String,
    shared: Arc<Mutex<Shared>>,
    task: tokio::task::JoinHandle<()>,
}

impl MockServer {
    /// Starts a server with scripted replies: `(kind, reply_text)` pairs, consumed in order per kind.
    pub async fn start(script: Vec<(&str, String)>) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let address = listener.local_addr()?;
        let mut shared = Shared::default();
        for (kind, reply) in script {
            shared.queues.entry(kind.to_owned()).or_default().push_back(reply);
        }
        let shared = Arc::new(Mutex::new(shared));
        let handler_state = Arc::clone(&shared);
        let task = tokio::spawn(async move {
            loop {
                let Ok((stream, _)) = listener.accept().await else { break };
                let state = Arc::clone(&handler_state);
                tokio::spawn(async move {
                    let _ = handle_connection(stream, state).await;
                });
            }
        });
        Ok(Self { base_url: format!("http://{address}/v1"), shared, task })
    }

    /// All request bodies received so far.
    pub fn requests(&self) -> Vec<String> {
        self.shared.lock().map(|s| s.requests.clone()).unwrap_or_default()
    }

    /// Request bodies whose schema name is `kind`.
    pub fn requests_of(&self, kind: &str) -> Vec<String> {
        self.requests().into_iter().filter(|body| body.contains(&format!("\"name\":\"{kind}\""))).collect()
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn handle_connection(mut stream: TcpStream, shared: Arc<Mutex<Shared>>) -> std::io::Result<()> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    // Read until the headers are complete, then until the declared body length is in.
    let (header_end, content_length) = loop {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            return Ok(());
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(position) = find_subsequence(&buffer, b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&buffer[..position]).to_lowercase();
            let length = headers
                .lines()
                .find_map(|line| line.strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0)))
                .unwrap_or(0);
            break (position + 4, length);
        }
    };
    while buffer.len() < header_end + content_length {
        let read = stream.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let head = String::from_utf8_lossy(&buffer[..header_end]).to_string();
    let body = String::from_utf8_lossy(&buffer[header_end..]).to_string();

    let response_body = if head.starts_with("GET") {
        r#"{"object":"list","data":[{"id":"mock-model","object":"model"}]}"#.to_owned()
    } else {
        let compact: String = serde_json::from_str::<serde_json::Value>(&body)
            .map(|value| value.to_string())
            .unwrap_or_else(|_| body.clone());
        let kind = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|value| {
                value
                    .pointer("/response_format/json_schema/name")
                    .and_then(|name| name.as_str().map(str::to_owned))
            })
            .unwrap_or_else(|| "unknown".to_owned());
        let reply = {
            let mut state = shared.lock().unwrap_or_else(|e| e.into_inner());
            state.requests.push(compact);
            let next = state.queues.get_mut(&kind).and_then(VecDeque::pop_front);
            match next {
                Some(reply) => {
                    state.last.insert(kind.clone(), reply.clone());
                    reply
                }
                None => state.last.get(&kind).cloned().unwrap_or_else(|| "{}".to_owned()),
            }
        };
        serde_json::json!({"choices": [{"index": 0, "message": {"role": "assistant", "content": reply}, "finish_reason": "stop"}]})
            .to_string()
    };
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        response_body.len(),
        response_body
    );
    stream.write_all(response.as_bytes()).await?;
    stream.shutdown().await
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

/// Builders for scripted replies, so tests read naturally.
pub mod replies {
    use serde_json::{json, Value};

    /// A proposal reply carrying `action` (any JSON action object).
    pub fn proposal(action: Value, thought: &str) -> String {
        proposal_with_impact(action, thought, "routine")
    }

    pub fn proposal_with_impact(action: Value, thought: &str, impact: &str) -> String {
        json!({
            "previousOutcome": "not_applicable",
            "reflection": "scripted",
            "thought": thought,
            "target": {"description": "scripted target"},
            "expectedOutcome": "scripted outcome",
            "action": action,
            "confidence": 0.9,
            "impact": impact
        })
        .to_string()
    }

    pub fn accept_verification() -> String {
        json!({"verdict": "accept", "reason": "scripted accept"}).to_string()
    }

    pub fn done_check(complete: bool, missing: &str) -> String {
        json!({"evidence": "scripted", "complete": complete, "missing": missing}).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn serves_scripted_replies_per_kind_and_repeats_the_last() {
        let server = MockServer::start(vec![
            ("aura_proposal", "first".into()),
            ("aura_proposal", "second".into()),
            ("aura_done_check", "check".into()),
        ])
        .await
        .unwrap();
        let client = reqwest::Client::new();
        let ask = |kind: &'static str| {
            let client = client.clone();
            let url = format!("{}/chat/completions", server.base_url);
            async move {
                let body = serde_json::json!({"response_format": {"type": "json_schema", "json_schema": {"name": kind}}});
                let value: serde_json::Value = client.post(url).json(&body).send().await.unwrap().json().await.unwrap();
                value["choices"][0]["message"]["content"].as_str().unwrap().to_owned()
            }
        };
        assert_eq!(ask("aura_proposal").await, "first");
        assert_eq!(ask("aura_proposal").await, "second");
        assert_eq!(ask("aura_proposal").await, "second", "an exhausted queue repeats its last reply");
        assert_eq!(ask("aura_done_check").await, "check");
        assert_eq!(server.requests_of("aura_proposal").len(), 3);
        let models: serde_json::Value = client.get(format!("{}/models", server.base_url)).send().await.unwrap().json().await.unwrap();
        assert!(models["data"].is_array());
    }
}
