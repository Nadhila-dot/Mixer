use crate::{
    ai::{self, ChatTurn, StreamEvent},
    db::{self, AgentRun, ToolCall},
    tools,
};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
    sync::{mpsc::Sender, Mutex, OnceLock},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub const MAX_ITERS: usize = 24;
pub const MAX_RETRIES: usize = 3;
pub const MAX_TOOL_CALLS_PER_RUN: usize = 120;
pub const LOOP_DETECT_THRESHOLD: usize = 3;
const MAX_INCOMPLETE_RETRIES: usize = 10;
const MAX_FILE_TOOL_CHARS: usize = 96_000;
const CONTINUATION_SYSTEM_PROMPT: &str = r#"You are a raw text continuation engine.

You will receive an assistant message that ended inside an XML tool call.
Return only the exact remaining bytes needed to finish that same tool body and its matching closing tag.

Rules:
- Do not start a new tool call.
- Do not output <think> blocks, JSON, markdown fences, commentary, apologies, or explanations.
- Do not repeat content already present in the unfinished tool call.
- Continue from the next character only.
- End with the matching closing XML tag."#;

/// Builds the system prompt for an agent turn, prepending a runtime block that
/// reminds the model about workspaces it already created in this chat so it
/// reuses them instead of calling CreateWorkspace again (which creates a fresh
/// empty workspace and silently loses the prior work).
fn build_system_prompt(ctx: &RunContext, turns: &[ChatTurn]) -> String {
    let runtime = runtime_context_prompt();
    let personalization = personalization_block(ctx.user_id);
    let workspaces = collect_chat_workspaces(ctx, turns);
    if workspaces.is_empty() {
        return format!("{runtime}{personalization}{}", crate::prompt::SYSTEM_PROMPT);
    }

    let mut header = String::with_capacity(800);
    header.push_str("## ⚠️ ACTIVE WORKSPACES IN THIS CHAT — READ BEFORE ANY ACTION\n\n");
    header.push_str(
        "You have ALREADY created the following workspace(s) earlier in this conversation. \
         If the current request is a follow-up on that project, DO NOT call <CreateWorkspace> again — doing so would spin up a fresh EMPTY workspace and \
         silently lose every file you've already built. If the current request is unrelated research, a status check, writing, planning, or Q&A, ignore these workspace IDs and answer without workspace tools.\n\n",
    );
    header
        .push_str("Existing workspace_id(s) in this chat (in creation order, most recent last):\n");
    for (i, ws_id) in workspaces.iter().enumerate() {
        header.push_str(&format!("  {}. workspace_id=\"{}\"\n", i + 1, ws_id));
    }

    let default_ws = workspaces.last().expect("non-empty checked above");
    if workspaces.len() == 1 {
        header.push_str(&format!(
            "\nFor follow-up work on this existing project, pass exactly `id=\"{}\"` in every workspace tool call \
             (CreateFile, AppendFile, PatchFile, ReadFile, Command, LongRunProcess, Preview, etc.). For unrelated non-workspace tasks, do not call workspace tools.\n",
            default_ws
        ));
    } else {
        header.push_str(&format!(
            "\nFor follow-up project work, default to the most recent workspace: pass `id=\"{}\"` unless the user explicitly \
             references one of the earlier ones by name or id. For unrelated non-workspace tasks, do not call workspace tools.\n",
            default_ws
        ));
    }

    header.push_str(
        "\nOnly call <CreateWorkspace> if the user has EXPLICITLY asked you to start a brand-new \
         separate project (e.g. \"start a fresh workspace\", \"new project\", \"different repo\"). \
         Otherwise: REUSE the existing workspace_id verbatim — never invent a shortened name like \"mixer\", \
         always use the full id with its suffix.\n\n",
    );
    header.push_str("─────────────────────────────────────────────\n\n");

    let mut out = String::with_capacity(
        runtime.len() + personalization.len() + header.len() + crate::prompt::SYSTEM_PROMPT.len(),
    );
    out.push_str(&runtime);
    out.push_str(&personalization);
    out.push_str(&header);
    out.push_str(crate::prompt::SYSTEM_PROMPT);
    out
}

/// Loads the user's personalization (base style + characteristics + custom
/// instructions) from the DB and renders it as a short system-prompt block.
/// Returns an empty string if the user has no preferences set, so we don't
/// pollute the prompt for fresh users.
fn personalization_block(user_id: i64) -> String {
    let Ok(settings) = db::auth_store().get_user_settings(user_id) else {
        return String::new();
    };

    let characteristics: serde_json::Value =
        serde_json::from_str(&settings.characteristics_json).unwrap_or_else(|_| json!({}));

    let has_style = settings.base_style != "default" && !settings.base_style.is_empty();
    let custom = settings.custom_instructions.trim();
    let has_custom = !custom.is_empty();
    let active_chars: Vec<(String, String)> = characteristics
        .as_object()
        .map(|map| {
            map.iter()
                .filter_map(|(k, v)| {
                    let value = v.as_str().unwrap_or("").trim().to_lowercase();
                    if value.is_empty() || value == "default" {
                        return None;
                    }
                    Some((k.clone(), value))
                })
                .collect()
        })
        .unwrap_or_default();

    if !has_style && active_chars.is_empty() && !has_custom {
        return String::new();
    }

    let mut block = String::with_capacity(400);
    block.push_str("## USER PERSONALIZATION\n");
    block.push_str(
        "Honor these style preferences when composing replies. They affect tone and \
         formatting only — never let them change correctness, refuse legitimate requests, \
         or override safety:\n",
    );

    if has_style {
        block.push_str(&format!(
            "- Base style/tone: {} — match this overall register.\n",
            describe_base_style(&settings.base_style)
        ));
    }

    for (key, value) in &active_chars {
        let label = describe_characteristic(key);
        let direction = match value.as_str() {
            "more" | "high" => "lean noticeably toward more",
            "less" | "low" => "use noticeably less",
            other => other,
        };
        block.push_str(&format!("- {label}: {direction}.\n"));
    }

    if has_custom {
        block.push_str("- Additional instructions from the user:\n  > ");
        // Indent multi-line custom text with the same gutter.
        let indented = custom.replace('\n', "\n  > ");
        block.push_str(&indented);
        block.push('\n');
    }

    block.push_str("─────────────────────────────────────────────\n\n");
    block
}

