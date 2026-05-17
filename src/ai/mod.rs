use crate::prompt;
use chrono::Utc;
use serde_json::json;
use std::{
    io::{BufRead, BufReader},
    sync::mpsc::{self, Receiver},
    time::Duration,
};

#[derive(Debug, Clone)]
pub struct ChatTurn {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct AiModel {
    pub id: String,
    pub provider: String,
    pub label: String,
}

#[derive(Debug, Clone)]
pub enum StreamEvent {
    Delta(String),
    Done,
    Error(String),
}

pub fn list_models() -> Vec<AiModel> {
    match provider().as_str() {
        "vultr" => vultr_models().unwrap_or_else(|_| fallback_models()),
        _ => fallback_models(),
    }
}

pub fn default_model() -> String {
    std::env::var("VULTR_DEFAULT_MODEL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| list_models().first().map(|model| model.id.clone()))
        .unwrap_or_else(|| "llama-3.1-8b-instruct".into())
}

pub fn stream_chat(model: String, messages: Vec<ChatTurn>) -> Receiver<StreamEvent> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = retry_ai_call("stream_chat", || match provider().as_str() {
            "vultr" => stream_vultr(model.clone(), with_system_prompt(messages.clone()), tx.clone()),
            other => Err(format!("unsupported AI provider: {other}")),
        });

        if let Err(error) = result {
            let _ = tx.send(StreamEvent::Error(error));
        }
        let _ = tx.send(StreamEvent::Done);
    });
    rx
}

pub fn complete_chat_with_system_prompt(
    model: String,
    system_prompt: &str,
    messages: Vec<ChatTurn>,
) -> Result<String, String> {
    retry_ai_call("complete_chat", || match provider().as_str() {
        "vultr" => complete_vultr(model.clone(), with_custom_system_prompt(system_prompt, messages.clone())),
        other => Err(format!("unsupported AI provider: {other}")),
    })
}

fn with_system_prompt(messages: Vec<ChatTurn>) -> Vec<ChatTurn> {
    with_custom_system_prompt(prompt::SYSTEM_PROMPT, messages)
}

fn with_custom_system_prompt(system_prompt: &str, messages: Vec<ChatTurn>) -> Vec<ChatTurn> {
    let mut turns = Vec::with_capacity(messages.len() + 1);
    if !messages.iter().any(|message| message.role == "system") {
        turns.push(ChatTurn {
            role: "system".into(),
            content: system_prompt.into(),
        });
    }
    turns.extend(messages);
    turns
}

fn provider() -> String {
    std::env::var("AI_PROVIDER")
        .unwrap_or_else(|_| "vultr".into())
        .to_ascii_lowercase()
}

fn vultr_base_url() -> String {
    std::env::var("VULTR_INFERENCE_BASE_URL")
        .unwrap_or_else(|_| "https://api.vultrinference.com/v1".into())
        .trim_end_matches('/')
        .to_string()
}

fn vultr_key() -> Result<String, String> {
    std::env::var("VULTR_INFERENCE_API_KEY")
        .or_else(|_| std::env::var("VULTR_SERVERLESS_INFERENCE_API_KEY"))
        .map_err(|_| "missing VULTR_INFERENCE_API_KEY".to_string())
        .and_then(|key| {
            if key.trim().is_empty() {
                Err("missing VULTR_INFERENCE_API_KEY".into())
            } else {
                Ok(key)
            }
        })
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_read(Duration::from_secs(180))
        .timeout_write(Duration::from_secs(30))
        .build()
}

fn retry_ai_call<T, F>(label: &str, mut f: F) -> Result<T, String>
where
    F: FnMut() -> Result<T, String>,
{
    let mut last_error = None;
    for attempt in 1..=3 {
        match f() {
            Ok(value) => return Ok(value),
            Err(error) => {
                let should_retry = is_retryable_ai_error(&error) && attempt < 3;
                eprintln!("[ai/retry] {label} attempt={attempt} retry={} error={error}", should_retry);
                last_error = Some(error);
                if should_retry {
                    std::thread::sleep(Duration::from_millis(250 * attempt as u64));
                    continue;
                }
                break;
            }
        }
    }
    Err(last_error.unwrap_or_else(|| format!("{label} failed")))
}

