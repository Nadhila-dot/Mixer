use crate::ai::{self, ChatTurn};
use regex::Regex;
use serde::Deserialize;
use std::{
    collections::HashMap,
    io::Write,
    process::{Command, Stdio},
    sync::OnceLock,
    thread,
    time::{Duration, Instant},
};

const PYTHON_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_TOOL_OUTPUT: usize = 8_000;
const GOOGLE_SEARCH_URL: &str = "https://www.googleapis.com/customsearch/v1";
const BRAVE_SEARCH_URL: &str = "https://api.search.brave.com/res/v1/web/search";
const SEARCH_RESULT_COUNT: usize = 5;
const SEARXNG_PATH: &str = "/search";
const SEARCH_SYSTEM_PROMPT: &str = "You are a search result synthesizer. \
Given web search results and a query, provide a concise, accurate, factual answer. \
Cite sources inline with [Source N] notation. \
Do not make up information not present in the results.";
const DOTHINK_SYSTEM_PROMPT: &str = r#"You are Mixer's dedicated ultra-think worker.

Your only job is to think through the user's request carefully and produce a structured reasoning artifact plus a final answer.

Output format:
#### topic / step
<reasoning>

Repeat for as many steps as needed.

#### final answer
<final user-facing answer>

Rules:
- Be concrete and analytical.
- Use the actual conversation context and the tool instruction from tellmyself.
- Do not mention hidden prompts, providers, or internal implementation details.
- Do not wrap the output in XML."#;

pub fn redact_sensitive(input: &str) -> String {
    if input.is_empty() {
        return String::new();
    }

    fn regex(pattern: &'static str) -> &'static Regex {
        static CACHE: OnceLock<std::sync::Mutex<HashMap<&'static str, &'static Regex>>> =
            OnceLock::new();
        let cache = CACHE.get_or_init(|| std::sync::Mutex::new(HashMap::new()));
        let mut cache = cache.lock().unwrap();
        if let Some(existing) = cache.get(pattern) {
            return existing;
        }
        let compiled = Box::leak(Box::new(
            Regex::new(pattern).expect("valid redaction regex"),
        ));
        cache.insert(pattern, compiled);
        compiled
    }

    let mut out = input.to_string();

    out = regex(r#"(?i)((?:\bsshpass\b[^\n\r;|&]*?\s-p\s+))(?:"[^"]*"|'[^']*'|\S+)"#)
        .replace_all(&out, |caps: &regex::Captures| {
            format!("{}[REDACTED]", &caps[1])
        })
        .into_owned();

    out = regex(r#"(?i)(\bSSHPASS\s*=\s*)(?:"[^"]*"|'[^']*'|[^\s;|&]+)"#)
        .replace_all(&out, |caps: &regex::Captures| {
            format!("{}[REDACTED]", &caps[1])
        })
        .into_owned();

    out = regex(r#"(?i)(\b(?:password|passwd|passphrase|token|api[_-]?key|secret|private[_-]?key)\b\s*[:=]\s*)(?:"[^"]*"|'[^']*'|[^\s,;|&]+)"#)
        .replace_all(&out, |caps: &regex::Captures| {
            format!("{}[REDACTED]", &caps[1])
        })
        .into_owned();

    out = regex(r#"(?i)([a-z][a-z0-9+.-]*://[^/\s:@]+:)([^@\s/]+)(@)"#)
        .replace_all(&out, |caps: &regex::Captures| {
            format!("{}[REDACTED]{}", &caps[1], &caps[3])
        })
        .into_owned();

    out
}

#[derive(Debug, Clone)]
pub enum AssistantAction {
    Final { content: String },
    Tool(ParsedToolOwned),
}

#[derive(Debug, Clone)]
pub enum ToolExecutionEvent {
    OutputChunk(String),
}

#[derive(Debug, Deserialize)]
struct JsonAssistantEnvelope {
    #[serde(default)]
    r#type: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    content: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    arguments: serde_json::Map<String, serde_json::Value>,
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
}

pub fn has_incomplete_tool_call(raw: &str) -> bool {
    let mut cursor = 0;
    for tool in parsed_tools(raw) {
        cursor = tool.end;
    }
    incomplete_tool(raw, cursor).is_some()
}

pub fn parse_assistant_action(raw: &str) -> Result<AssistantAction, String> {
    let cleaned = strip_think_blocks(raw);
    let trimmed = cleaned.trim();

    if let Some((tool, start, _)) = first_complete_tool(trimmed) {
        let leading = &trimmed[..start];
        if leading.trim().is_empty() {
            if tool.name == "response" {
                return Ok(AssistantAction::Final {
                    content: tool.body.trim().to_string(),
                });
            }
            return Ok(AssistantAction::Tool(tool));
        }
    }

    if starts_like_json_action(trimmed) {
        if let Some(action) = parse_json_assistant_action(trimmed)? {
            return Ok(action);
        }
        return Err("assistant started a JSON action but did not finish a valid action".into());
    }

    if let Some((tool, _, _)) = first_complete_tool(trimmed) {
        if tool.name == "response" {
            return Ok(AssistantAction::Final {
                content: tool.body.trim().to_string(),
            });
        }
        return Ok(AssistantAction::Tool(tool));
    }

    if trimmed.is_empty() {
        return Err("assistant returned empty output".into());
    }

    if looks_like_incomplete_action(raw) || looks_like_incomplete_action(trimmed) {
        return Err("assistant returned an incomplete or malformed agent action".into());
    }

    Ok(AssistantAction::Final {
        content: strip_tool_xml(trimmed),
    })
}

pub fn executable_tool_names(raw: &str) -> Vec<String> {
    parsed_tools(raw)
        .into_iter()
        .filter(|tool| {
            matches!(
                tool.name,
                "python"
                    | "query"
                    | "end"
                    | "done"
                    | "dothink"
                    | "search"
                    | "fetchUrl"
                    | "createTodo"
                    | "completeTodo"
                    | "viewTodo"
                    | "listSkills"
                    | "readSkill"
                    | "CreateWorkspace"
                    | "WorkspaceStatus"
                    | "Command"
                    | "LongRunProcess"
                    | "CreateFile"
                    | "AppendFile"
                    | "PatchFile"
                    | "Preview"
                    | "ReadFile"
                    | "DeleteFile"
                    | "CreateDirectory"
                    | "DeleteDirectory"
                    | "VultrListInstances"
                    | "VultrGetInstance"
                    | "VultrDeployInstance"
                    | "VultrPowerAction"
                    | "VultrListRegions"
                    | "VultrListPlans"
                    | "VultrListOs"
                    | "VultrListSshKeys"
                    | "VultrImportSshKey"
                    | "VultrCheckAccess"
                    | "RemoteCommand"
            )
        })
        .map(|tool| tool.name.to_string())
        .collect()
}

pub fn process_assistant_text(raw: &str, transcript: &str, model: &str, user_id: i64) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut cursor = 0;

    for tool in parsed_tools(raw) {
        out.push_str(&raw[cursor..tool.start]);
        out.push_str(&execute_tool(
            tool.name, tool.attrs, tool.body, transcript, model, user_id,
        ));
        cursor = tool.end;
    }

    if let Some(incomplete) = incomplete_tool(raw, cursor) {
        out.push_str(&raw[cursor..incomplete.start]);
        out.push_str(&incomplete_tool_result(incomplete.name, incomplete.body));
        cursor = raw.len();
    }

    out.push_str(&raw[cursor..]);
    out
}

pub fn process_assistant_text_with_callback<F>(
    raw: &str,
    transcript: &str,
    model: &str,
    user_id: i64,
    mut on_tool_done: F,
) -> String
where
    F: FnMut(&str),
{
    let mut out = String::with_capacity(raw.len());
    let mut cursor = 0;

    for tool in parsed_tools(raw) {
        out.push_str(&raw[cursor..tool.start]);
        out.push_str(&execute_tool(
            tool.name, tool.attrs, tool.body, transcript, model, user_id,
        ));
        cursor = tool.end;
        on_tool_done(&out);
    }

    if let Some(incomplete) = incomplete_tool(raw, cursor) {
        out.push_str(&raw[cursor..incomplete.start]);
        out.push_str(&incomplete_tool_result(incomplete.name, incomplete.body));
        cursor = raw.len();
    }

    out.push_str(&raw[cursor..]);
    out
}

struct ParsedTool<'a> {
    name: &'a str,
    attrs: HashMap<String, String>,
    body: &'a str,
    start: usize,
    end: usize,
}

/// Owned (heap-allocated) version of ParsedTool — used by the run loop so the
/// parsed tool can outlive the raw input slice.
#[derive(Debug, Clone)]
pub struct ParsedToolOwned {
    pub name: String,
    pub attrs: HashMap<String, String>,
    pub args_json: String,
    pub body: String,
}

/// Parse all complete tool calls in `raw`, returning owned values.
pub fn parsed_tools_owned(raw: &str) -> Vec<ParsedToolOwned> {
    parsed_tools(raw)
        .into_iter()
        .map(|t| {
            let args_json = serde_json::to_string(
                &t.attrs.iter().collect::<std::collections::BTreeMap<_, _>>(),
            )
            .unwrap_or_else(|_| "{}".into());
            ParsedToolOwned {
                name: t.name.to_string(),
                attrs: t.attrs,
                args_json,
                body: t.body.to_string(),
            }
        })
        .collect()
}

/// Execute a single tool by name with owned args. Returns the tool_result XML.
pub fn execute_tool_owned(
    name: &str,
    attrs: &HashMap<String, String>,
    body: &str,
    transcript: &str,
    model: &str,
    user_id: i64,
) -> String {
    execute_tool(name, attrs.clone(), body, transcript, model, user_id)
}

pub fn execute_tool_owned_with_events<F>(
    name: &str,
    attrs: &HashMap<String, String>,
    body: &str,
    transcript: &str,
    model: &str,
    user_id: i64,
    mut on_event: F,
) -> String
where
    F: FnMut(ToolExecutionEvent),
{
    execute_tool_with_events(
        name,
        attrs.clone(),
        body,
        transcript,
        model,
        user_id,
        &mut on_event,
    )
}

/// Strip tool XML blocks from `raw`, returning only the prose text between them.
/// Used to extract the model's user-facing commentary while preserving tool calls.
pub fn strip_tool_xml(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut cursor = 0;
    for tool in parsed_tools(raw) {
        out.push_str(&raw[cursor..tool.start]);
        cursor = tool.end;
    }
    out.push_str(&raw[cursor..]);
    out.trim().to_string()
}

/// Build a tool_result XML wrapper for a given tool name + content.
pub fn tool_result_xml(name: &str, content: &str) -> String {
    tool_result(name, content)
}

pub fn canonical_action_json(action: &AssistantAction) -> String {
    match action {
        AssistantAction::Final { content } => serde_json::json!({
            "type": "final",
            "content": content,
        })
        .to_string(),
        AssistantAction::Tool(tool) => serde_json::json!({
            "type": "tool_call",
            "name": tool.name,
            "arguments": tool.attrs,
            "body": tool.body,
        })
        .to_string(),
    }
}

pub fn looks_like_internal_action(raw: &str) -> bool {
    let cleaned = strip_think_blocks(raw);
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return false;
    }
    looks_like_incomplete_action(trimmed)
        || trimmed.starts_with("<tool_result")
        || trimmed.starts_with("<CreateWorkspace")
        || trimmed.starts_with("<CreateFile")
        || trimmed.starts_with("<AppendFile")
        || trimmed.starts_with("<Command")
        || trimmed.starts_with("<LongRunProcess")
        || trimmed.starts_with("<Vultr")
        || trimmed.starts_with("<RemoteCommand")
        || (trimmed.starts_with('{')
            && (trimmed.contains("\"arguments\"")
                || trimmed.contains("\"tool_call\"")
                || trimmed.contains("\"assistant_message_uuid\"")
                || trimmed.contains("\"final_text\"")))
}

pub fn decode_xml_entities(value: &str) -> String {
    unescape_xml(value)
}

/// Incremental scanner: returns the first COMPLETE tool call in `raw` along
/// with its byte range, or `None` if there isn't a complete tool yet.
/// Used by the ReAct loop to stop streaming as soon as a tool is observed.
pub fn first_complete_tool(raw: &str) -> Option<(ParsedToolOwned, usize, usize)> {
    let tools = parsed_tools(raw);
    let first = tools.into_iter().next()?;
    let args_json = serde_json::to_string(
        &first
            .attrs
            .iter()
            .collect::<std::collections::BTreeMap<_, _>>(),
    )
    .unwrap_or_else(|_| "{}".into());
    Some((
        ParsedToolOwned {
            name: first.name.to_string(),
            attrs: first.attrs.clone(),
            args_json,
            body: first.body.to_string(),
        },
        first.start,
        first.end,
    ))
}

struct IncompleteTool<'a> {
    name: &'a str,
    body: &'a str,
    start: usize,
}

fn parsed_tools(raw: &str) -> Vec<ParsedTool<'_>> {
    let mut tools = Vec::new();
    let mut cursor = 0;

    while let Some(relative_start) = raw[cursor..].find('<') {
        let start = cursor + relative_start;
        let Some(open_end) = raw[start..].find('>').map(|offset| start + offset) else {
            break;
        };

        let open = &raw[start + 1..open_end];
        let Some((name, attrs, self_closing)) = parse_open_tag(open) else {
            cursor = open_end + 1;
            continue;
        };

        if !is_supported_tool(name) {
            cursor = open_end + 1;
            continue;
        }

        if self_closing {
            tools.push(ParsedTool {
                name,
                attrs,
                body: "",
                start,
                end: open_end + 1,
            });
            cursor = open_end + 1;
            continue;
        }

        let close_tag = format!("</{name}>");
        let Some(close_start) = raw[open_end + 1..]
            .find(&close_tag)
            .map(|offset| open_end + 1 + offset)
        else {
            // No closing tag — everything after this opening tag belongs to this incomplete
            // tool's body. Stop scanning so inner tags (e.g. <html> inside <CreateFile>)
            // are not falsely executed as separate tools.
            break;
        };

        tools.push(ParsedTool {
            name,
            attrs,
            body: &raw[open_end + 1..close_start],
            start,
            end: close_start + close_tag.len(),
        });
        cursor = close_start + close_tag.len();
    }

    tools
}