fn describe_base_style(style: &str) -> &str {
    match style {
        "concise" => "concise — short, direct sentences; minimal padding",
        "detailed" => "detailed — thorough explanations with context and examples",
        "friendly" => "friendly — warm, conversational, approachable",
        "professional" => "professional — polished, neutral, business-register",
        "witty" => "witty — light wordplay and dry humor where appropriate",
        "direct" => "direct — blunt and to the point; no hedging",
        _ => "default — balanced and adaptive",
    }
}

fn describe_characteristic(key: &str) -> &str {
    match key {
        "warmth" | "warm" => "Warmth",
        "enthusiasm" | "enthusiastic" => "Enthusiasm",
        "headers_lists" | "headers" | "lists" => "Headers & lists",
        "emoji" | "emojis" => "Emoji",
        "verbosity" => "Verbosity",
        "technicality" | "technical" => "Technical depth",
        other => other,
    }
}

fn runtime_context_prompt() -> String {
    let now = chrono::Local::now();
    let timezone = std::env::var("TZ").unwrap_or_else(|_| now.format("%Z").to_string());
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "unknown".to_string());
    let cwd = std::env::current_dir()
        .ok()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    let sandbox = "workspace cwd execution with network access, package-manager access, secret redaction, and approval gates for destructive local actions";

    format!(
        "## RUNTIME CONTEXT\n\
         - OS: {} {}\n\
         - Architecture: {}\n\
         - Current date/time: {}\n\
         - Timezone: {}\n\
         - Shell: {}\n\
         - Server working directory: {}\n\
         - Workspace sandbox: {}\n\n",
        std::env::consts::OS,
        std::env::consts::FAMILY,
        std::env::consts::ARCH,
        now.format("%Y-%m-%d %H:%M:%S %Z"),
        timezone,
        shell,
        cwd,
        sandbox,
    )
}

/// Returns the workspace_ids discovered in prior tool results in this chat,
/// in first-seen (chronological) order, deduplicated.
fn collect_chat_workspaces(ctx: &RunContext, turns: &[ChatTurn]) -> Vec<String> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut order: Vec<String> = Vec::new();
    for ws_id in collect_persisted_chat_workspaces(ctx.user_id, &ctx.chat_uuid) {
        if seen.insert(ws_id.clone()) {
            order.push(ws_id);
        }
    }
    for turn in turns {
        for ws_id in extract_workspace_ids(&turn.content) {
            if seen.insert(ws_id.clone()) {
                order.push(ws_id);
            }
        }
    }
    order
}

fn collect_persisted_chat_workspaces(user_id: i64, chat_uuid: &str) -> Vec<String> {
    let store = db::auth_store();
    let Ok(runs) = store.runs_for_chat(user_id, chat_uuid) else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    let mut order = Vec::new();
    for run in runs {
        let Ok(calls) = store.tool_calls_for_run(&run.uuid) else {
            continue;
        };
        for call in calls {
            if call.name != "CreateWorkspace" {
                continue;
            }
            for source in [
                call.output.as_deref().unwrap_or_default(),
                call.error.as_deref().unwrap_or_default(),
                call.body.as_str(),
                call.args_json.as_str(),
            ] {
                for ws_id in extract_workspace_ids(source) {
                    if seen.insert(ws_id.clone()) {
                        order.push(ws_id);
                    }
                }
            }
        }
    }
    order
}

fn extract_workspace_ids(content: &str) -> Vec<String> {
    let mut results = Vec::new();
    for needle in ["workspace_id=\"", "\"workspace_id\":\"", "workspace_id:"] {
        let mut cursor = 0usize;
        while cursor < content.len() {
            let Some(rel) = content[cursor..].find(needle) else {
                break;
            };
            let mut abs = cursor + rel + needle.len();
            while content[abs..].starts_with(' ') {
                abs += 1;
            }
            let end = if needle.ends_with('"') {
                content[abs..].find('"').map(|end_rel| abs + end_rel)
            } else {
                Some(
                    content[abs..]
                        .find(|c: char| c.is_whitespace() || c == ',' || c == '<' || c == '&')
                        .map(|end_rel| abs + end_rel)
                        .unwrap_or(content.len()),
                )
            };
            let Some(end) = end else { break };
            let id = content[abs..end].trim().trim_matches('"');
            if !id.is_empty() {
                results.push(id.to_string());
            }
            cursor = end + 1;
        }
    }
    results
}

fn latest_persisted_workspace(ctx: &RunContext) -> Option<String> {
    collect_persisted_chat_workspaces(ctx.user_id, &ctx.chat_uuid).pop()
}

fn existing_workspace_result(ctx: &RunContext, requested_name: &str) -> String {
    if let Some(ws_id) = latest_persisted_workspace(ctx) {
        return tools::tool_result_xml(
            "workspace",
            &format!(
                "Workspace already exists for this chat. Do not create another workspace. Reuse workspace_id=\"{ws_id}\" for every subsequent workspace tool call. Requested new workspace name was \"{requested_name}\"."
            ),
        );
    }
    tools::tool_result_xml(
        "workspace",
        "No existing workspace was found for this chat.",
    )
}

// ── Live event broadcast registry ────────────────────────────────────────────
// Each run can have N WS subscribers. When the run loop emits an event we
// fan it out to all of them. Subscribers register/unregister via subscribe/drop.

type EventTx = Sender<String>;

fn cancelled_runs() -> &'static Mutex<std::collections::HashSet<String>> {
    static SET: OnceLock<Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    SET.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

fn subscribers() -> &'static Mutex<HashMap<String, Vec<EventTx>>> {
    static MAP: OnceLock<Mutex<HashMap<String, Vec<EventTx>>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn subscribe(run_uuid: &str, tx: EventTx) {
    let mut map = subscribers().lock().unwrap();
    map.entry(run_uuid.to_string()).or_default().push(tx);
}

fn fanout(run_uuid: &str, payload: &str) {
    let map = subscribers().lock().unwrap();
    if let Some(subs) = map.get(run_uuid) {
        for tx in subs {
            let _ = tx.send(payload.to_string());
        }
    }
}