fn is_retryable_ai_error(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("connection reset by peer")
        || lower.contains("tls connection init failed")
        || lower.contains("connection failed")
        || lower.contains("broken pipe")
        || lower.contains("timed out")
        || lower.contains("deadline")
        || lower.contains("transport")
}

fn vultr_models() -> Result<Vec<AiModel>, String> {
    let response = agent()
        .get(&format!("{}/models", vultr_base_url()))
        .set("Authorization", &format!("Bearer {}", vultr_key()?))
        .call()
        .map_err(|e| e.to_string())?;

    let value: serde_json::Value = response.into_json().map_err(|e| e.to_string())?;
    let models = value
        .get("data")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|model| model.get("id").and_then(|id| id.as_str()))
        .map(|id| AiModel {
            id: id.to_string(),
            provider: "vultr".into(),
            label: id.rsplit('/').next().unwrap_or(id).replace('-', " "),
        })
        .collect::<Vec<_>>();

    if models.is_empty() {
        Err("Vultr returned no models".into())
    } else {
        Ok(models)
    }
}

fn fallback_models() -> Vec<AiModel> {
    std::env::var("VULTR_DEFAULT_MODEL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(|id| vec![model("vultr", &id)])
        .unwrap_or_else(|| {
            vec![
                model("vultr", "llama-3.1-8b-instruct"),
                model("vultr", "mistral-7b-instruct"),
                model("vultr", "qwen2.5-7b-instruct"),
            ]
        })
}

fn model(provider: &str, id: &str) -> AiModel {
    AiModel {
        id: id.into(),
        provider: provider.into(),
        label: id.rsplit('/').next().unwrap_or(id).replace('-', " "),
    }
}

fn stream_vultr(
    model: String,
    messages: Vec<ChatTurn>,
    tx: mpsc::Sender<StreamEvent>,
) -> Result<(), String> {
    // max_tokens raised from 512 → 4096. Many recent models (especially
    // thinking / reasoning variants like GLM 5.1) burn 100–500 tokens on
    // internal reasoning before emitting any visible content, so 512 caps
    // them mid-thought and we end up with an empty `content` stream.
    let payload = json!({
        "model": model,
        "stream": true,
        "max_tokens": max_completion_tokens(),
        "messages": messages
            .into_iter()
            .map(|message| json!({ "role": message.role, "content": message.content }))
            .collect::<Vec<_>>()
    });

    let response = agent()
        .post(&format!("{}/chat/completions", vultr_base_url()))
        .set("Authorization", &format!("Bearer {}", vultr_key()?))
        .set("Content-Type", "application/json")
        .set("Accept", "text/event-stream")        // explicit: ask for SSE back
        .send_json(payload.clone())
        .map_err(|e| {
            eprintln!("[ai/vultr] HTTP error from upstream: {e}");
            e.to_string()
        })?;

    let status = response.status();
    let content_type = response.content_type().to_string();

    // Non-streaming response — provider returned a single JSON blob despite
    // `stream: true`. Parse it and emit as a single delta so the chat doesn't
    // look broken. Happens for some Vultr models that don't honour the flag.
    if !content_type.starts_with("text/event-stream") {
        let body = response.into_string().map_err(|e| e.to_string())?;
        eprintln!(
            "[ai/vultr] non-stream response (status={status}, content-type={content_type})\n\
             body preview: {}",
            body.chars().take(400).collect::<String>()
        );

        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&body) {
            if let Some(content) = extract_full_content(&value) {
                if !content.is_empty() {
                    debug_upstream_event("delta", &json!({ "text": content }));
                    let _ = tx.send(StreamEvent::Delta(content));
                    return Ok(());
                }
            }
            if let Some(error) = value
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
            {
                return Err(format!("Vultr error: {error}"));
            }
        }
        return Err(format!(
            "Vultr returned no content (status={status}, model={model})"
        ));
    }

    let mut reader = BufReader::new(response.into_reader());
    let mut line = String::new();
    let mut sent_any = false;
    let mut diag_buffer: Vec<String> = Vec::new();
    let mut stripper = ThinkStripper::default();

    loop {
        line.clear();
        let read = reader.read_line(&mut line).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }

        let trimmed = line.trim();

        // Empty line is the SSE event boundary — skip silently.
        if trimmed.is_empty() {
            continue;
        }

        let Some(data) = trimmed.strip_prefix("data:") else {
            // Anything else (event:, id:, retry:, etc.) — collect for diagnostics
            // but don't spam unless we end up with no content.
            if diag_buffer.len() < 8 {
                diag_buffer.push(trimmed.to_string());
            }
            continue;
        };
        let data = data.trim();
        if data == "[DONE]" {
            break;
        }

        let value: serde_json::Value = match serde_json::from_str(data) {
            Ok(value) => value,
            Err(err) => {
                eprintln!("[ai/vultr] failed to parse data chunk: {err} :: {data}");
                continue;
            }
        };

        // ── Try several places where the actual text might live ──────────
        //   1. choices[0].delta.content     (OpenAI SSE standard)
        //   2. choices[0].delta.reasoning_content (DeepSeek / GLM thinking)
        //   3. choices[0].message.content   (some providers when stream=true is ignored)
        //   4. choices[0].text              (legacy completions API)
        let choice = value.get("choices").and_then(|c| c.get(0));
        let delta = choice.and_then(|c| c.get("delta"));

        let mut emitted = false;
        // Suppress the dedicated reasoning_content channel entirely — it's pure thinking.
        let _ = delta.and_then(|d| d.get("reasoning_content"));

        if let Some(text) = delta
            .and_then(|d| d.get("content"))
            .and_then(|c| c.as_str())
        {
            let visible = stripper.process(text);
            if !visible.is_empty() {
                debug_upstream_event("delta", &json!({ "text": &visible }));
                let _ = tx.send(StreamEvent::Delta(visible));
                emitted = true;
            }
        }
        if !emitted {
            if let Some(text) = choice
                .and_then(|c| c.get("message"))
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_str())
            {
                let visible = stripper.process(text);
                if !visible.is_empty() {
                    debug_upstream_event("delta", &json!({ "text": &visible }));
                    let _ = tx.send(StreamEvent::Delta(visible));
                    emitted = true;
                }
            }
        }
        if !emitted {
            if let Some(text) = choice
                .and_then(|c| c.get("text"))
                .and_then(|c| c.as_str())
            {
                let visible = stripper.process(text);
                if !visible.is_empty() {
                    debug_upstream_event("delta", &json!({ "text": &visible }));
                    let _ = tx.send(StreamEvent::Delta(visible));
                    emitted = true;
                }
            }
        }

        if emitted {
            sent_any = true;
        } else if diag_buffer.len() < 8 {
            // Capture the first few non-emitting chunks so the empty-response
            // path can print them — invaluable for diagnosing unfamiliar models.
            diag_buffer.push(format!("(no content) {data}"));
        }
    }

    // Flush any trailing visible content (e.g., a partial tag the stripper was holding).
    let tail = stripper.flush();
    if !tail.is_empty() {
        let _ = tx.send(StreamEvent::Delta(tail));
        sent_any = true;
    }

    if !sent_any {
        eprintln!(
            "[ai/vultr] stream completed with zero deltas — model={model}\n\
             status={status}, content-type={content_type}\n\
             first chunks:"
        );
        for line in &diag_buffer {
            eprintln!("    {line}");
        }
    }

    Ok(())
}