fn incomplete_tool(raw: &str, from: usize) -> Option<IncompleteTool<'_>> {
    let mut cursor = from;

    while let Some(relative_start) = raw[cursor..].find('<') {
        let start = cursor + relative_start;
        let Some(open_end) = raw[start..].find('>').map(|offset| start + offset) else {
            return None;
        };
        let open = &raw[start + 1..open_end];
        let Some((name, _attrs, self_closing)) = parse_open_tag(open) else {
            cursor = open_end + 1;
            continue;
        };

        if !is_supported_tool(name) {
            cursor = open_end + 1;
            continue;
        }

        if self_closing {
            cursor = open_end + 1;
            continue;
        }

        let close_tag = format!("</{name}>");
        if raw[open_end + 1..].contains(&close_tag) {
            cursor = open_end + 1;
            continue;
        }

        return Some(IncompleteTool {
            name,
            body: &raw[open_end + 1..],
            start,
        });
    }

    None
}

fn parse_json_assistant_action(raw: &str) -> Result<Option<AssistantAction>, String> {
    let Some(json_text) = extract_first_json_object(raw) else {
        return Ok(None);
    };

    let envelope: JsonAssistantEnvelope =
        serde_json::from_str(&json_text).map_err(|e| format!("invalid agent json: {e}"))?;
    let kind = envelope.r#type.trim();

    match kind {
        "final" | "response" => {
            let content = strip_think_blocks(&envelope.content);
            let content = content.trim();
            if content.is_empty() {
                return Err("final action is missing content".into());
            }
            Ok(Some(AssistantAction::Final {
                content: content.to_string(),
            }))
        }
        "tool_call" | "tool" => parse_json_tool_action(envelope).map(Some),
        other if other.is_empty() && looks_like_json_tool_envelope(&envelope) => {
            parse_json_tool_action(envelope).map(Some)
        }
        other if other.is_empty() => Err("agent json is missing type".into()),
        other => Err(format!("unsupported agent action type '{other}'")),
    }
}

fn parse_json_tool_action(envelope: JsonAssistantEnvelope) -> Result<AssistantAction, String> {
    let mut merged_args = envelope.arguments;
    for (key, value) in envelope.extra {
        if key != "type" && key != "name" && key != "content" && key != "body" {
            merged_args.entry(key).or_insert(value);
        }
    }

    let inferred_name = infer_tool_name(envelope.name.trim(), &merged_args, &envelope.body);
    let name = inferred_name.trim();
    if !is_supported_tool(name) {
        return Err(format!("unsupported tool '{name}'"));
    }
    let body = merged_args
        .remove("body")
        .map(json_value_to_attr_string)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| envelope.body.clone());
    let attrs = merged_args
        .into_iter()
        .map(|(k, v)| (k, json_value_to_attr_string(v)))
        .collect::<HashMap<_, _>>();
    let args_json =
        serde_json::to_string(&attrs.iter().collect::<std::collections::BTreeMap<_, _>>())
            .unwrap_or_else(|_| "{}".into());
    Ok(AssistantAction::Tool(ParsedToolOwned {
        name: name.to_string(),
        attrs,
        args_json,
        body,
    }))
}

fn looks_like_json_tool_envelope(envelope: &JsonAssistantEnvelope) -> bool {
    if !envelope.name.trim().is_empty() || !envelope.body.trim().is_empty() {
        return true;
    }
    envelope.arguments.contains_key("id")
        || envelope.arguments.contains_key("path")
        || envelope.arguments.contains_key("command")
        || envelope.arguments.contains_key("query")
        || envelope.arguments.contains_key("regex")
        || envelope.arguments.contains_key("tellmyself")
        || envelope.arguments.contains_key("task")
        || envelope.arguments.contains_key("name")
        || envelope.arguments.contains_key("body")
}

fn starts_like_json_action(raw: &str) -> bool {
    let trimmed = raw.trim_start();
    trimmed.starts_with('{') || trimmed.starts_with("```json") || trimmed.starts_with("```")
}