fn emit_run_event(run_uuid: &str, event_type: &str, payload: Value) {
    let now = unix_secs();
    let payload = redact_json_value(payload);
    let payload_str = payload.to_string();
    let seq = next_seq(run_uuid);
    let _ = db::auth_store().insert_event(run_uuid, seq, event_type, &payload_str, now);
    let envelope = json!({
        "seq": seq,
        "type": event_type,
        "payload": payload,
        "created_at": now,
    });
    fanout(run_uuid, &envelope.to_string());
}

pub fn drop_dead_subscribers(run_uuid: &str) {
    let mut map = subscribers().lock().unwrap();
    if let Some(subs) = map.get_mut(run_uuid) {
        subs.retain(|tx| tx.send(String::new()).is_ok());
        if subs.is_empty() {
            map.remove(run_uuid);
        }
    }
}

pub fn cancel_run(user_id: i64, run_uuid: &str) -> Result<(), String> {
    let store = db::auth_store();
    let run = store
        .get_run(user_id, run_uuid)
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "run not found".to_string())?;

    cancelled_runs().lock().unwrap().insert(run.uuid.clone());

    if run.status != "running" {
        return Ok(());
    }

    let now = unix_secs();
    store
        .update_run_status(
            &run.uuid,
            "cancelled",
            None,
            Some("Stopped by user"),
            None,
            Some(now),
        )
        .map_err(|e| e.to_string())?;
    emit_run_event(
        &run.uuid,
        "run_cancelled",
        json!({ "reason": "Stopped by user" }),
    );
    Ok(())
}

// ── Public entry points ──────────────────────────────────────────────────────

/// Spawn the agent loop in a background thread. Returns immediately with the
/// run uuid. Events are persisted to the DB and fanned out to subscribers.
pub fn spawn_run(
    user_id: i64,
    chat_uuid: String,
    user_message_uuid: String,
    transcript: String,
    turns: Vec<ChatTurn>,
    model: String,
) -> String {
    let now = unix_secs();
    let run_uuid = uuid_v4();

    let run = AgentRun {
        uuid: run_uuid.clone(),
        chat_uuid: chat_uuid.clone(),
        user_id,
        user_message_uuid,
        assistant_message_uuid: None,
        model: model.clone(),
        status: "running".into(),
        final_text: None,
        error_summary: None,
        started_at: now,
        ended_at: None,
    };
    if let Err(e) = db::auth_store().create_run(&run) {
        eprintln!("[run/create] FAILED: {e}");
        return run_uuid;
    }

    let run_uuid_thread = run_uuid.clone();
    thread::spawn(move || {
        execute_run(
            user_id,
            chat_uuid,
            run_uuid_thread,
            transcript,
            turns,
            model,
        );
    });

    run_uuid
}

/// Synchronous run executor — durable ReAct loop.
/// The model returns exactly one structured action per turn:
/// either a final answer or one tool call. Tool progress is emitted as
/// persisted events so the UI can resume after refresh.
fn execute_run(
    user_id: i64,
    chat_uuid: String,
    run_uuid: String,
    transcript: String,
    turns: Vec<ChatTurn>,
    model: String,
) {
    let ctx = RunContext::new(run_uuid.clone(), user_id, chat_uuid.clone(), model.clone());

    ctx.emit(
        "run_started",
        json!({
            "run_uuid": run_uuid,
            "model": model,
            "chat_uuid": chat_uuid,
        }),
    );
    if ctx.is_cancelled() {
        return;
    }

    let mut running_transcript = transcript;
    let mut final_text = String::new();
    let mut tool_call_seq: i64 = 0;
    let mut tool_call_count: usize = 0;
    let mut history_key_counts: HashMap<String, usize> = HashMap::new();
    let mut current_turns = turns;
    let mut tail_text = String::new(); // text from the most recent turn that wasn't tool XML
    let mut saw_final = false;

    for iter in 0..MAX_ITERS {
        if ctx.is_cancelled() {
            return;
        }
        ctx.emit("iteration_started", json!({ "iter": iter }));

        let action = match decide_next_action(&ctx, &current_turns) {
            Ok(action) => action,
            Err(e) => {
                if ctx.is_cancelled() {
                    return;
                }
                finalize_failed(&ctx, &format!("AI call failed: {e}"));
                return;
            }
        };
        if ctx.is_cancelled() {
            return;
        }

        match action {
            DecidedAction::Final {
                content,
                assistant_raw,
            } => {
                final_text = content;
                tail_text = final_text.clone();
                saw_final = true;
                emit_text_chunks(&ctx, &final_text);
                if ctx.is_cancelled() {
                    return;
                }
                current_turns.push(ChatTurn {
                    role: "assistant".into(),
                    content: assistant_raw,
                });
                break;
            }
            DecidedAction::Tool {
                parsed,
                assistant_raw,
            } => {
                tail_text.clear();

                tool_call_count += 1;
                if tool_call_count > MAX_TOOL_CALLS_PER_RUN {
                    if ctx.is_cancelled() {
                        return;
                    }
                    ctx.emit("run_failed", json!({
                        "error": format!("Exceeded MAX_TOOL_CALLS_PER_RUN ({MAX_TOOL_CALLS_PER_RUN})")
                    }));
                    finalize_failed(&ctx, "Tool budget exhausted");
                    return;
                }

                // Loop detection
                let history_key = loop_history_key(&parsed);
                let count = history_key_counts.entry(history_key.clone()).or_insert(0);
                *count += 1;
                if *count > LOOP_DETECT_THRESHOLD {
                    if ctx.is_cancelled() {
                        return;
                    }
                    ctx.emit(
                        "run_failed",
                        json!({
                            "error": format!("Loop detected on tool '{}'", parsed.name),
                            "tool": parsed.name,
                        }),
                    );
                    finalize_failed(&ctx, &format!("Loop detected on tool '{}'", parsed.name));
                    return;
                }

                // Validate args
                tool_call_seq += 1;
                let tool_result_xml = if let Err(arg_err) = validate_args(&parsed) {
                    let call = make_tool_call(&run_uuid, tool_call_seq, &parsed, "failed", 1, None);
                    let _ = db::auth_store().insert_tool_call(&call);
                    let _ = db::auth_store().update_tool_call_status(
                        &call.uuid,
                        "failed",
                        None,
                        Some(&arg_err),
                        Some(unix_secs()),
                        Some(unix_secs()),
                    );
                    ctx.emit("tool_failed", json!({
                        "uuid": call.uuid, "name": call.name, "error": arg_err,
                        "args": serde_json::from_str::<Value>(&call.args_json).unwrap_or(Value::Null),
                        "attempt": 1,
                    }));
                    tools::tool_result_xml(&parsed.name, &format!("Invalid args: {arg_err}"))
                } else {
                    execute_one_tool_with_retries(
                        &ctx,
                        &run_uuid,
                        tool_call_seq,
                        &parsed,
                        &running_transcript,
                        user_id,
                        &model,
                    )
                };
                if ctx.is_cancelled() {
                    return;
                }

                current_turns.push(ChatTurn {
                    role: "assistant".into(),
                    content: assistant_raw.clone(),
                });
                let follow_up = if parsed.name == "done" {
                    "The execution phase is complete. Your next reply must be exactly one final answer action: <response>...</response>. Do not call more tools.".to_string()
                } else {
                    format!(
                        "Tool result for {}:\n{}\n\nContinue. Return exactly one action for the next step.",
                        parsed.name,
                        tool_result_xml,
                    )
                };
                current_turns.push(ChatTurn {
                    role: "user".into(),
                    content: follow_up,
                });
                running_transcript.push_str(&format!(
                    "\nassistant: {}\nuser: tool_result\n{}",
                    assistant_raw, tool_result_xml
                ));
            }
        }
    }

    if !saw_final {
        if ctx.is_cancelled() {
            return;
        }
        finalize_failed(
            &ctx,
            &format!("Agent stopped after {MAX_ITERS} iterations without a final answer"),
        );
        return;
    }

    // Finalize
    let final_to_save = if !final_text.trim().is_empty() {
        final_text.clone()
    } else {
        tail_text
    };
    let final_to_save = tools::redact_sensitive(&final_to_save);
    if ctx.is_cancelled() {
        return;
    }
    if final_to_save.trim().is_empty() || tools::looks_like_internal_action(&final_to_save) {
        finalize_failed(
            &ctx,
            "Agent produced an internal action payload instead of a final answer",
        );
        return;
    }
    let now = unix_secs();
    let assistant_msg_uuid = match db::auth_store().append_message(
        user_id,
        &chat_uuid,
        "assistant",
        &final_to_save,
        now,
        Some(&model),
    ) {
        Ok(m) => Some(m.uuid),
        Err(e) => {
            eprintln!("[run/persist final] {e}");
            None
        }
    };

    let _ = db::auth_store().update_run_status(
        &run_uuid,
        "completed",
        Some(&final_to_save),
        None,
        assistant_msg_uuid.as_deref(),
        Some(now),
    );
    ctx.emit(
        "run_completed",
        json!({
            "final_text": final_to_save,
            "assistant_message_uuid": assistant_msg_uuid,
        }),
    );
}