/// Strips `<think>...</think>` blocks from a streamed text. The model emits
/// reasoning inline in the content channel; we hide it from the user.
/// Stateful: holds back any partial tag at the end of a chunk so we don't emit
/// "<thi" before knowing whether it's the start of <think>.
#[derive(Default)]
struct ThinkStripper {
    inside: bool,
    /// Partial bytes held back because they might be the start of an open/close tag.
    held: String,
}

impl ThinkStripper {
    fn process(&mut self, chunk: &str) -> String {
        let mut combined = std::mem::take(&mut self.held);
        combined.push_str(chunk);

        let mut out = String::new();
        let mut i = 0;
        let bytes = combined.as_bytes();
        let len = bytes.len();

        while i < len {
            if !self.inside {
                if let Some(rel) = combined[i..].find('<') {
                    let abs = i + rel;
                    out.push_str(&combined[i..abs]);
                    // Check if "<think>" starts here, or might start (partial)
                    let remainder = &combined[abs..];
                    if remainder.starts_with("<think>") {
                        self.inside = true;
                        i = abs + "<think>".len();
                    } else if "<think>".starts_with(remainder) {
                        // Partial — hold back from `abs` to end
                        self.held = combined[abs..].to_string();
                        return out;
                    } else {
                        // Not a think tag — emit the `<` and continue scanning
                        out.push('<');
                        i = abs + 1;
                    }
                } else {
                    out.push_str(&combined[i..]);
                    break;
                }
            } else {
                if let Some(rel) = combined[i..].find('<') {
                    let abs = i + rel;
                    // Drop everything before `abs` (it's inside think)
                    let remainder = &combined[abs..];
                    if remainder.starts_with("</think>") {
                        self.inside = false;
                        i = abs + "</think>".len();
                    } else if "</think>".starts_with(remainder) {
                        // Partial — hold back, but DON'T emit (we're still inside)
                        self.held = remainder.to_string();
                        return out;
                    } else {
                        i = abs + 1;
                    }
                } else {
                    // All remaining content is inside think — drop it
                    break;
                }
            }
        }
        out
    }