fn looks_like_incomplete_action(raw: &str) -> bool {
    let trimmed = raw.trim_start();
    if trimmed.starts_with("<think>") {
        return true;
    }
    if starts_like_json_action(trimmed) {
        return true;
    }
    trimmed.contains("\"type\":\"tool_call\"")
        || trimmed.contains("\"type\": \"tool_call\"")
        || trimmed.contains("\"arguments\"")
        || trimmed.contains("\"assistant_message_uuid\"")
        || trimmed.contains("\"final_text\"")
        || has_incomplete_tool_call(trimmed)
}

fn extract_first_json_object(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let candidate = if trimmed.starts_with("```") {
        trimmed
            .strip_prefix("```json")
            .or_else(|| trimmed.strip_prefix("```"))
            .map(str::trim_start)?
    } else {
        trimmed
    };
    let candidate = candidate
        .strip_suffix("```")
        .map(str::trim_end)
        .unwrap_or(candidate);

    let mut start = None;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (idx, ch) in candidate.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
                continue;
            }
            match ch {
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => {
                if depth == 0 {
                    start = Some(idx);
                }
                depth += 1;
            }
            '}' => {
                if depth == 0 {
                    continue;
                }
                depth -= 1;
                if depth == 0 {
                    let start = start?;
                    return Some(candidate[start..=idx].to_string());
                }
            }
            _ => {}
        }
    }

    None
}

fn json_value_to_attr_string(value: serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::Bool(v) => {
            if v {
                "true".into()
            } else {
                "false".into()
            }
        }
        serde_json::Value::Number(v) => v.to_string(),
        serde_json::Value::String(v) => v,
        other => other.to_string(),
    }
}

fn infer_tool_name(
    explicit_name: &str,
    args: &serde_json::Map<String, serde_json::Value>,
    body: &str,
) -> String {
    if !explicit_name.trim().is_empty() {
        return explicit_name.trim().to_string();
    }

    let has = |key: &str| {
        args.get(key)
            .is_some_and(|value| !json_value_to_attr_string(value.clone()).trim().is_empty())
    };
    let body_present = !body.trim().is_empty()
        || args
            .get("body")
            .is_some_and(|value| !json_value_to_attr_string(value.clone()).trim().is_empty());

    if has("regex") {
        return "query".into();
    }
    if has("tellmyself") {
        return "dothink".into();
    }
    if has("url") {
        return "fetchUrl".into();
    }
    if has("query") {
        return "search".into();
    }
    if has("task") {
        return "completeTodo".into();
    }
    if has("instance_id") && has("command") {
        return "RemoteCommand".into();
    }
    if has("instance_id") && has("action") {
        return "VultrPowerAction".into();
    }
    if has("instance_id") && has("verify") {
        return "VultrCheckAccess".into();
    }
    if has("instance_id") {
        return "VultrGetInstance".into();
    }
    if has("region") && has("plan") && has("os_id") {
        return "VultrDeployInstance".into();
    }
    if has("public_key") && has("name") {
        return "VultrImportSshKey".into();
    }
    if has("path") && has("id") && has("command") {
        return "Command".into();
    }
    if has("command") && has("id") {
        let timeout = args
            .get("timeout")
            .map(|value| json_value_to_attr_string(value.clone()))
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        return if timeout > 30 {
            "LongRunProcess".into()
        } else {
            "Command".into()
        };
    }
    if has("path") && has("id") && body_present {
        return "CreateFile".into();
    }
    if has("path") && has("id") {
        return "ReadFile".into();
    }
    if has("name") {
        return "readSkill".into();
    }
    if has("id") {
        return "CreateWorkspace".into();
    }

    String::new()
}

fn strip_think_blocks(input: &str) -> String {
    let mut out = String::new();
    let mut cursor = 0;

    while let Some(start_rel) = input[cursor..].find("<think>") {
        let start = cursor + start_rel;
        out.push_str(&input[cursor..start]);
        let content_start = start + "<think>".len();
        if let Some(end_rel) = input[content_start..].find("</think>") {
            cursor = content_start + end_rel + "</think>".len();
        } else {
            cursor = input.len();
            break;
        }
    }

    if cursor < input.len() {
        out.push_str(&input[cursor..]);
    }
    out
}

fn parse_open_tag(open: &str) -> Option<(&str, HashMap<String, String>, bool)> {
    let open = open.trim();
    if open.starts_with('/') {
        return None;
    }
    let self_closing = open.ends_with('/');
    let open = if self_closing {
        open[..open.len().saturating_sub(1)].trim_end()
    } else {
        open
    };

    let mut split_at = open.len();
    for (i, ch) in open.char_indices() {
        if ch.is_ascii_whitespace() {
            split_at = i;
            break;
        }
    }

    let name = &open[..split_at];
    if name.is_empty()
        || !name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    {
        return None;
    }

    Some((name, parse_attrs(&open[split_at..]), self_closing))
}

fn parse_attrs(raw: &str) -> HashMap<String, String> {
    let mut attrs = HashMap::new();
    let mut cursor = 0;
    let bytes = raw.as_bytes();

    while cursor < bytes.len() {
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        let key_start = cursor;
        while cursor < bytes.len()
            && (bytes[cursor].is_ascii_alphanumeric() || matches!(bytes[cursor], b'_' | b'-'))
        {
            cursor += 1;
        }
        if key_start == cursor {
            break;
        }

        let key = &raw[key_start..cursor];
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() || bytes[cursor] != b'=' {
            continue;
        }
        cursor += 1;
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() || bytes[cursor] != b'"' {
            continue;
        }
        cursor += 1;
        let value_start = cursor;
        while cursor < bytes.len() && bytes[cursor] != b'"' {
            cursor += 1;
        }
        if cursor > value_start {
            attrs.insert(key.to_string(), unescape_xml(&raw[value_start..cursor]));
        }
        cursor = cursor.saturating_add(1);
    }

    attrs
}

fn is_supported_tool(name: &str) -> bool {
    matches!(
        name,
        "response"
            | "python"
            | "query"
            | "end"
            | "done"
            | "code"
            | "html"
            | "dothink"
            | "search"
            | "fetchUrl"
            | "createTodo"
            | "completeTodo"
            | "viewTodo"
            | "listSkills"
            | "readSkill"
            | "CreateWorkspace"
            | "WorkspaceStatus"
            | "Command"
            | "LongRunProcess"
            | "CreateFile"
            | "AppendFile"
            | "PatchFile"
            | "Preview"
            | "ReadFile"
            | "DeleteFile"
            | "CreateDirectory"
            | "DeleteDirectory"
            | "VultrListInstances"
            | "VultrGetInstance"
            | "VultrDeployInstance"
            | "VultrPowerAction"
            | "VultrListRegions"
            | "VultrListPlans"
            | "VultrListOs"
            | "VultrListSshKeys"
            | "VultrImportSshKey"
            | "VultrCheckAccess"
            | "RemoteCommand"
    )
}

fn execute_tool(
    name: &str,
    attrs: HashMap<String, String>,
    body: &str,
    transcript: &str,
    model: &str,
    user_id: i64,
) -> String {
    execute_tool_with_events(name, attrs, body, transcript, model, user_id, &mut |_| {})
}