/// Find the first byte position from `from` that could be the start of an XML
/// tag (`<` followed by an alphanumeric or `/`). Returns None if no such char
/// exists. Used to decide where it's safe to emit text without leaking the
/// start of a tool tag.
fn first_potential_tag(buf: &str, from: usize) -> Option<usize> {
    let bytes = buf.as_bytes();
    let mut i = from;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Rebuild a parsed tool back into its XML form for the conversation history.
fn rebuild_tool_xml(parsed: &tools::ParsedToolOwned) -> String {
    let mut s = String::new();
    s.push('<');
    s.push_str(&parsed.name);
    for (k, v) in &parsed.attrs {
        s.push(' ');
        s.push_str(k);
        s.push_str("=\"");
        s.push_str(&v.replace('&', "&amp;").replace('"', "&quot;"));
        s.push('"');
    }
    s.push('>');
    s.push_str(&parsed.body);
    s.push_str("</");
    s.push_str(&parsed.name);
    s.push('>');
    s
}

fn rebuild_response_xml(content: &str) -> String {
    let mut s = String::new();
    s.push_str("<response>");
    s.push_str(content);
    s.push_str("</response>");
    s
}

enum DecidedAction {
    Final {
        content: String,
        assistant_raw: String,
    },
    Tool {
        parsed: tools::ParsedToolOwned,
        assistant_raw: String,
    },
}

struct StreamOutcome {
    text_before: String,
    tool_call: Option<tools::ParsedToolOwned>,
    full_text: String,
}

fn decide_next_action(ctx: &RunContext, turns: &[ChatTurn]) -> Result<DecidedAction, String> {
    let mut repair_turns = turns.to_vec();

    for attempt in 1..=MAX_RETRIES {
        let thinking_started = Instant::now();
        ctx.emit("thinking_started", json!({ "attempt": attempt }));
        let system_prompt = build_system_prompt(ctx, &repair_turns);
        let mut raw = ai::complete_chat_with_system_prompt(
            ctx.model.clone(),
            &system_prompt,
            repair_turns.clone(),
        )?;
        ctx.emit(
            "thinking_done",
            json!({
                "attempt": attempt,
                "duration_ms": thinking_started.elapsed().as_millis() as u64,
                "had_think_block": raw.contains("<think>"),
            }),
        );
        if tools::has_incomplete_tool_call(&raw) {
            raw = complete_incomplete_tool_action(ctx, &repair_turns, raw);
        }

        match tools::parse_assistant_action(&raw) {
            Ok(tools::AssistantAction::Final { content }) => {
                return Ok(DecidedAction::Final {
                    assistant_raw: rebuild_response_xml(&content),
                    content,
                });
            }
            Ok(tools::AssistantAction::Tool(parsed)) => {
                let assistant_raw = rebuild_tool_xml(&parsed);
                return Ok(DecidedAction::Tool {
                    parsed,
                    assistant_raw,
                });
            }
            Err(error) => {
                ctx.emit(
                    "agent_parse_error",
                    json!({
                        "attempt": attempt,
                        "error": error,
                        "raw_preview": raw.chars().take(1200).collect::<String>(),
                    }),
                );
                repair_turns.push(ChatTurn {
                    role: "assistant".into(),
                    content: raw,
                });
                repair_turns.push(ChatTurn {
                    role: "user".into(),
                    content: "Your previous reply was invalid. Respond again with exactly one complete action: either <response>...</response> or one complete tool XML tag. Use XML only. If writing a large file, make this action a small <CreateFile> or <AppendFile> chunk instead of trying to write the entire file at once. No prose, no markdown fences.".into(),
                });
            }
        }
    }

    Err("model failed to return a valid agent action".into())
}

fn complete_incomplete_tool_action(
    ctx: &RunContext,
    _turns: &[ChatTurn],
    mut raw: String,
) -> String {
    for continuation in 1..=MAX_INCOMPLETE_RETRIES {
        if !tools::has_incomplete_tool_call(&raw) {
            break;
        }

        let state = continuation_state(&raw);
        ctx.emit(
            "agent_continuation",
            json!({
                "attempt": continuation,
                "tool": state.tool,
                "path": state.path,
                "missing_close": state.missing_close,
                "raw_chars": raw.chars().count(),
                "body_chars": state.body_chars,
                "body_lines": state.body_lines,
                "tail_preview": state.tail_preview,
            }),
        );

        let mut fix_turns = Vec::new();
        fix_turns.push(ChatTurn {
            role: "assistant".into(),
            content: raw.clone(),
        });
        fix_turns.push(ChatTurn {
            role: "user".into(),
            content: "Your previous response stopped inside an open tool call. Continue from exactly the next character where it stopped, and output only the remaining tool body plus the matching closing tag. Do not repeat any earlier content. Do not add prose or markdown fences.".into(),
        });

        match ai::complete_chat_with_system_prompt(
            ctx.model.clone(),
            CONTINUATION_SYSTEM_PROMPT,
            fix_turns,
        ) {
            Ok(ext) => {
                let raw_ext_chars = ext.chars().count();
                let ext = sanitize_continuation_chunk(&ext);
                if ext.trim().is_empty() {
                    ctx.emit(
                        "agent_continuation_failed",
                        json!({
                            "attempt": continuation,
                            "tool": state.tool,
                            "path": state.path,
                            "error": "model returned empty continuation after sanitization",
                            "raw_delta_chars": raw_ext_chars,
                        }),
                    );
                    break;
                }
                if looks_like_restarted_action(&ext) {
                    ctx.emit("agent_continuation_failed", json!({
                        "attempt": continuation,
                        "tool": state.tool,
                        "path": state.path,
                        "error": "model restarted an action instead of continuing raw tool content",
                        "raw_preview": ext.chars().take(500).collect::<String>(),
                    }));
                    break;
                }
                ctx.emit(
                    "agent_continuation_delta",
                    json!({
                        "attempt": continuation,
                        "tool": state.tool,
                        "path": state.path,
                        "delta_chars": ext.chars().count(),
                        "raw_delta_chars": raw_ext_chars,
                        "delta_preview": ext.chars().take(300).collect::<String>(),
                    }),
                );
                raw.push_str(&ext);
            }
            Err(error) => {
                ctx.emit(
                    "agent_continuation_failed",
                    json!({
                        "attempt": continuation,
                        "tool": state.tool,
                        "path": state.path,
                        "error": error,
                    }),
                );
                break;
            }
        }
    }
    raw
}

struct ContinuationState {
    tool: String,
    path: Option<String>,
    missing_close: String,
    body_chars: usize,
    body_lines: usize,
    tail_preview: String,
}

fn continuation_state(raw: &str) -> ContinuationState {
    let Some((open_start, open_end, open_tag)) = last_supported_open_tag(raw) else {
        return ContinuationState {
            tool: "unknown".into(),
            path: None,
            missing_close: "unknown".into(),
            body_chars: raw.chars().count(),
            body_lines: raw.lines().count(),
            tail_preview: preview_tail(raw, 300),
        };
    };

    let tool = open_tag
        .split_ascii_whitespace()
        .next()
        .unwrap_or("unknown")
        .trim_end_matches('/')
        .to_string();
    let path = extract_attr_from_open_tag(open_tag, "path");
    let body = raw.get(open_end + 1..).unwrap_or_default();

    ContinuationState {
        missing_close: format!("</{tool}>"),
        tool,
        path,
        body_chars: body.chars().count(),
        body_lines: body.lines().count(),
        tail_preview: preview_tail(&raw[open_start..], 300),
    }
}

fn last_supported_open_tag(raw: &str) -> Option<(usize, usize, &str)> {
    let mut cursor = 0;
    let mut last = None;
    while let Some(rel_start) = raw[cursor..].find('<') {
        let start = cursor + rel_start;
        let Some(open_end) = raw[start..].find('>').map(|offset| start + offset) else {
            break;
        };
        let open = &raw[start + 1..open_end];
        let name = open
            .trim()
            .split_ascii_whitespace()
            .next()
            .unwrap_or("")
            .trim_end_matches('/');
        if matches!(
            name,
            "CreateFile"
                | "AppendFile"
                | "ReadFile"
                | "Command"
                | "LongRunProcess"
                | "CreateWorkspace"
                | "WorkspaceStatus"
                | "readSkill"
                | "listSkills"
                | "response"
        ) {
            last = Some((start, open_end, open));
        }
        cursor = open_end + 1;
    }
    last
}

fn extract_attr_from_open_tag(open: &str, key: &str) -> Option<String> {
    let needle = format!("{key}=\"");
    let start = open.find(&needle)? + needle.len();
    let end = open[start..].find('"')?;
    Some(open[start..start + end].to_string())
}

fn preview_tail(raw: &str, max_chars: usize) -> String {
    let chars = raw.chars().collect::<Vec<_>>();
    let start = chars.len().saturating_sub(max_chars);
    chars[start..].iter().collect()
}

fn sanitize_continuation_chunk(chunk: &str) -> String {
    let mut out = String::new();
    let mut cursor = 0;

    while let Some(start_rel) = chunk[cursor..].find("<think>") {
        let start = cursor + start_rel;
        out.push_str(&chunk[cursor..start]);
        let content_start = start + "<think>".len();
        if let Some(end_rel) = chunk[content_start..].find("</think>") {
            cursor = content_start + end_rel + "</think>".len();
        } else {
            return out;
        }
    }
    out.push_str(&chunk[cursor..]);

    out.trim_start_matches("```html")
        .trim_start_matches("```xml")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .to_string()
}

fn looks_like_restarted_action(chunk: &str) -> bool {
    let trimmed = chunk.trim_start();
    trimmed.starts_with("<think>")
        || trimmed.starts_with("<CreateFile")
        || trimmed.starts_with("<AppendFile")
        || trimmed.starts_with("<CreateWorkspace")
        || trimmed.starts_with("<Command")
        || trimmed.starts_with("<LongRunProcess")
        || trimmed.starts_with("<response")
        || trimmed.starts_with('{') && trimmed.contains("\"type\"")
        || trimmed.starts_with("Now ")
        || trimmed.starts_with("Let me ")
        || trimmed.starts_with("I'll ")
}

fn emit_text_chunks(ctx: &RunContext, content: &str) {
    if content.is_empty() {
        return;
    }

    let mut chunk = String::new();
    for ch in content.chars() {
        chunk.push(ch);
        if chunk.len() >= 18 || ch == '\n' {
            ctx.emit("text_delta", json!({ "text": chunk }));
            chunk = String::new();
        }
    }
    if !chunk.is_empty() {
        ctx.emit("text_delta", json!({ "text": chunk }));
    }
}

/// Stream the LLM response. As soon as a complete tool tag appears in the
/// accumulated buffer, stop reading from the provider and return what we have.
///
/// `text_delta` events are emitted ONLY for text guaranteed to be outside any
/// tool body. Once we see a `<` that could be the start of a tool tag, we hold
/// back emission until we know whether it's a tool. This prevents HTML
/// fragments inside `<CreateFile>` bodies from leaking out as visible text.
fn stream_until_tool(ctx: &RunContext, turns: &[ChatTurn]) -> Result<StreamOutcome, String> {
    let provider_rx = ai::stream_chat(ctx.model.clone(), turns.to_vec());
    let mut buf = String::new();
    let mut emitted_up_to: usize = 0; // byte position emitted as text_delta so far
    let mut detected: Option<(tools::ParsedToolOwned, usize)> = None;

    for event in provider_rx {
        if ctx.is_cancelled() {
            return Err("Run cancelled by user".to_string());
        }
        match event {
            StreamEvent::Delta(d) => {
                if d.is_empty() {
                    continue;
                }
                buf.push_str(&d);

                // Check for a complete tool first.
                if let Some((tool, start, _end)) = tools::first_complete_tool(&buf) {
                    // Emit any text before the tool's start that wasn't yet emitted.
                    if emitted_up_to < start {
                        let chunk = &buf[emitted_up_to..start];
                        if !chunk.is_empty() {
                            ctx.emit("text_delta", json!({ "text": chunk }));
                        }
                    }
                    detected = Some((tool, start));
                    break;
                }

                // No complete tool yet. Emit any text up to the first unparsed `<`
                // (could be the start of a tool tag — withhold from there).
                let safe_emit_end = first_potential_tag(&buf, emitted_up_to).unwrap_or(buf.len());
                if safe_emit_end > emitted_up_to {
                    let chunk = &buf[emitted_up_to..safe_emit_end];
                    if !chunk.is_empty() {
                        ctx.emit("text_delta", json!({ "text": chunk }));
                    }
                    emitted_up_to = safe_emit_end;
                }
            }
            StreamEvent::Error(e) => return Err(e),
            StreamEvent::Done => break,
        }
    }

    // Stream finished without a complete tool. Flush any remaining unemitted text.
    if detected.is_none() && emitted_up_to < buf.len() {
        let chunk = &buf[emitted_up_to..];
        if !chunk.is_empty() {
            ctx.emit("text_delta", json!({ "text": chunk }));
        }
    }

    // Recovery: if the buffer ends with an OPEN tool call but no close (token
    // truncation), ask the model to finish it.
    if detected.is_none() && tools::has_incomplete_tool_call(&buf) {
        let mut retries = 0;
        while retries < MAX_INCOMPLETE_RETRIES && tools::has_incomplete_tool_call(&buf) {
            if ctx.is_cancelled() {
                return Err("Run cancelled by user".to_string());
            }
            retries += 1;
            let mut fix_turns = turns.to_vec();
            fix_turns.push(ChatTurn {
                role: "assistant".into(),
                content: buf.clone(),
            });
            fix_turns.push(ChatTurn {
                role: "user".into(),
                content: "Your response was cut off mid tool call. Continue from exactly \
                         where you stopped — output ONLY the remaining content and closing tag. \
                         No repeated content, no commentary."
                    .into(),
            });
            let fix_system_prompt = build_system_prompt(ctx, &fix_turns);
            match ai::complete_chat_with_system_prompt(
                ctx.model.clone(),
                &fix_system_prompt,
                fix_turns,
            ) {
                Ok(ext) if !ext.trim().is_empty() => {
                    ctx.emit("text_delta", json!({ "text": &ext }));
                    buf.push_str(&ext);
                }
                _ => break,
            }
        }
        if let Some((tool, start, _end)) = tools::first_complete_tool(&buf) {
            detected = Some((tool, start));
        }
    }

    if let Some((tool, start)) = detected {
        Ok(StreamOutcome {
            text_before: buf[..start].to_string(),
            tool_call: Some(tool),
            full_text: buf,
        })
    } else {
        Ok(StreamOutcome {
            text_before: String::new(),
            tool_call: None,
            full_text: buf,
        })
    }
}

/// Execute a single tool with up to MAX_RETRIES, persisting per-attempt rows
/// and emitting events for each transition.
fn execute_one_tool_with_retries(
    ctx: &RunContext,
    run_uuid: &str,
    seq: i64,
    parsed: &tools::ParsedToolOwned,
    transcript: &str,
    user_id: i64,
    model: &str,
) -> String {
    let original_uuid = uuid_v4();
    let mut attempt: i64 = 1;
    let mut parent_uuid: Option<String> = None;
    let mut last_result_xml = String::new();

    loop {
        if ctx.is_cancelled() {
            return tools::tool_result_xml(&parsed.name, "Run cancelled by user.");
        }
        let current_uuid = if attempt == 1 {
            original_uuid.clone()
        } else {
            uuid_v4()
        };

        let call = ToolCall {
            uuid: current_uuid.clone(),
            run_uuid: run_uuid.to_string(),
            parent_call_uuid: parent_uuid.clone(),
            seq,
            name: parsed.name.clone(),
            args_json: tools::redact_sensitive(&parsed.args_json),
            body: tools::redact_sensitive(&parsed.body),
            status: "pending".into(),
            output: None,
            error: None,
            attempt,
            started_at: None,
            ended_at: None,
        };
        let _ = db::auth_store().insert_tool_call(&call);
        ctx.emit(
            "tool_pending",
            json!({
                "uuid": current_uuid, "seq": seq, "name": call.name,
                "args": serde_json::from_str::<Value>(&call.args_json).unwrap_or(Value::Null),
                "attempt": attempt,
                "parent_uuid": parent_uuid,
            }),
        );

        let started = unix_secs();
        let _ = db::auth_store().update_tool_call_status(
            &current_uuid,
            "running",
            None,
            None,
            Some(started),
            None,
        );
        ctx.emit(
            "tool_running",
            json!({ "uuid": current_uuid, "name": call.name }),
        );

        let result_xml =
            if parsed.name == "CreateWorkspace" && latest_persisted_workspace(ctx).is_some() {
                existing_workspace_result(
                    ctx,
                    parsed
                        .attrs
                        .get("id")
                        .map(String::as_str)
                        .unwrap_or(parsed.body.as_str()),
                )
            } else {
                tools::execute_tool_owned_with_events(
                    &parsed.name,
                    &parsed.attrs,
                    &parsed.body,
                    transcript,
                    model,
                    user_id,
                    |event| match event {
                        tools::ToolExecutionEvent::OutputChunk(chunk) => {
                            ctx.emit(
                                "tool_output",
                                json!({
                                    "uuid": current_uuid,
                                    "name": call.name,
                                    "chunk": tools::redact_sensitive(&chunk),
                                }),
                            );
                        }
                    },
                )
            };
        let ended = unix_secs();
        if ctx.is_cancelled() {
            let _ = db::auth_store().update_tool_call_status(
                &current_uuid,
                "failed",
                None,
                Some("Run cancelled by user"),
                Some(started),
                Some(ended),
            );
            ctx.emit(
                "tool_failed",
                json!({
                    "uuid": current_uuid, "name": call.name,
                    "error": "Run cancelled by user",
                    "attempt": attempt,
                }),
            );
            return tools::tool_result_xml(&parsed.name, "Run cancelled by user.");
        }

        let (status, output, error) = classify_tool_result(&parsed.name, &result_xml);
        let _ = db::auth_store().update_tool_call_status(
            &current_uuid,
            status,
            output.as_deref(),
            error.as_deref(),
            Some(started),
            Some(ended),
        );

        if status == "succeeded" {
            ctx.emit(
                "tool_succeeded",
                json!({
                    "uuid": current_uuid, "name": call.name,
                    "output": output.clone().unwrap_or_default(),
                    "result_xml": result_xml,
                }),
            );
            return result_xml;
        }

        if status == "approval_required" {
            ctx.emit(
                "tool_approval_required",
                json!({
                    "uuid": current_uuid, "name": call.name,
                    "reason": error.clone().unwrap_or_else(|| "approval required".to_string()),
                    "output": output.clone().unwrap_or_default(),
                    "result_xml": result_xml,
                }),
            );
            return result_xml;
        }

        ctx.emit(
            "tool_failed",
            json!({
                "uuid": current_uuid, "name": call.name,
                "error": error.clone().unwrap_or_default(),
                "attempt": attempt,
            }),
        );
        last_result_xml = result_xml;

        if should_not_retry_tool_failure(&parsed.name, error.as_deref()) {
            return last_result_xml;
        }

        if (attempt as usize) >= MAX_RETRIES {
            return last_result_xml;
        }

        let delay_ms = 1000u64 << (attempt as u32 - 1);
        ctx.emit(
            "tool_retrying",
            json!({
                "original_uuid": original_uuid, "attempt_next": attempt + 1,
                "delay_ms": delay_ms,
            }),
        );
        let sleep_started = Instant::now();
        while sleep_started.elapsed() < Duration::from_millis(delay_ms) {
            if ctx.is_cancelled() {
                return tools::tool_result_xml(&parsed.name, "Run cancelled by user.");
            }
            thread::sleep(Duration::from_millis(50));
        }
        parent_uuid = Some(current_uuid);
        attempt += 1;
    }
}

fn finalize_failed(ctx: &RunContext, error_summary: &str) {
    if ctx.is_cancelled() {
        return;
    }
    let now = unix_secs();
    let _ = db::auth_store().update_run_status(
        &ctx.run_uuid,
        "failed",
        None,
        Some(error_summary),
        None,
        Some(now),
    );
    ctx.emit("run_failed", json!({ "error": error_summary }));
}

fn loop_history_key(tool: &tools::ParsedToolOwned) -> String {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    tool.body.trim().hash(&mut hasher);
    let body_hash = hasher.finish();
    format!("{}|{}|body:{body_hash:016x}", tool.name, tool.args_json)
}

// ── Streaming helpers ────────────────────────────────────────────────────────

fn stream_llm(ctx: &RunContext, turns: &[ChatTurn]) -> Result<String, String> {
    let provider_rx = ai::stream_chat(ctx.model.clone(), turns.to_vec());
    let mut buf = String::new();
    for event in provider_rx {
        match event {
            StreamEvent::Delta(d) => {
                buf.push_str(&d);
                ctx.emit("text_delta", json!({ "text": d }));
            }
            StreamEvent::Error(e) => return Err(e),
            StreamEvent::Done => break,
        }
    }
    Ok(buf)
}

// ── Validation ───────────────────────────────────────────────────────────────

fn validate_args(tool: &tools::ParsedToolOwned) -> Result<(), String> {
    let need_id = matches!(
        tool.name.as_str(),
        "WorkspaceStatus"
            | "Command"
            | "LongRunProcess"
            | "CreateFile"
            | "AppendFile"
            | "PatchFile"
            | "ReadFile"
            | "DeleteFile"
            | "Preview"
            | "CreateDirectory"
            | "DeleteDirectory"
    );
    if need_id {
        let id = tool
            .attrs
            .get("id")
            .map(String::as_str)
            .unwrap_or("")
            .trim();
        if id.is_empty() {
            return Err(format!("Tool '{}' requires id attribute", tool.name));
        }
    }
    if matches!(
        tool.name.as_str(),
        "CreateFile" | "AppendFile" | "PatchFile" | "ReadFile" | "DeleteFile" | "Preview"
    ) {
        let path = tool
            .attrs
            .get("path")
            .map(String::as_str)
            .unwrap_or("")
            .trim();
        if path.is_empty() {
            return Err(format!("Tool '{}' requires path attribute", tool.name));
        }
    }
    if matches!(tool.name.as_str(), "CreateFile" | "AppendFile") {
        let char_count = tool.body.chars().count();
        if char_count > MAX_FILE_TOOL_CHARS {
            return Err(format!(
                "Tool '{}' body is too large ({} chars). Use <CreateFile id=\"...\" path=\"...\"></CreateFile> to create/truncate, then multiple small <AppendFile> chunks of at most {MAX_FILE_TOOL_CHARS} chars.",
                tool.name,
                char_count,
            ));
        }
    }
    Ok(())
}

// ── Result classification ────────────────────────────────────────────────────

fn classify_tool_result(name: &str, xml: &str) -> (&'static str, Option<String>, Option<String>) {
    // Tools that produce a <tool_result> block — we look at the body.
    let body = tools::decode_xml_entities(
        &extract_tool_result_body(xml).unwrap_or_else(|| xml.to_string()),
    );
    let body_lower = body.to_lowercase();
    if body_lower.starts_with("approval required:") {
        return ("approval_required", Some(body.clone()), Some(body));
    }
    let looks_failed = if name == "Command" || name == "LongRunProcess" {
        body_lower
            .lines()
            .rev()
            .find(|line| line.starts_with("exit: "))
            .is_some_and(|line| !line.starts_with("exit: 0"))
    } else if matches!(
        name,
        "CreateWorkspace"
            | "WorkspaceStatus"
            | "CreateFile"
            | "AppendFile"
            | "PatchFile"
            | "ReadFile"
            | "DeleteFile"
            | "Preview"
            | "CreateDirectory"
            | "DeleteDirectory"
    ) {
        body_lower.starts_with("workspace ") && body_lower.contains(" not found")
            || body_lower.starts_with("database error:")
            || body_lower.starts_with("no workspace id provided")
            || body_lower.starts_with("failed to create workspace:")
            || body_lower.starts_with("workspace registered but directory creation failed:")
            || body_lower.starts_with("createfile failed:")
            || body_lower.starts_with("appendfile failed:")
            || body_lower.starts_with("patchfile failed:")
            || body_lower.starts_with("readfile failed:")
            || body_lower.starts_with("deletefile failed:")
            || body_lower.starts_with("createdirectory failed:")
            || body_lower.starts_with("deletedirectory failed:")
    } else {
        body_lower.starts_with("error")
            || body_lower.starts_with("failed:")
            || body_lower.starts_with("invalid args:")
    };
    if looks_failed {
        ("failed", Some(body.clone()), Some(body))
    } else {
        ("succeeded", Some(body), None)
    }
}

fn extract_tool_result_body(xml: &str) -> Option<String> {
    let start = xml.find('>')?;
    let end_marker = "</tool_result>";
    let end = xml.rfind(end_marker)?;
    if end <= start {
        return None;
    }
    Some(xml[start + 1..end].to_string())
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn should_not_retry_tool_failure(name: &str, error: Option<&str>) -> bool {
    if !matches!(name, "Command" | "LongRunProcess") {
        return false;
    }
    let error = error.unwrap_or_default().to_lowercase();
    error.contains("failed to execute command")
        || error.contains("command not found")
        || error.contains("no such file or directory")
        || error.contains("exit: -1")
        || error.contains("exit: 126")
        || error.contains("exit: 127")
}

fn redact_json_value(value: Value) -> Value {
    match value {
        Value::String(s) => Value::String(tools::redact_sensitive(&s)),
        Value::Array(items) => Value::Array(items.into_iter().map(redact_json_value).collect()),
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| {
                    let key_lower = key.to_ascii_lowercase();
                    let redacted_value = if key_lower.contains("password")
                        || key_lower.contains("passwd")
                        || key_lower.contains("passphrase")
                        || key_lower.contains("token")
                        || key_lower.contains("secret")
                        || key_lower.contains("api_key")
                        || key_lower.contains("api-key")
                        || key_lower.contains("apikey")
                    {
                        Value::String("[REDACTED]".to_string())
                    } else {
                        redact_json_value(value)
                    };
                    (key, redacted_value)
                })
                .collect(),
        ),
        other => other,
    }
}