    fn flush(&mut self) -> String {
        if self.inside { String::new() } else { std::mem::take(&mut self.held) }
    }
}

fn complete_vultr(model: String, messages: Vec<ChatTurn>) -> Result<String, String> {
    let payload = json!({
        "model": model,
        "stream": false,
        "max_tokens": max_completion_tokens(),
        "messages": messages
            .into_iter()
            .map(|message| json!({ "role": message.role, "content": message.content }))
            .collect::<Vec<_>>()
    });

    let response = agent()
        .post(&format!("{}/chat/completions", vultr_base_url()))
        .set("Authorization", &format!("Bearer {}", vultr_key()?))
        .set("Content-Type", "application/json")
        .send_json(payload)
        .map_err(|e| {
            eprintln!("[ai/vultr] HTTP error from upstream: {e}");
            e.to_string()
        })?;

    let status = response.status();
    let body = response.into_string().map_err(|e| e.to_string())?;
    let value: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| format!("Invalid upstream JSON: {e}"))?;

    if let Some(content) = extract_full_content(&value) {
        if !content.trim().is_empty() {
            return Ok(content);
        }
    }

    if let Some(error) = value
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
    {
        return Err(format!("Vultr error: {error}"));
    }

    Err(format!(
        "Vultr returned no content (status={status}, model={model})"
    ))
}

fn max_completion_tokens() -> usize {
    std::env::var("MIXER_MAX_COMPLETION_TOKENS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| (1024..=16_384).contains(value))
        .unwrap_or(8192)
}

fn debug_upstream_event(event: &str, data: &serde_json::Value) {
    println!(
        "[ai<-upstream {}]\nevent: {}\ndata: {}\n",
        Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        event,
        data
    );
}

/// Pull `choices[0].message.content` (or .delta.content) out of a full
/// non-streaming completion response. Used as a fallback when the provider
/// returns JSON instead of SSE.
fn extract_full_content(value: &serde_json::Value) -> Option<String> {
    let choice = value.get("choices").and_then(|c| c.get(0))?;
    if let Some(text) = choice
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
    {
        return Some(text.to_string());
    }
    if let Some(text) = choice
        .get("delta")
        .and_then(|d| d.get("content"))
        .and_then(|c| c.as_str())
    {
        return Some(text.to_string());
    }
    if let Some(text) = choice.get("text").and_then(|c| c.as_str()) {
        return Some(text.to_string());
    }
    None
}