fn execute_tool_with_events<F>(
    name: &str,
    attrs: HashMap<String, String>,
    body: &str,
    transcript: &str,
    model: &str,
    user_id: i64,
    on_event: &mut F,
) -> String
where
    F: FnMut(ToolExecutionEvent),
{
    match name {
        "response" => body.trim().to_string(),
        "python" => tool_result("python", &run_python(body)),
        "query" => tool_result(
            "query",
            &run_query(
                attrs.get("regex").map(String::as_str).unwrap_or_default(),
                transcript,
            ),
        ),
        "end" => tool_result("end", body.trim()),
        "done" => tool_result(
            "done",
            if body.trim().is_empty() {
                "Task execution complete."
            } else {
                body.trim()
            },
        ),
        "dothink" => run_dothink(&attrs, body, transcript, model),
        "search" => run_search(
            attrs
                .get("query")
                .map(String::as_str)
                .unwrap_or(body.trim()),
            model,
        ),
        "fetchUrl" => run_fetch_url(attrs.get("url").map(String::as_str).unwrap_or(body.trim())),
        "createTodo" => run_create_todo(body),
        "completeTodo" => run_complete_todo(
            attrs.get("task").map(String::as_str).unwrap_or(body.trim()),
            transcript,
        ),
        "viewTodo" => run_view_todo(transcript),
        "listSkills" => tool_result("listSkills", &crate::skills::list_skills()),
        "readSkill" => {
            let skill_name = attrs.get("name").map(String::as_str).unwrap_or(body.trim());
            tool_result("readSkill", &crate::skills::read_skill(skill_name))
        }
        "CreateWorkspace" => {
            let name = attrs.get("id").map(String::as_str).unwrap_or("workspace");
            ws_create_workspace(user_id, name)
        }
        "WorkspaceStatus" => {
            let ws_id = attrs.get("id").map(String::as_str).unwrap_or("");
            ws_status(user_id, ws_id)
        }
        "Command" => {
            let ws_id = attrs.get("id").map(String::as_str).unwrap_or("");
            let command = attrs
                .get("command")
                .map(String::as_str)
                .unwrap_or(body.trim());
            let timeout = attrs
                .get("timeout")
                .and_then(|t| t.parse::<u64>().ok())
                .unwrap_or(10);
            ws_command(user_id, ws_id, command, timeout, on_event)
        }
        "LongRunProcess" => {
            let ws_id = attrs.get("id").map(String::as_str).unwrap_or("");
            let command = attrs
                .get("command")
                .map(String::as_str)
                .unwrap_or(body.trim());
            let timeout = attrs
                .get("timeout")
                .and_then(|t| t.parse::<u64>().ok())
                .unwrap_or(60);
            ws_long_process(user_id, ws_id, command, timeout, on_event)
        }
        "CreateFile" => {
            let ws_id = attrs.get("id").map(String::as_str).unwrap_or("");
            let path = attrs.get("path").map(String::as_str).unwrap_or("");
            ws_create_file(user_id, ws_id, path, body)
        }
        "AppendFile" => {
            let ws_id = attrs.get("id").map(String::as_str).unwrap_or("");
            let path = attrs.get("path").map(String::as_str).unwrap_or("");
            ws_append_file(user_id, ws_id, path, body)
        }
        "PatchFile" => {
            let ws_id = attrs.get("id").map(String::as_str).unwrap_or("");
            let path = attrs.get("path").map(String::as_str).unwrap_or("");
            ws_patch_file(user_id, ws_id, path, body)
        }
        "Preview" => {
            let ws_id = attrs.get("id").map(String::as_str).unwrap_or("");
            let path = attrs
                .get("path")
                .or_else(|| attrs.get("file"))
                .map(String::as_str)
                .unwrap_or(body.trim());
            ws_preview(user_id, ws_id, path)
        }
        "ReadFile" => {
            let ws_id = attrs.get("id").map(String::as_str).unwrap_or("");
            let path = attrs.get("path").map(String::as_str).unwrap_or("");
            let start_line = attrs
                .get("start")
                .or_else(|| attrs.get("line"))
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(1);
            let max_lines = attrs
                .get("lines")
                .or_else(|| attrs.get("max_lines"))
                .and_then(|value| value.parse::<usize>().ok())
                .unwrap_or(500);
            ws_read_file(user_id, ws_id, path, start_line, max_lines)
        }
        "DeleteFile" => {
            let ws_id = attrs.get("id").map(String::as_str).unwrap_or("");
            let path = attrs.get("path").map(String::as_str).unwrap_or("");
            ws_delete_file(user_id, ws_id, path)
        }
        "CreateDirectory" => {
            let ws_id = attrs.get("id").map(String::as_str).unwrap_or("");
            let path = attrs.get("path").map(String::as_str).unwrap_or(body.trim());
            ws_create_directory(user_id, ws_id, path)
        }
        "DeleteDirectory" => {
            let ws_id = attrs.get("id").map(String::as_str).unwrap_or("");
            let path = attrs.get("path").map(String::as_str).unwrap_or(body.trim());
            ws_delete_directory(user_id, ws_id, path)
        }
        "VultrListInstances" => vultr_list_instances(user_id),
        "VultrGetInstance" => {
            let instance_id = attrs
                .get("instance_id")
                .or_else(|| attrs.get("id"))
                .map(String::as_str)
                .unwrap_or(body.trim());
            vultr_get_instance(user_id, instance_id)
        }
        "VultrDeployInstance" => vultr_deploy_instance(user_id, &attrs),
        "VultrPowerAction" => {
            let instance_id = attrs
                .get("instance_id")
                .or_else(|| attrs.get("id"))
                .map(String::as_str)
                .unwrap_or("");
            let action = attrs
                .get("action")
                .map(String::as_str)
                .unwrap_or(body.trim());
            vultr_power_action(user_id, instance_id, action)
        }
        "VultrListRegions" => vultr_list_regions(user_id),
        "VultrListPlans" => vultr_list_plans(user_id),
        "VultrListOs" => vultr_list_os(user_id),
        "VultrListSshKeys" => vultr_list_ssh_keys(user_id),
        "VultrImportSshKey" => {
            let name = attrs.get("name").map(String::as_str).unwrap_or("");
            let public_key = attrs
                .get("public_key")
                .or_else(|| attrs.get("key"))
                .map(String::as_str)
                .unwrap_or(body.trim());
            vultr_import_ssh_key(user_id, name, public_key)
        }
        "VultrCheckAccess" => {
            let instance_id = attrs
                .get("instance_id")
                .or_else(|| attrs.get("id"))
                .map(String::as_str)
                .unwrap_or("");
            let verify = attrs
                .get("verify")
                .map(|value| matches!(value.as_str(), "true" | "1" | "yes"))
                .unwrap_or(false);
            vultr_check_access(user_id, instance_id, verify)
        }
        "RemoteCommand" => {
            let instance_id = attrs
                .get("instance_id")
                .or_else(|| attrs.get("id"))
                .map(String::as_str)
                .unwrap_or("");
            let timeout = attrs
                .get("timeout")
                .and_then(|t| t.parse::<u64>().ok())
                .unwrap_or(20);
            let command = attrs
                .get("command")
                .map(String::as_str)
                .unwrap_or(body.trim());
            remote_command(user_id, instance_id, command, timeout)
        }
        "code" | "html" => rebuild_tool(name, attrs, body),
        _ => body.to_string(),
    }
}

// ── Workspace tool helpers ──────────────────────────────────────────────────

/// Resolves a workspace handle to (directory, canonical_uuid).
///
/// The agent sometimes passes the friendly name ("mixer") instead of the full id
/// ("mixer-034f32d0"). This resolver accepts either:
///   1. Exact uuid match (the canonical case).
///   2. Exact name match — single hit wins; ambiguous names resolve to the most recent.
///   3. Prefix match on either uuid or name.
///
/// The returned canonical_uuid is what the tool result advertises to the frontend,
/// so the UI updates the correct workspace even when the agent supplied the short name.
fn resolve_workspace(user_id: i64, ws_id: &str) -> Result<(std::path::PathBuf, String), String> {
    if ws_id.is_empty() {
        return Err(
            "No workspace id provided. Use <CreateWorkspace id=\"name\"/> first.".to_string(),
        );
    }
    let store = crate::db::auth_store();

    // 1. Exact uuid match
    if let Ok(Some(ws)) = store.get_workspace(user_id, ws_id) {
        return Ok((crate::workspace::workspace_dir(user_id, &ws.uuid), ws.uuid));
    }

    // 2. Fallback: scan this user's workspaces for a name or prefix match.
    let workspaces = match store.list_workspaces(user_id) {
        Ok(list) => list,
        Err(e) => return Err(format!("Database error: {e}")),
    };

    // list_workspaces returns ORDER BY created_at DESC, so by_name[0] is the most recent.
    let by_name: Vec<&crate::db::Workspace> =
        workspaces.iter().filter(|w| w.name == ws_id).collect();
    if let Some(ws) = by_name.first() {
        return Ok((
            crate::workspace::workspace_dir(user_id, &ws.uuid),
            ws.uuid.clone(),
        ));
    }

    let by_prefix: Vec<&crate::db::Workspace> = workspaces
        .iter()
        .filter(|w| w.uuid.starts_with(ws_id) || w.name.starts_with(ws_id))
        .collect();
    if by_prefix.len() == 1 {
        let ws = by_prefix[0];
        return Ok((
            crate::workspace::workspace_dir(user_id, &ws.uuid),
            ws.uuid.clone(),
        ));
    }

    if workspaces.is_empty() {
        return Err(format!(
            "Workspace '{ws_id}' not found. No workspaces exist yet for this user — call <CreateWorkspace id=\"name\"/> first."
        ));
    }

    let available = workspaces
        .iter()
        .take(8)
        .map(|w| format!("{} (name: {})", w.uuid, w.name))
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!(
        "Workspace '{ws_id}' not found. Use the full workspace_id returned by CreateWorkspace (not just the name). Available workspaces: {available}"
    ))
}

fn ws_tool_result(action: &str, ws_id: &str, body: &str, file_tree_json: &str) -> String {
    // Embed file_tree as an attribute so the frontend can parse and render it.
    // Escape the body too: command output and file contents can contain XML/HTML
    // fragments that would otherwise corrupt the chat parser.
    let tree_escaped = escape_xml(file_tree_json);
    let body_escaped = escape_xml(&redact_sensitive(body));
    format!(
        "<tool_result tool=\"workspace\" action=\"{}\" workspace_id=\"{}\" file_tree=\"{}\">{}</tool_result>",
        escape_xml(action),
        escape_xml(ws_id),
        tree_escaped,
        body_escaped,
    )
}

fn ws_create_workspace(user_id: i64, name: &str) -> String {
    let clean_name = name
        .trim()
        .replace(|c: char| !c.is_alphanumeric() && c != '-' && c != '_', "-");
    let ws_id = format!("{}-{}", clean_name, &uuid_hex()[..8]);
    let store = crate::db::auth_store();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    if let Err(e) = store.create_workspace(user_id, &ws_id, &clean_name, now) {
        return tool_result("workspace", &format!("Failed to create workspace: {e}"));
    }
    match crate::workspace::init_workspace_dir(user_id, &ws_id) {
        Ok(dir) => {
            let tree = crate::workspace::file_tree_json(&dir);
            ws_tool_result(
                "CreateWorkspace",
                &ws_id,
                &format!(
                    "Workspace created.\n\
                     workspace_id: {ws_id}    ← use this exact value as id=\"...\" in every subsequent workspace tool call\n\
                     name: {clean_name}\n\
                     path: {}",
                    dir.display(),
                ),
                &tree,
            )
        }
        Err(e) => tool_result(
            "workspace",
            &format!("Workspace registered but directory creation failed: {e}"),
        ),
    }
}