fn make_tool_call(
    run_uuid: &str,
    seq: i64,
    parsed: &tools::ParsedToolOwned,
    status: &str,
    attempt: i64,
    parent: Option<String>,
) -> ToolCall {
    let now = unix_secs();
    ToolCall {
        uuid: uuid_v4(),
        run_uuid: run_uuid.to_string(),
        parent_call_uuid: parent,
        seq,
        name: parsed.name.clone(),
        args_json: tools::redact_sensitive(&parsed.args_json),
        body: tools::redact_sensitive(&parsed.body),
        status: status.to_string(),
        output: None,
        error: None,
        attempt,
        started_at: Some(now),
        ended_at: Some(now),
    }
}

struct RunContext {
    run_uuid: String,
    user_id: i64,
    chat_uuid: String,
    model: String,
}

impl RunContext {
    fn new(run_uuid: String, user_id: i64, chat_uuid: String, model: String) -> Self {
        Self {
            run_uuid,
            user_id,
            chat_uuid,
            model,
        }
    }

    fn emit(&self, event_type: &str, payload: Value) {
        emit_run_event(&self.run_uuid, event_type, payload);
        let _ = self.user_id;
        let _ = &self.chat_uuid;
        let _ = &self.model; // suppress unused
    }

    fn is_cancelled(&self) -> bool {
        if cancelled_runs().lock().unwrap().contains(&self.run_uuid) {
            return true;
        }
        matches!(
            db::auth_store().get_run(self.user_id, &self.run_uuid),
            Ok(Some(run)) if run.status == "cancelled"
        )
    }
}

fn next_seq(run_uuid: &str) -> i64 {
    // Compute next monotonic seq based on max(seq) in DB.
    // This is per-run so contention is minimal.
    let cur = db::auth_store().max_event_seq(run_uuid).unwrap_or(0);
    cur + 1
}

fn unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn uuid_v4() -> String {
    use std::fs::File;
    use std::io::Read;
    let mut bytes = [0u8; 16];
    if let Ok(mut f) = File::open("/dev/urandom") {
        let _ = f.read_exact(&mut bytes);
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        bytes[6], bytes[7],
        bytes[8], bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}