fn ws_status(user_id: i64, ws_id: &str) -> String {
    match resolve_workspace(user_id, ws_id) {
        Ok((dir, canonical)) => {
            let tree = crate::workspace::file_tree_json(&dir);
            ws_tool_result(
                "WorkspaceStatus",
                &canonical,
                &format!("Workspace: {canonical}\n{}", format_tree_text(&dir)),
                &tree,
            )
        }
        Err(e) => tool_result("workspace", &e),
    }
}

fn ws_command<F>(user_id: i64, ws_id: &str, command: &str, timeout: u64, on_event: &mut F) -> String
where
    F: FnMut(ToolExecutionEvent),
{
    match resolve_workspace(user_id, ws_id) {
        Ok((dir, canonical)) => {
            let result = crate::workspace::run_command_streaming(&dir, command, timeout, |chunk| {
                on_event(ToolExecutionEvent::OutputChunk(redact_sensitive(chunk)));
            });
            let tree = crate::workspace::file_tree_json(&dir);
            let safe_command = redact_sensitive(command);
            let safe_output = redact_sensitive(&result.combined_output);
            let body = format!(
                "$ {safe_command}\n{}{}\nExit: {} ({}ms{})",
                safe_output,
                if safe_output.ends_with('\n') {
                    ""
                } else {
                    "\n"
                },
                result.exit_code,
                result.duration_ms,
                if result.timed_out { ", timed out" } else { "" },
            );
            ws_tool_result("Command", &canonical, &body, &tree)
        }
        Err(e) => tool_result("workspace", &e),
    }
}

fn ws_long_process<F>(
    user_id: i64,
    ws_id: &str,
    command: &str,
    timeout: u64,
    on_event: &mut F,
) -> String
where
    F: FnMut(ToolExecutionEvent),
{
    match resolve_workspace(user_id, ws_id) {
        Ok((dir, canonical)) => {
            let result =
                crate::workspace::run_long_process_streaming(&dir, command, timeout, |chunk| {
                    on_event(ToolExecutionEvent::OutputChunk(redact_sensitive(chunk)));
                });
            let tree = crate::workspace::file_tree_json(&dir);
            let safe_command = redact_sensitive(command);
            let safe_output = redact_sensitive(&result.combined_output);
            let proxy_url = if result.port > 0 {
                format!(
                    "/api/workspaces/{}/port/{}/",
                    percent_encode(&canonical),
                    result.port,
                )
            } else {
                String::new()
            };
            let public_url = public_app_url(result.port);
            let pid_line = result
                .pid
                .map(|pid| format!("\nPID: {pid}"))
                .unwrap_or_default();
            let body = format!(
                "$ {safe_command}\n{}{}\nExit: {} ({}ms)\nPORT: {}\nRUNNING: {}{}{}{}",
                safe_output,
                if safe_output.ends_with('\n') {
                    ""
                } else {
                    "\n"
                },
                result.exit_code,
                result.duration_ms,
                result.port,
                result.running,
                pid_line,
                if proxy_url.is_empty() {
                    String::new()
                } else {
                    format!("\nAPP_PREVIEW_URL: {proxy_url}")
                },
                public_url
                    .map(|url| format!("\nPUBLIC_URL: {url}"))
                    .unwrap_or_default(),
            );
            ws_tool_result("LongRunProcess", &canonical, &body, &tree)
        }
        Err(e) => tool_result("workspace", &e),
    }
}

fn public_app_url(port: u16) -> Option<String> {
    if port == 0 {
        return None;
    }
    let host = std::env::var("PUBLIC_HOST")
        .or_else(|_| std::env::var("PUBLIC_IP"))
        .or_else(|_| std::env::var("VULTR_PUBLIC_IP"))
        .ok()
        .map(|value| {
            value
                .trim()
                .trim_start_matches("http://")
                .trim_start_matches("https://")
                .trim_end_matches('/')
                .to_string()
        })
        .filter(|value| !value.is_empty())?;
    Some(format!("http://{host}:{port}/"))
}

fn ws_create_file(user_id: i64, ws_id: &str, path: &str, content: &str) -> String {
    match resolve_workspace(user_id, ws_id) {
        Ok((dir, canonical)) => match crate::workspace::create_file(&dir, path, content) {
            Ok(()) => {
                let tree = crate::workspace::file_tree_json(&dir);
                ws_tool_result("CreateFile", &canonical, &format!("Created: {path}"), &tree)
            }
            Err(e) => tool_result("workspace", &format!("CreateFile failed: {e}")),
        },
        Err(e) => tool_result("workspace", &e),
    }
}

fn ws_append_file(user_id: i64, ws_id: &str, path: &str, content: &str) -> String {
    match resolve_workspace(user_id, ws_id) {
        Ok((dir, canonical)) => match crate::workspace::append_file(&dir, path, content) {
            Ok(()) => {
                let tree = crate::workspace::file_tree_json(&dir);
                ws_tool_result(
                    "AppendFile",
                    &canonical,
                    &format!("Appended to: {path}"),
                    &tree,
                )
            }
            Err(e) => tool_result("workspace", &format!("AppendFile failed: {e}")),
        },
        Err(e) => tool_result("workspace", &e),
    }
}

fn ws_patch_file(user_id: i64, ws_id: &str, path: &str, patch: &str) -> String {
    match resolve_workspace(user_id, ws_id) {
        Ok((dir, canonical)) => match crate::workspace::patch_file(&dir, path, patch) {
            Ok(diff) => {
                let tree = crate::workspace::file_tree_json(&dir);
                ws_tool_result(
                    "PatchFile",
                    &canonical,
                    &format!("Patched: {path}\n\n{diff}"),
                    &tree,
                )
            }
            Err(e) => tool_result("workspace", &format!("PatchFile failed: {e}")),
        },
        Err(e) => tool_result("workspace", &e),
    }
}

fn ws_preview(user_id: i64, ws_id: &str, path: &str) -> String {
    match resolve_workspace(user_id, ws_id) {
        Ok((dir, canonical)) => {
            let tree = crate::workspace::file_tree_json(&dir);
            let url = format!(
                "/api/workspaces/{}/preview?path={}",
                percent_encode(&canonical),
                percent_encode(path),
            );
            ws_tool_result(
                "Preview",
                &canonical,
                &format!("Preview: {path}\nURL: {url}"),
                &tree,
            )
        }
        Err(e) => tool_result("workspace", &e),
    }
}

fn ws_read_file(
    user_id: i64,
    ws_id: &str,
    path: &str,
    start_line: usize,
    max_lines: usize,
) -> String {
    match resolve_workspace(user_id, ws_id) {
        Ok((dir, canonical)) => {
            match crate::workspace::read_file_window(&dir, path, start_line, max_lines) {
                Ok(content) => {
                    let tree = crate::workspace::file_tree_json(&dir);
                    ws_tool_result(
                        "ReadFile",
                        &canonical,
                        &format!("File: {path}\n\n{content}"),
                        &tree,
                    )
                }
                Err(e) => tool_result("workspace", &format!("ReadFile failed: {e}")),
            }
        }
        Err(e) => tool_result("workspace", &e),
    }
}

fn ws_delete_file(user_id: i64, ws_id: &str, path: &str) -> String {
    if !path.trim().is_empty() {
        return tool_result(
            "workspace",
            &format!(
                "Approval required: deleting file '{path}' requires user approval. Ask the user before running DeleteFile."
            ),
        );
    }
    match resolve_workspace(user_id, ws_id) {
        Ok((dir, canonical)) => match crate::workspace::delete_file(&dir, path) {
            Ok(()) => {
                let tree = crate::workspace::file_tree_json(&dir);
                ws_tool_result("DeleteFile", &canonical, &format!("Deleted: {path}"), &tree)
            }
            Err(e) => tool_result("workspace", &format!("DeleteFile failed: {e}")),
        },
        Err(e) => tool_result("workspace", &e),
    }
}

fn ws_create_directory(user_id: i64, ws_id: &str, path: &str) -> String {
    match resolve_workspace(user_id, ws_id) {
        Ok((dir, canonical)) => match crate::workspace::create_directory(&dir, path) {
            Ok(()) => {
                let tree = crate::workspace::file_tree_json(&dir);
                ws_tool_result(
                    "CreateDirectory",
                    &canonical,
                    &format!("Created directory: {path}"),
                    &tree,
                )
            }
            Err(e) => tool_result("workspace", &format!("CreateDirectory failed: {e}")),
        },
        Err(e) => tool_result("workspace", &e),
    }
}

fn ws_delete_directory(user_id: i64, ws_id: &str, path: &str) -> String {
    if !path.trim().is_empty() {
        return tool_result(
            "workspace",
            &format!(
                "Approval required: deleting directory '{path}' requires user approval. Ask the user before running DeleteDirectory."
            ),
        );
    }
    match resolve_workspace(user_id, ws_id) {
        Ok((dir, canonical)) => match crate::workspace::delete_directory(&dir, path) {
            Ok(()) => {
                let tree = crate::workspace::file_tree_json(&dir);
                ws_tool_result(
                    "DeleteDirectory",
                    &canonical,
                    &format!("Deleted directory: {path}"),
                    &tree,
                )
            }
            Err(e) => tool_result("workspace", &format!("DeleteDirectory failed: {e}")),
        },
        Err(e) => tool_result("workspace", &e),
    }
}

fn vultr_tool_result(action: &str, body: &str) -> String {
    format!(
        "<tool_result tool=\"vultr\" action=\"{}\">{}</tool_result>",
        escape_xml(action),
        escape_xml(&redact_sensitive(body)),
    )
}

fn vultr_list_instances(user_id: i64) -> String {
    match crate::vultr::tool_list_instances(user_id) {
        Ok(instances) => {
            let mut out = String::from("Vultr instances:\n");
            for instance in instances {
                out.push_str(&format!(
                    "- {} | {} | {} | {} | {} | ssh={}\n",
                    instance.id,
                    if instance.label.trim().is_empty() {
                        "(unlabeled)"
                    } else {
                        instance.label.as_str()
                    },
                    instance.main_ip,
                    instance.power_status,
                    instance.server_status,
                    instance.ssh_state,
                ));
            }
            vultr_tool_result("VultrListInstances", out.trim_end())
        }
        Err(error) => vultr_tool_result("VultrListInstances", &crate::vultr::format_tool_error(error)),
    }
}

fn vultr_get_instance(user_id: i64, instance_id: &str) -> String {
    match crate::vultr::tool_get_instance(user_id, instance_id) {
        Ok(instance) => vultr_tool_result(
            "VultrGetInstance",
            &format!(
                "Instance {}\nLabel: {}\nIP: {}\nRegion: {}\nPlan: {}\nOS: {}\nPower: {}\nServer: {}\nSSH: {}",
                instance.id,
                instance.label,
                instance.main_ip,
                instance.region,
                instance.plan,
                instance.os,
                instance.power_status,
                instance.server_status,
                instance.ssh_state,
            ),
        ),
        Err(error) => vultr_tool_result("VultrGetInstance", &crate::vultr::format_tool_error(error)),
    }
}

fn vultr_deploy_instance(user_id: i64, attrs: &HashMap<String, String>) -> String {
    let ssh_key_ids = attrs
        .get("ssh_key_ids")
        .map(|value| {
            value
                .split(',')
                .map(|part| part.trim().to_string())
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let input = crate::vultr::DeployInstanceInput {
        label: attrs.get("label").cloned().unwrap_or_default(),
        region: attrs.get("region").cloned().unwrap_or_default(),
        plan: attrs.get("plan").cloned().unwrap_or_default(),
        os_id: attrs
            .get("os_id")
            .or_else(|| attrs.get("image_id"))
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0),
        ssh_key_ids,
    };
    match crate::vultr::tool_deploy_instance(user_id, input) {
        Ok(instance) => vultr_tool_result(
            "VultrDeployInstance",
            &format!(
                "Deployed instance {}\nLabel: {}\nIP: {}\nRegion: {}\nPlan: {}\nOS: {}",
                instance.id, instance.label, instance.main_ip, instance.region, instance.plan, instance.os
            ),
        ),
        Err(error) => vultr_tool_result("VultrDeployInstance", &crate::vultr::format_tool_error(error)),
    }
}

fn vultr_power_action(user_id: i64, instance_id: &str, action: &str) -> String {
    match crate::vultr::tool_power_action(user_id, instance_id, action) {
        Ok(()) => vultr_tool_result(
            "VultrPowerAction",
            &format!("Power action '{}' submitted for instance '{}'.", action, instance_id),
        ),
        Err(error) => vultr_tool_result("VultrPowerAction", &crate::vultr::format_tool_error(error)),
    }
}

fn vultr_list_regions(user_id: i64) -> String {
    match crate::vultr::tool_list_regions(user_id) {
        Ok(entries) => {
            let body = entries
                .into_iter()
                .map(|entry| format!("- {} | {}", entry.id, entry.description))
                .collect::<Vec<_>>()
                .join("\n");
            vultr_tool_result("VultrListRegions", &format!("Vultr regions:\n{body}"))
        }
        Err(error) => vultr_tool_result("VultrListRegions", &crate::vultr::format_tool_error(error)),
    }
}

fn vultr_list_plans(user_id: i64) -> String {
    match crate::vultr::tool_list_plans(user_id) {
        Ok(entries) => {
            let body = entries
                .into_iter()
                .map(|entry| format!("- {} | {}", entry.id, entry.description))
                .collect::<Vec<_>>()
                .join("\n");
            vultr_tool_result("VultrListPlans", &format!("Vultr plans:\n{body}"))
        }
        Err(error) => vultr_tool_result("VultrListPlans", &crate::vultr::format_tool_error(error)),
    }
}

fn vultr_list_os(user_id: i64) -> String {
    match crate::vultr::tool_list_os(user_id) {
        Ok(entries) => {
            let body = entries
                .into_iter()
                .map(|entry| format!("- {} | {} | {}", entry.id, entry.label, entry.description))
                .collect::<Vec<_>>()
                .join("\n");
            vultr_tool_result("VultrListOs", &format!("Vultr OS images:\n{body}"))
        }
        Err(error) => vultr_tool_result("VultrListOs", &crate::vultr::format_tool_error(error)),
    }
}

fn vultr_list_ssh_keys(user_id: i64) -> String {
    match crate::vultr::tool_list_ssh_keys(user_id) {
        Ok(entries) => {
            let body = entries
                .into_iter()
                .map(|entry| format!("- {} | {}", entry.id, entry.label))
                .collect::<Vec<_>>()
                .join("\n");
            vultr_tool_result("VultrListSshKeys", &format!("Vultr SSH keys:\n{body}"))
        }
        Err(error) => vultr_tool_result("VultrListSshKeys", &crate::vultr::format_tool_error(error)),
    }
}

fn vultr_import_ssh_key(user_id: i64, name: &str, public_key: &str) -> String {
    match crate::vultr::tool_import_ssh_key(user_id, name, public_key) {
        Ok(entry) => vultr_tool_result(
            "VultrImportSshKey",
            &format!("Imported SSH key {} ({})", entry.label, entry.id),
        ),
        Err(error) => vultr_tool_result("VultrImportSshKey", &crate::vultr::format_tool_error(error)),
    }
}

fn vultr_check_access(user_id: i64, instance_id: &str, verify: bool) -> String {
    match crate::vultr::tool_check_access(user_id, instance_id, verify) {
        Ok(profile) => vultr_tool_result(
            "VultrCheckAccess",
            &format!(
                "Access profile for {}\nHost: {}:{}\nUser: {}\nAuth: {}\nState: {}",
                profile.instance_id,
                profile.host,
                profile.port,
                profile.username,
                profile.auth_mode,
                profile.ssh_state,
            ),
        ),
        Err(error) => vultr_tool_result("VultrCheckAccess", &crate::vultr::format_tool_error(error)),
    }
}

fn remote_command(user_id: i64, instance_id: &str, command: &str, timeout: u64) -> String {
    match crate::vultr::tool_remote_command(user_id, instance_id, command, timeout) {
        Ok(result) => vultr_tool_result(
            "RemoteCommand",
            &format!(
                "$ ssh {}@{}:{} -- {}\n{}\nExit: {} ({}ms{})",
                result.username,
                result.host,
                result.port,
                redact_sensitive(command),
                result.output,
                result.exit_code,
                result.duration_ms,
                if result.timed_out { ", timed out" } else { "" },
            ),
        ),
        Err(error) => vultr_tool_result("RemoteCommand", &crate::vultr::format_tool_error(error)),
    }
}

fn format_tree_text(dir: &std::path::Path) -> String {
    let entries = crate::workspace::file_tree(dir, dir, 0);
    fn fmt(entries: &[crate::workspace::FileEntry], prefix: &str) -> String {
        let mut out = String::new();
        for (i, e) in entries.iter().enumerate() {
            let is_last = i + 1 == entries.len();
            let branch = if is_last { "└── " } else { "├── " };
            let icon = if e.is_dir { "/" } else { "" };
            out.push_str(&format!("{prefix}{branch}{}{icon}\n", e.name));
            if let Some(children) = &e.children {
                let next = format!("{}{}", prefix, if is_last { "    " } else { "│   " });
                out.push_str(&fmt(children, &next));
            }
        }
        out
    }
    fmt(&entries, "")
}

fn uuid_hex() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    format!("{nanos:08x}")
}

fn run_dothink(
    attrs: &HashMap<String, String>,
    body: &str,
    transcript: &str,
    model: &str,
) -> String {
    let tellmyself = attrs
        .get("tellmyself")
        .map(String::as_str)
        .unwrap_or_default()
        .trim();
    let instruction = if !tellmyself.is_empty() {
        tellmyself
    } else {
        body.trim()
    };

    let prompt = if instruction.is_empty() {
        "Think through the latest user request carefully and provide the best answer."
    } else {
        instruction
    };

    let messages = vec![ChatTurn {
        role: "user".into(),
        content: format!(
            "Conversation transcript:\n{transcript}\n\nDedicated thinking instruction:\n{prompt}"
        ),
    }];

    match ai::complete_chat_with_system_prompt(model.to_string(), DOTHINK_SYSTEM_PROMPT, messages) {
        Ok(content) if !content.trim().is_empty() => truncate(content.trim(), MAX_TOOL_OUTPUT),
        Ok(_) => "#### final answer\nThe ultra-think step returned no content.".into(),
        Err(error) => tool_result("dothink", &format!("Ultra think failed: {error}")),
    }
}

fn run_query(pattern: &str, transcript: &str) -> String {
    if pattern.trim().is_empty() {
        return "No regex was provided.".into();
    }

    let regex = match Regex::new(pattern) {
        Ok(regex) => regex,
        Err(error) => return format!("Invalid regex: {error}"),
    };

    let matches = transcript
        .lines()
        .filter(|line| regex.is_match(line))
        .take(20)
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>();

    if matches.is_empty() {
        "No matching conversation lines.".into()
    } else {
        matches.join("\n")
    }
}

fn run_python(code: &str) -> String {
    let mut child = match Command::new("python3")
        .arg("-I")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => return format!("Python failed to start: {error}"),
    };

    if let Some(mut stdin) = child.stdin.take() {
        if let Err(error) = stdin.write_all(code.as_bytes()) {
            return format!("Python stdin failed: {error}");
        }
    }

    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() >= PYTHON_TIMEOUT => {
                let _ = child.kill();
                return "Python execution timed out.".into();
            }
            Ok(None) => thread::sleep(Duration::from_millis(25)),
            Err(error) => return format!("Python wait failed: {error}"),
        }
    }

    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(error) => return format!("Python output failed: {error}"),
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut result = String::new();
    if !stdout.trim().is_empty() {
        result.push_str(stdout.trim_end());
    }
    if !stderr.trim().is_empty() {
        if !result.is_empty() {
            result.push_str("\n\n");
        }
        result.push_str("stderr:\n");
        result.push_str(stderr.trim_end());
    }
    if result.trim().is_empty() {
        result.push_str("(python produced no output)");
    }
    truncate(&result, MAX_TOOL_OUTPUT)
}

fn run_search(query: &str, model: &str) -> String {
    let query = query.trim();
    if query.is_empty() {
        return tool_result("search", "No search query was provided.");
    }

    let searxng_url = std::env::var("SEARXNG_URL")
        .ok()
        .filter(|k| !k.trim().is_empty());
    let brave_key = std::env::var("BRAVE_SEARCH_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty());
    let google_key = std::env::var("GOOGLE_SEARCH_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty());
    let google_cx = std::env::var("GOOGLE_SEARCH_ENGINE_ID")
        .ok()
        .filter(|k| !k.trim().is_empty());

    let fetch_result = if let Some(base) = searxng_url {
        fetch_searxng(query, &base)
    } else if let Some(key) = brave_key {
        fetch_brave(query, &key)
    } else if let (Some(key), Some(cx)) = (google_key, google_cx) {
        fetch_google(query, &key, &cx)
    } else {
        return tool_result(
            "search",
            "No search provider configured. Set SEARXNG_URL, BRAVE_SEARCH_API_KEY, \
             or GOOGLE_SEARCH_API_KEY + GOOGLE_SEARCH_ENGINE_ID in .env.",
        );
    };

    let results = match fetch_result {
        Err(e) => return tool_result("search", &e),
        Ok(r) if r.is_empty() => return tool_result("search", "No results found for this query."),
        Ok(r) => r,
    };

    let results_text = results.join("\n\n");
    let today = current_date();

    let synthesis_prompt = format!(
        "Today's date is {today}. Keep this in mind when evaluating dates in search results — \
         web pages are often cached or refer to past events, so a result saying \"2025\" may \
         actually be referring to something that happened in 2026. Cross-check dates against \
         what makes sense given today's date.\n\n\
         Web search results for \"{query}\":\n\n{results_text}\n\n\
         Based on these results, give a concise, accurate answer to: {query}\n\
         Cite sources inline using the result titles or URLs. \
         If the results don't contain enough info, say so.",
    );

    let messages = vec![ai::ChatTurn {
        role: "user".into(),
        content: synthesis_prompt,
    }];

    match ai::complete_chat_with_system_prompt(model.to_string(), SEARCH_SYSTEM_PROMPT, messages) {
        Ok(raw) if !raw.trim().is_empty() => {
            let answer = strip_think_tags(raw.trim());
            // Compact source list for the collapsed reference block
            let sources = results
                .iter()
                .filter_map(|r| {
                    let mut lines = r.lines();
                    let header = lines.next()?.trim().to_string();
                    let url = lines.next().unwrap_or("").trim().to_string();
                    Some(if url.is_empty() {
                        header
                    } else {
                        format!("{header}\n{url}")
                    })
                })
                .collect::<Vec<_>>()
                .join("\n\n");
            // Collapsed block holds sources; polished answer flows as normal text after
            format!("{}\n\n{}", search_tool_result(query, &sources), answer,)
        }
        Ok(_) => tool_result(
            "search",
            &format!("Synthesis failed.\n\nRaw results:\n{results_text}"),
        ),
        Err(e) => tool_result(
            "search",
            &format!("Synthesis failed: {e}\n\nRaw results:\n{results_text}"),
        ),
    }
}

fn search_tool_result(query: &str, sources: &str) -> String {
    format!(
        "<tool_result tool=\"search\" query=\"{}\">{}</tool_result>",
        escape_xml(query),
        escape_xml(&truncate(sources, MAX_TOOL_OUTPUT)),
    )
}

fn run_fetch_url(url: &str) -> String {
    let url = url.trim();
    if url.is_empty() {
        return tool_result("fetchUrl", "No URL was provided.");
    }
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return tool_result("fetchUrl", "Only http:// and https:// URLs are supported.");
    }

    let response = match ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(12))
        .user_agent("MixerEnterpriseAgent/1.0")
        .build()
        .get(url)
        .call()
    {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            let final_url = response.get_url().to_string();
            let body = read_response_text(response);
            return format_fetch_url_result(url, Some(code), &final_url, body);
        }
        Err(error) => return tool_result("fetchUrl", &format!("URL fetch failed: {error}")),
    };

    let status = response.status();
    let final_url = response.get_url().to_string();
    format_fetch_url_result(url, Some(status), &final_url, read_response_text(response))
}

fn read_response_text(response: ureq::Response) -> String {
    match response.into_string() {
        Ok(text) => readable_page_text(&text),
        Err(error) => format!("Could not read response body: {error}"),
    }
}

fn format_fetch_url_result(
    requested_url: &str,
    status: Option<u16>,
    final_url: &str,
    body: String,
) -> String {
    let status_line = status
        .map(|code| code.to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let content = format!(
        "Requested URL: {requested_url}\nFinal URL: {final_url}\nHTTP status: {status_line}\n\n{}",
        truncate(&body, MAX_TOOL_OUTPUT),
    );
    format!(
        "<tool_result tool=\"fetchUrl\" url=\"{}\">{}</tool_result>",
        escape_xml(requested_url),
        escape_xml(&content),
    )
}

fn readable_page_text(raw: &str) -> String {
    let mut text = raw.to_string();
    for pattern in [
        r"(?is)<script[^>]*>.*?</script>",
        r"(?is)<style[^>]*>.*?</style>",
        r"(?is)<noscript[^>]*>.*?</noscript>",
        r"(?is)<!--.*?-->",
    ] {
        if let Ok(re) = Regex::new(pattern) {
            text = re.replace_all(&text, " ").into_owned();
        }
    }
    if let Ok(re) = Regex::new(r"(?is)<[^>]+>") {
        text = re.replace_all(&text, " ").into_owned();
    }
    decode_html_entities(&text)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn decode_html_entities(input: &str) -> String {
    input
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
}

// ── Todo tool ────────────────────────────────────────────────────────────────

struct TodoList {
    tasks: Vec<(usize, bool, String)>, // (number, done, text)
}

impl TodoList {
    fn complete(&mut self, num: usize) -> bool {
        for (n, done, _) in &mut self.tasks {
            if *n == num {
                *done = true;
                return true;
            }
        }
        false
    }

    fn format(&self) -> String {
        let mut out = String::from("TODO:\n");
        for (num, done, text) in &self.tasks {
            let marker = if *done { "[x]" } else { "[ ]" };
            out.push_str(&format!("{marker} {num}. {text}\n"));
        }
        let total = self.tasks.len();
        let done_count = self.tasks.iter().filter(|(_, d, _)| *d).count();
        out.push_str(&format!("\n{done_count}/{total} done"));
        out
    }
}

fn parse_todo_content(content: &str) -> TodoList {
    let mut tasks = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        let (done, rest) = if let Some(r) = line.strip_prefix("[x] ") {
            (true, r)
        } else if let Some(r) = line.strip_prefix("[ ] ") {
            (false, r)
        } else {
            continue;
        };
        if let Some(dot) = rest.find(". ") {
            if let Ok(num) = rest[..dot].parse::<usize>() {
                tasks.push((num, done, rest[dot + 2..].to_string()));
                continue;
            }
        }
        tasks.push((tasks.len() + 1, done, rest.to_string()));
    }
    TodoList { tasks }
}

fn find_current_todo(transcript: &str) -> Option<TodoList> {
    let mut last: Option<&str> = None;
    let mut cursor = 0;
    while cursor < transcript.len() {
        let Some(rel) = transcript[cursor..].find("<tool_result") else {
            break;
        };
        let abs = cursor + rel;
        let Some(tag_end_rel) = transcript[abs..].find('>') else {
            break;
        };
        let tag_end = abs + tag_end_rel;
        let header = &transcript[abs..=tag_end];
        let is_todo = header.contains("createTodo") || header.contains("completeTodo");
        let content_start = tag_end + 1;
        if let Some(close_rel) = transcript[content_start..].find("</tool_result>") {
            if is_todo {
                last = Some(&transcript[content_start..content_start + close_rel]);
            }
            cursor = content_start + close_rel + "</tool_result>".len();
        } else {
            break;
        }
    }
    last.map(parse_todo_content)
}

fn run_create_todo(body: &str) -> String {
    let tasks: Vec<String> = body
        .lines()
        .map(str::trim)
        // Skip blank lines and lines that look like XML tags — the model sometimes
        // generates wrong closing tags (e.g. </create>) or embeds other tool calls
        // inside the todo body when output is truncated mid-generation.
        .filter(|l| !l.is_empty() && !l.starts_with('<'))
        .enumerate()
        .map(|(i, line)| {
            // Strip leading "1." / "1:" / "1)" numbering the model may include
            let text = line
                .trim_start_matches(|c: char| c.is_ascii_digit())
                .trim_start_matches(|c: char| matches!(c, '.' | ':' | ')'))
                .trim();
            format!("[ ] {}. {}", i + 1, text)
        })
        .collect();

    if tasks.is_empty() {
        return tool_result("createTodo", "No tasks provided.");
    }

    tool_result("createTodo", &format!("TODO:\n{}", tasks.join("\n")))
}

fn run_complete_todo(task_str: &str, transcript: &str) -> String {
    let num: usize = match task_str.trim().parse() {
        Ok(n) => n,
        Err(_) => {
            return tool_result(
                "completeTodo",
                &format!("Invalid task number: '{task_str}'"),
            )
        }
    };

    let mut todo = match find_current_todo(transcript) {
        Some(t) => t,
        None => {
            return tool_result(
                "completeTodo",
                "No active todo list found. Use createTodo first.",
            )
        }
    };

    if !todo.complete(num) {
        return tool_result(
            "completeTodo",
            &format!("Task {num} not found in todo list."),
        );
    }

    tool_result("completeTodo", &todo.format())
}

fn run_view_todo(transcript: &str) -> String {
    match find_current_todo(transcript) {
        Some(todo) => tool_result("viewTodo", &todo.format()),
        None => tool_result("viewTodo", "No active todo list."),
    }
}

fn strip_think_tags(text: &str) -> String {
    let mut out = text.to_string();
    for tag in &["think", "thinking"] {
        let open = format!("<{tag}>");
        let close = format!("</{tag}>");
        loop {
            if let Some(start) = out.find(&open) {
                let end = out[start..]
                    .find(&close)
                    .map(|i| start + i + close.len())
                    .unwrap_or(start + open.len());
                out.drain(start..end);
            } else {
                break;
            }
        }
    }
    out.trim().to_string()
}

fn current_date() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let z = secs / 86400 + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y}-{m:02}-{d:02}")
}

fn fetch_searxng(query: &str, base_url: &str) -> Result<Vec<String>, String> {
    let base = base_url.trim_end_matches('/');
    let url = format!(
        "{}{SEARXNG_PATH}?q={}&format=json&categories=general&language=en",
        base,
        percent_encode(query),
    );

    let json: serde_json::Value = ureq::get(&url)
        .set("Accept", "application/json")
        .set("User-Agent", "Mozilla/5.0 (compatible; Mixer-Chat/1.0)")
        .call()
        .map_err(|e| format!("SearXNG search failed: {e}"))?
        .into_json()
        .map_err(|e| format!("SearXNG response parse failed: {e}"))?;

    let mut parts: Vec<String> = Vec::new();

    if let Some(answers) = json["answers"].as_array() {
        for answer in answers.iter().take(2) {
            if let Some(text) = answer.as_str().filter(|s| !s.is_empty()) {
                parts.push(format!("[Direct Answer] {text}"));
            }
        }
    }

    if let Some(boxes) = json["infoboxes"].as_array() {
        if let Some(ib) = boxes.first() {
            if let Some(content) = ib["content"].as_str().filter(|s| !s.is_empty()) {
                let title = ib["infobox"].as_str().unwrap_or("");
                let src = ib["urls"]
                    .as_array()
                    .and_then(|u| u.first())
                    .and_then(|u| u["url"].as_str())
                    .unwrap_or("");
                parts.push(format!("[{title}] {content}\n{src}"));
            }
        }
    }

    if let Some(results) = json["results"].as_array() {
        for (i, r) in results.iter().enumerate().take(SEARCH_RESULT_COUNT) {
            let title = r["title"].as_str().unwrap_or("");
            let href = r["url"].as_str().unwrap_or("");
            let content = r["content"].as_str().unwrap_or("");
            if !title.is_empty() || !content.is_empty() {
                parts.push(format!(
                    "[Source {}] {}\n{}\n{}",
                    i + 1,
                    title,
                    href,
                    content
                ));
            }
        }
    }

    Ok(parts)
}

fn fetch_google(query: &str, api_key: &str, cx: &str) -> Result<Vec<String>, String> {
    let url = format!(
        "{}?key={}&cx={}&q={}&num={}",
        GOOGLE_SEARCH_URL,
        api_key.trim(),
        cx.trim(),
        percent_encode(query),
        SEARCH_RESULT_COUNT,
    );

    let json: serde_json::Value = ureq::get(&url)
        .set("Accept", "application/json")
        .call()
        .map_err(|e| format!("Google search failed: {e}"))?
        .into_json()
        .map_err(|e| format!("Google response parse failed: {e}"))?;

    Ok(json["items"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .enumerate()
                .take(SEARCH_RESULT_COUNT)
                .filter_map(|(i, r)| {
                    let title = r["title"].as_str()?;
                    let link = r["link"].as_str()?;
                    let snippet = r["snippet"].as_str().unwrap_or("");
                    Some(format!(
                        "[Source {}] {}\n{}\n{}",
                        i + 1,
                        title,
                        link,
                        snippet
                    ))
                })
                .collect()
        })
        .unwrap_or_default())
}

fn fetch_brave(query: &str, api_key: &str) -> Result<Vec<String>, String> {
    let url = format!(
        "{}?q={}&count={}&text_decorations=false&extra_snippets=true",
        BRAVE_SEARCH_URL,
        percent_encode(query),
        SEARCH_RESULT_COUNT,
    );

    let json: serde_json::Value = ureq::get(&url)
        .set("X-Subscription-Token", api_key.trim())
        .set("Accept", "application/json")
        .call()
        .map_err(|e| format!("Brave search failed: {e}"))?
        .into_json()
        .map_err(|e| format!("Brave response parse failed: {e}"))?;

    Ok(json["web"]["results"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .enumerate()
                .take(SEARCH_RESULT_COUNT)
                .filter_map(|(i, r)| {
                    let title = r["title"].as_str()?;
                    let url = r["url"].as_str()?;
                    let desc = r["description"].as_str().unwrap_or("");
                    let extra = r["extra_snippets"]
                        .as_array()
                        .and_then(|s| s.first())
                        .and_then(|s| s.as_str())
                        .unwrap_or("");
                    let snippet = if extra.is_empty() { desc } else { extra };
                    Some(format!(
                        "[Source {}] {}\n{}\n{}",
                        i + 1,
                        title,
                        url,
                        snippet
                    ))
                })
                .collect()
        })
        .unwrap_or_default())
}

fn percent_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn tool_result(tool: &str, content: &str) -> String {
    format!(
        "<tool_result tool=\"{}\">{}</tool_result>",
        escape_xml(tool),
        escape_xml(&truncate(&redact_sensitive(content), MAX_TOOL_OUTPUT))
    )
}

fn incomplete_tool_result(tool: &str, body: &str) -> String {
    match tool {
        "response" => body.trim().to_string(),
        "code" => rebuild_tool("code", HashMap::new(), body),
        "html" => tool_result(
            "html",
            "The HTML tool call was incomplete and was not rendered.",
        ),
        "dothink" => tool_result(
            "dothink",
            "The dothink tool call was incomplete and was not executed.",
        ),
        "done" => tool_result("done", "The done tool call was incomplete."),
        _ => tool_result(
            tool,
            &format!("The {tool} tool call was incomplete, so it was not executed."),
        ),
    }
}

fn rebuild_tool(name: &str, attrs: HashMap<String, String>, body: &str) -> String {
    let mut rebuilt = String::new();
    rebuilt.push('<');
    rebuilt.push_str(name);
    for (key, value) in attrs {
        rebuilt.push(' ');
        rebuilt.push_str(&key);
        rebuilt.push_str("=\"");
        rebuilt.push_str(&escape_xml(&value));
        rebuilt.push('"');
    }
    rebuilt.push('>');
    rebuilt.push_str(body);
    rebuilt.push_str("</");
    rebuilt.push_str(name);
    rebuilt.push('>');
    rebuilt
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn unescape_xml(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&gt;", ">")
        .replace("&lt;", "<")
        .replace("&amp;", "&")
}

fn truncate(value: &str, max_chars: usize) -> String {
    let mut out = value.chars().take(max_chars).collect::<String>();
    if value.chars().count() > max_chars {
        out.push_str("\n...[truncated]");
    }
    out
}
