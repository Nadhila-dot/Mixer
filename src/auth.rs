use crate::{
    ai::{self, ChatTurn, StreamEvent},
    db::{self, DbError, User},
    router::{Request, Response, ResponseBody},
    run, tools,
};
use chrono::{Datelike, TimeZone, Utc};
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::Read,
    time::{SystemTime, UNIX_EPOCH},
};

const SESSION_COOKIE: &str = "forge_session";
const SESSION_TTL_SECS: u64 = 60 * 60 * 24 * 7;
const RETIRED_PLACEHOLDER_REPLY: &str = "Ai system will respond here in the later builds oki doki";

#[derive(Debug, Deserialize)]
struct AuthInput {
    email: String,
    password: String,
    name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ChatInput {
    message: String,
    model: Option<String>,
    #[serde(default)]
    attachments: Vec<ChatAttachmentInput>,
}

#[derive(Debug, Deserialize)]
struct ModelInput {
    model: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatAttachmentInput {
    pub name: String,
    #[serde(default)]
    pub mime_type: String,
    pub size: u64,
    #[serde(default)]
    pub encoding: String,
    #[serde(default)]
    pub content: String,
}

#[derive(Debug, Deserialize)]
pub struct WsChatInput {
    pub action: String,
    pub chat_id: String,
    pub message: Option<String>,
    pub model: Option<String>,
    #[serde(default)]
    attachments: Vec<ChatAttachmentInput>,
    pub run_id: Option<String>,
    pub after_seq: Option<i64>,
}

pub async fn me(req: &Request) -> Response {
    match current_user(req) {
        Ok(Some(user)) => Response::json(user_json(&user).to_string()),
        Ok(None) => auth_error(401, "unauthenticated"),
        Err(e) => server_error(e),
    }
}

pub async fn app_state(req: &Request) -> Response {
    match app_state_json(req) {
        Ok(data) => Response::json(data.to_string()),
        Err(e) => server_error(e),
    }
}

pub async fn models() -> Response {
    let models = ai::list_models()
        .into_iter()
        .map(|model| {
            json!({
                "id": model.id,
                "provider": model.provider,
                "label": model.label,
            })
        })
        .collect::<Vec<_>>();

    Response::json(
        json!({
            "provider": std::env::var("AI_PROVIDER").unwrap_or_else(|_| "vultr".into()),
            "default_model": ai::default_model(),
            "models": models,
        })
        .to_string(),
    )
}

pub async fn chat(_req: &Request) -> Response {
    Response::json(
        json!({
            "ok": false,
            "error": "use /api/chat/start plus /api/chats/<uuid>/assistant-stream",
        })
        .to_string(),
    )
    .with_status(410)
}

pub async fn start_chat(req: &Request) -> Response {
    let user = match current_user(req) {
        Ok(Some(user)) => user,
        Ok(None) => return auth_error(401, "unauthenticated"),
        Err(e) => return server_error(e),
    };

    let input = match parse_chat_input(req) {
        Ok(input) => input,
        Err(message) => return auth_error(400, message),
    };

    let message = input.message.trim();
    if message.is_empty() && input.attachments.is_empty() {
        return auth_error(400, "message is required");
    }
    let prepared_message = match prepare_user_message(message, &input.attachments) {
        Ok(value) => value,
        Err(error) => return auth_error(400, error),
    };
    let display_message = display_message_for(message, &input.attachments);

    let (updated_usage, limits, now) = match reserve_usage(&user, &prepared_message) {
        Ok(value) => value,
        Err(response) => return response,
    };

    let store = db::auth_store();
    let title = chat_title(&display_message);
    let uuid = random_uuid_v4();
    let chat = match store.create_chat(user.id, &uuid, &title, &display_message, now) {
        Ok(chat) => chat,
        Err(e) => return server_error(e),
    };
    let user_message =
        match store.append_message(user.id, &uuid, "user", &prepared_message, now, None) {
            Ok(message) => message,
            Err(e) => return server_error(e),
        };

    Response::json(
        json!({
            "ok": true,
            "chat": chat_json(&chat),
            "user_message": message_json(&user_message),
            "usage": usage_json(&updated_usage),
            "limits": limits_json(limits),
            "model": input.model.unwrap_or_else(ai::default_model),
        })
        .to_string(),
    )
}

pub async fn stream_new_chat(req: &Request) -> Response {
    let user = match current_user(req) {
        Ok(Some(user)) => user,
        Ok(None) => return auth_error(401, "unauthenticated"),
        Err(e) => return server_error(e),
    };

    let input = match parse_chat_input(req) {
        Ok(input) => input,
        Err(message) => return auth_error(400, message),
    };
    let message = input.message.trim();
    if message.is_empty() && input.attachments.is_empty() {
        return auth_error(400, "message is required");
    }
    let prepared_message = match prepare_user_message(message, &input.attachments) {
        Ok(value) => value,
        Err(error) => return auth_error(400, error),
    };
    let display_message = display_message_for(message, &input.attachments);

    let (updated_usage, limits, now) = match reserve_usage(&user, &prepared_message) {
        Ok(value) => value,
        Err(response) => return response,
    };

    let store = db::auth_store();
    let title = chat_title(&display_message);
    let uuid = random_uuid_v4();
    let chat = match store.create_chat(user.id, &uuid, &title, &display_message, now) {
        Ok(chat) => chat,
        Err(e) => return server_error(e),
    };
    let user_message =
        match store.append_message(user.id, &uuid, "user", &prepared_message, now, None) {
            Ok(message) => message,
            Err(e) => return server_error(e),
        };

    stream_assistant_response(
        user.id,
        uuid,
        chat,
        user_message,
        vec![ChatTurn {
            role: "user".into(),
            content: prepared_message,
        }],
        input.model.unwrap_or_else(ai::default_model),
        updated_usage,
        limits,
    )
}

pub async fn stream_existing_assistant(req: &Request, uuid: &str) -> Response {
    if !is_uuid_like(uuid) {
        return auth_error(400, "invalid chat id");
    }

    let user = match current_user(req) {
        Ok(Some(user)) => user,
        Ok(None) => return auth_error(401, "unauthenticated"),
        Err(e) => return server_error(e),
    };

    let input = match parse_model_input(req) {
        Ok(input) => input,
        Err(message) => return auth_error(400, message),
    };

    let store = db::auth_store();
    let chat = match store.chat_by_uuid(user.id, uuid) {
        Ok(Some(chat)) => chat,
        Ok(None) => {
            return Response::json(json!({ "ok": false, "error": "chat not found" }).to_string())
                .with_status(404);
        }
        Err(e) => return server_error(e),
    };
    let messages = match store.chat_messages(user.id, uuid) {
        Ok(messages) => messages,
        Err(e) => return server_error(e),
    };
    let messages = visible_messages(messages);
    let Some(user_message) = messages
        .last()
        .filter(|message| message.role == "user")
        .cloned()
    else {
        return auth_error(409, "latest message already has an assistant response");
    };

    let usage = match store.usage_for_day(user.id, &current_usage_day()) {
        Ok(usage) => usage,
        Err(e) => return server_error(e),
    };
    let limits = tier_limits(&user.tier);
    let turns = messages
        .into_iter()
        .map(|message| ChatTurn {
            role: message.role,
            content: message.content,
        })
        .collect::<Vec<_>>();

    stream_assistant_response(
        user.id,
        uuid.to_string(),
        chat,
        user_message,
        turns,
        input.model.unwrap_or_else(ai::default_model),
        usage,
        limits,
    )
}

pub async fn get_chat(req: &Request, uuid: &str) -> Response {
    if !is_uuid_like(uuid) {
        return auth_error(400, "invalid chat id");
    }

    let user = match current_user(req) {
        Ok(Some(user)) => user,
        Ok(None) => return auth_error(401, "unauthenticated"),
        Err(e) => return server_error(e),
    };

    let store = db::auth_store();
    let chat = match store.chat_by_uuid(user.id, uuid) {
        Ok(Some(chat)) => chat,
        Ok(None) => {
            return Response::json(json!({ "ok": false, "error": "chat not found" }).to_string())
                .with_status(404);
        }
        Err(e) => return server_error(e),
    };
    let messages = match store.chat_messages(user.id, uuid) {
        Ok(messages) => messages,
        Err(e) => return server_error(e),
    };
    let messages = visible_messages(messages);

    Response::json(
        json!({
            "ok": true,
            "chat": chat_json(&chat),
            "messages": messages.iter().map(message_json).collect::<Vec<_>>(),
        })
        .to_string(),
    )
}

pub async fn append_chat_message(_req: &Request, uuid: &str) -> Response {
    if !is_uuid_like(uuid) {
        return auth_error(400, "invalid chat id");
    }

    Response::json(
        json!({
            "ok": false,
            "error": "use /api/chats/<uuid>/stream",
        })
        .to_string(),
    )
    .with_status(410)
}

pub async fn stream_chat_message(req: &Request, uuid: &str) -> Response {
    if !is_uuid_like(uuid) {
        return auth_error(400, "invalid chat id");
    }

    let user = match current_user(req) {
        Ok(Some(user)) => user,
        Ok(None) => return auth_error(401, "unauthenticated"),
        Err(e) => return server_error(e),
    };

    let input = match parse_chat_input(req) {
        Ok(input) => input,
        Err(message) => return auth_error(400, message),
    };
    let message = input.message.trim();
    if message.is_empty() && input.attachments.is_empty() {
        return auth_error(400, "message is required");
    }
    let prepared_message = match prepare_user_message(message, &input.attachments) {
        Ok(value) => value,
        Err(error) => return auth_error(400, error),
    };
    let display_message = display_message_for(message, &input.attachments);

    let store = db::auth_store();
    let chat = match store.chat_by_uuid(user.id, uuid) {
        Ok(Some(chat)) => chat,
        Ok(None) => {
            return Response::json(json!({ "ok": false, "error": "chat not found" }).to_string())
                .with_status(404);
        }
        Err(e) => return server_error(e),
    };
    let previous_messages = match store.chat_messages(user.id, uuid) {
        Ok(messages) => messages,
        Err(e) => return server_error(e),
    };
    let previous_messages = visible_messages(previous_messages);

    let (updated_usage, limits, now) = match reserve_usage(&user, &prepared_message) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let user_message =
        match store.append_message(user.id, uuid, "user", &prepared_message, now, None) {
            Ok(message) => message,
            Err(e) => return server_error(e),
        };

    let mut turns = previous_messages
        .into_iter()
        .map(|message| ChatTurn {
            role: message.role,
            content: message.content,
        })
        .collect::<Vec<_>>();
    turns.push(ChatTurn {
        role: "user".into(),
        content: prepared_message,
    });

    stream_assistant_response(
        user.id,
        uuid.to_string(),
        db::ChatSummary {
            preview: display_message,
            updated_at: now,
            ..chat
        },
        user_message,
        turns,
        input.model.unwrap_or_else(ai::default_model),
        updated_usage,
        limits,
    )
}

pub fn websocket_chat_stream(
    cookie: Option<String>,
    input: WsChatInput,
) -> Result<std::sync::mpsc::Receiver<String>, (u16, String)> {
    if input.action != "subscribe_run" && !is_uuid_like(&input.chat_id) {
        return Err((400, "invalid chat id".into()));
    }

    let user = current_user_from_cookie(cookie.as_deref())
        .map_err(|e| (500, e.to_string()))?
        .ok_or_else(|| (401, "unauthenticated".into()))?;

    let response = match input.action.as_str() {
        "create" => {
            let message = input.message.as_deref().map(str::trim).unwrap_or_default();
            if message.is_empty() && input.attachments.is_empty() {
                return Err((400, "message is required".to_string()));
            }
            let prepared_message =
                prepare_user_message(message, &input.attachments).map_err(|error| (400, error))?;
            let display_message = display_message_for(message, &input.attachments);
            stream_new_chat_for_user(
                &user,
                &input.chat_id,
                &display_message,
                &prepared_message,
                input.model,
            )
        }
        "send" => {
            let message = input.message.as_deref().map(str::trim).unwrap_or_default();
            if message.is_empty() && input.attachments.is_empty() {
                return Err((400, "message is required".to_string()));
            }
            let prepared_message =
                prepare_user_message(message, &input.attachments).map_err(|error| (400, error))?;
            let display_message = display_message_for(message, &input.attachments);
            stream_chat_message_for_user(
                &user,
                &input.chat_id,
                &display_message,
                &prepared_message,
                input.model,
            )
        }
        "assistant" => stream_existing_assistant_for_user(&user, &input.chat_id, input.model),
        "subscribe_run" => {
            return subscribe_run_for_user(&user, input.run_id.as_deref(), input.after_seq)
        }
        _ => return Err((400, "unknown websocket action".into())),
    };

    match response.body {
        ResponseBody::Stream(rx) if response.status == 200 => Ok(rx),
        ResponseBody::Fixed(body) => Err((
            response.status,
            error_from_json_body(&body).unwrap_or_else(|| "websocket chat stream failed".into()),
        )),
        ResponseBody::Stream(_) => Err((response.status, "websocket chat stream failed".into())),
    }
}

fn subscribe_run_for_user(
    user: &User,
    run_id: Option<&str>,
    after_seq: Option<i64>,
) -> Result<std::sync::mpsc::Receiver<String>, (u16, String)> {
    let run_uuid = run_id
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| (400, "run_id is required".to_string()))?;
    let run = db::auth_store()
        .get_run(user.id, run_uuid)
        .map_err(|e| (500, e.to_string()))?
        .ok_or_else(|| (404, "run not found".to_string()))?;
    let since = after_seq.unwrap_or(0);
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let run_uuid = run.uuid.clone();
    let chat_uuid = run.chat_uuid.clone();
    let user_id = user.id;

    std::thread::spawn(move || {
        let (live_tx, live_rx) = std::sync::mpsc::channel::<String>();
        run::subscribe(&run_uuid, live_tx);

        let mut last_seq = since;
        if let Ok(events) = db::auth_store().events_for_run_since(&run_uuid, since) {
            for event in events {
                let payload = serde_json::from_str::<serde_json::Value>(&event.payload_json)
                    .unwrap_or_else(|_| json!({}));
                let envelope = json!({
                    "seq": event.seq,
                    "type": event.event_type,
                    "payload": payload,
                    "created_at": event.created_at,
                });
                last_seq = last_seq.max(event.seq);
                let _ = tx.send(sse("agent_event", envelope));
            }
        }

        if let Ok(Some(updated)) = db::auth_store().get_run(user_id, &run_uuid) {
            if updated.status != "running" {
                finish_run_subscription(&tx, user_id, &chat_uuid, &updated);
                return;
            }
        }

        while let Ok(envelope_str) = live_rx.recv() {
            if envelope_str.is_empty() {
                continue;
            }
            let envelope: serde_json::Value = match serde_json::from_str(&envelope_str) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let seq = envelope.get("seq").and_then(|v| v.as_i64()).unwrap_or(0);
            if seq <= last_seq {
                continue;
            }
            last_seq = seq;
            let _ = tx.send(sse("agent_event", envelope.clone()));
            let event_type = envelope.get("type").and_then(|v| v.as_str()).unwrap_or("");
            if matches!(event_type, "run_completed" | "run_failed" | "run_cancelled") {
                if let Ok(Some(updated)) = db::auth_store().get_run(user_id, &run_uuid) {
                    finish_run_subscription(&tx, user_id, &chat_uuid, &updated);
                } else {
                    let _ = tx.send(sse("done", json!({ "assistant_message": null })));
                }
                break;
            }
        }
    });

    Ok(rx)
}

fn finish_run_subscription(
    tx: &std::sync::mpsc::Sender<String>,
    user_id: i64,
    chat_uuid: &str,
    run: &db::AgentRun,
) {
    if run.status == "failed" {
        if let Some(error) = &run.error_summary {
            let _ = tx.send(sse("error", json!({ "error": error })));
        }
    }

    let assistant_message = run
        .assistant_message_uuid
        .as_deref()
        .and_then(|message_uuid| {
            db::auth_store()
                .chat_messages(user_id, chat_uuid)
                .ok()
                .and_then(|messages| messages.into_iter().find(|m| m.uuid == message_uuid))
        });

    let _ = tx.send(sse(
        "done",
        json!({
            "assistant_message": assistant_message.as_ref().map(message_json),
        }),
    ));
}

fn stream_new_chat_for_user(
    user: &User,
    uuid: &str,
    display_message: &str,
    prompt_message: &str,
    model: Option<String>,
) -> Response {
    let (updated_usage, limits, now) = match reserve_usage(user, prompt_message) {
        Ok(value) => value,
        Err(response) => return response,
    };

    let store = db::auth_store();
    let title = chat_title(display_message);
    let chat = match store.create_chat(user.id, uuid, &title, display_message, now) {
        Ok(chat) => chat,
        Err(e) => return server_error(e),
    };
    let user_message = match store.append_message(user.id, uuid, "user", prompt_message, now, None)
    {
        Ok(msg) => msg,
        Err(e) => return server_error(e),
    };

    stream_assistant_response(
        user.id,
        uuid.to_string(),
        chat,
        user_message,
        vec![ChatTurn {
            role: "user".into(),
            content: prompt_message.to_string(),
        }],
        model.unwrap_or_else(ai::default_model),
        updated_usage,
        limits,
    )
}

fn stream_chat_message_for_user(
    user: &User,
    uuid: &str,
    display_message: &str,
    prompt_message: &str,
    model: Option<String>,
) -> Response {
    let store = db::auth_store();
    let chat = match store.chat_by_uuid(user.id, uuid) {
        Ok(Some(chat)) => chat,
        Ok(None) => {
            return Response::json(json!({ "ok": false, "error": "chat not found" }).to_string())
                .with_status(404);
        }
        Err(e) => return server_error(e),
    };
    let previous_messages = match store.chat_messages(user.id, uuid) {
        Ok(messages) => messages,
        Err(e) => return server_error(e),
    };
    let previous_messages = visible_messages(previous_messages);

    let (updated_usage, limits, now) = match reserve_usage(user, prompt_message) {
        Ok(value) => value,
        Err(response) => return response,
    };
    let user_message = match store.append_message(user.id, uuid, "user", prompt_message, now, None)
    {
        Ok(message) => message,
        Err(e) => return server_error(e),
    };

    let mut turns = previous_messages
        .into_iter()
        .map(|message| ChatTurn {
            role: message.role,
            content: message.content,
        })
        .collect::<Vec<_>>();
    turns.push(ChatTurn {
        role: "user".into(),
        content: prompt_message.to_string(),
    });

    stream_assistant_response(
        user.id,
        uuid.to_string(),
        db::ChatSummary {
            preview: display_message.to_string(),
            updated_at: now,
            ..chat
        },
        user_message,
        turns,
        model.unwrap_or_else(ai::default_model),
        updated_usage,
        limits,
    )
}

fn stream_existing_assistant_for_user(user: &User, uuid: &str, model: Option<String>) -> Response {
    let store = db::auth_store();
    let chat = match store.chat_by_uuid(user.id, uuid) {
        Ok(Some(chat)) => chat,
        Ok(None) => {
            return Response::json(json!({ "ok": false, "error": "chat not found" }).to_string())
                .with_status(404);
        }
        Err(e) => return server_error(e),
    };
    let messages = match store.chat_messages(user.id, uuid) {
        Ok(messages) => messages,
        Err(e) => return server_error(e),
    };
    let messages = visible_messages(messages);
    let Some(user_message) = messages
        .last()
        .filter(|message| message.role == "user")
        .cloned()
    else {
        return auth_error(409, "latest message already has an assistant response");
    };

    let usage = match store.usage_for_day(user.id, &current_usage_day()) {
        Ok(usage) => usage,
        Err(e) => return server_error(e),
    };
    let limits = tier_limits(&user.tier);
    let turns = messages
        .into_iter()
        .map(|message| ChatTurn {
            role: message.role,
            content: message.content,
        })
        .collect::<Vec<_>>();

    stream_assistant_response(
        user.id,
        uuid.to_string(),
        chat,
        user_message,
        turns,
        model.unwrap_or_else(ai::default_model),
        usage,
        limits,
    )
}

fn error_from_json_body(body: &[u8]) -> Option<String> {
    serde_json::from_slice::<serde_json::Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("error")
                .and_then(|error| error.as_str())
                .map(str::to_string)
        })
}

pub async fn list_runs_for_chat(req: &Request, chat_uuid: &str) -> Response {
    if !is_uuid_like(chat_uuid) {
        return auth_error(400, "invalid chat id");
    }
    let user = match current_user(req) {
        Ok(Some(user)) => user,
        Ok(None) => return auth_error(401, "unauthenticated"),
        Err(e) => return server_error(e),
    };
    let store = db::auth_store();
    let runs = match store.runs_for_chat(user.id, chat_uuid) {
        Ok(r) => r,
        Err(e) => return server_error(e),
    };
    let mut out = Vec::new();
    for run in &runs {
        let tool_calls = store.tool_calls_for_run(&run.uuid).unwrap_or_default();
        let events = store.events_for_run_since(&run.uuid, 0).unwrap_or_default();
        out.push(json!({
            "uuid": run.uuid,
            "chat_uuid": run.chat_uuid,
            "user_message_uuid": run.user_message_uuid,
            "assistant_message_uuid": run.assistant_message_uuid,
            "model": run.model,
            "status": run.status,
            "final_text": run.final_text,
            "error_summary": run.error_summary,
            "started_at": run.started_at,
            "ended_at": run.ended_at,
            "tool_calls": tool_calls.iter().map(tool_call_json).collect::<Vec<_>>(),
            "events": events.iter().map(agent_event_json).collect::<Vec<_>>(),
        }));
    }
    Response::json(json!({ "ok": true, "runs": out }).to_string())
}

pub async fn get_run(req: &Request, run_uuid: &str) -> Response {
    let user = match current_user(req) {
        Ok(Some(user)) => user,
        Ok(None) => return auth_error(401, "unauthenticated"),
        Err(e) => return server_error(e),
    };
    let store = db::auth_store();
    let run = match store.get_run(user.id, run_uuid) {
        Ok(Some(r)) => r,
        Ok(None) => return auth_error(404, "run not found"),
        Err(e) => return server_error(e),
    };
    let since: i64 = req
        .query
        .as_deref()
        .and_then(|q| {
            q.split('&').find_map(|pair| {
                let (k, v) = pair.split_once('=')?;
                if k == "since" {
                    v.parse().ok()
                } else {
                    None
                }
            })
        })
        .unwrap_or(0);
    let events = store
        .events_for_run_since(&run.uuid, since)
        .unwrap_or_default();
    let tool_calls = store.tool_calls_for_run(&run.uuid).unwrap_or_default();

    Response::json(
        json!({
            "ok": true,
            "run": {
                "uuid": run.uuid,
                "chat_uuid": run.chat_uuid,
                "user_message_uuid": run.user_message_uuid,
                "assistant_message_uuid": run.assistant_message_uuid,
                "model": run.model,
                "status": run.status,
                "final_text": run.final_text,
                "error_summary": run.error_summary,
                "started_at": run.started_at,
                "ended_at": run.ended_at,
            },
            "tool_calls": tool_calls.iter().map(tool_call_json).collect::<Vec<_>>(),
            "events": events.iter().map(agent_event_json).collect::<Vec<_>>(),
        })
        .to_string(),
    )
}

pub async fn cancel_run(req: &Request, run_uuid: &str) -> Response {
    let run_uuid = run_uuid.trim();
    if run_uuid.is_empty() || run_uuid.contains('/') {
        return auth_error(400, "invalid run id");
    }
    let user = match current_user(req) {
        Ok(Some(user)) => user,
        Ok(None) => return auth_error(401, "unauthenticated"),
        Err(e) => return server_error(e),
    };
    match run::cancel_run(user.id, run_uuid) {
        Ok(()) => Response::json(json!({ "ok": true, "status": "cancelled" }).to_string()),
        Err(error) if error == "run not found" => auth_error(404, error),
        Err(error) => server_error(error),
    }
}

fn tool_call_json(t: &db::ToolCall) -> serde_json::Value {
    json!({
        "uuid": t.uuid,
        "run_uuid": t.run_uuid,
        "parent_call_uuid": t.parent_call_uuid,
        "seq": t.seq,
        "name": t.name,
        "args": serde_json::from_str::<serde_json::Value>(&t.args_json).unwrap_or(json!({})),
        "body": t.body,
        "status": t.status,
        "output": t.output,
        "error": t.error,
        "attempt": t.attempt,
        "started_at": t.started_at,
        "ended_at": t.ended_at,
    })
}

fn agent_event_json(e: &db::AgentEvent) -> serde_json::Value {
    json!({
        "seq": e.seq,
        "type": e.event_type,
        "payload": serde_json::from_str::<serde_json::Value>(&e.payload_json).unwrap_or(json!({})),
        "created_at": e.created_at,
    })
}

pub async fn delete_chat(req: &Request, uuid: &str) -> Response {
    if !is_uuid_like(uuid) {
        return auth_error(400, "invalid chat id");
    }

    let user = match current_user(req) {
        Ok(Some(user)) => user,
        Ok(None) => return auth_error(401, "unauthenticated"),
        Err(e) => return server_error(e),
    };

    match db::auth_store().delete_chat(user.id, uuid) {
        Ok(true) => Response::json(json!({ "ok": true }).to_string()),
        Ok(false) => Response::json(json!({ "ok": false, "error": "chat not found" }).to_string())
            .with_status(404),
        Err(e) => server_error(e),
    }
}

pub async fn login(req: &Request) -> Response {
    let input = match parse_auth_input(req) {
        Ok(input) => input,
        Err(message) => return auth_error(400, message),
    };

    match verify_login(&input.email, &input.password) {
        Ok(Some(user)) => match create_session(user.id) {
            Ok(token) => {
                Response::json(json!({ "ok": true, "user": user_json(&user) }).to_string())
                    .with_header("Set-Cookie", session_cookie(&token))
            }
            Err(e) => server_error(e),
        },
        Ok(None) => auth_error(401, "invalid email or password"),
        Err(e) => server_error(e),
    }
}

pub async fn register(req: &Request) -> Response {
    let input = match parse_auth_input(req) {
        Ok(input) => input,
        Err(message) => return auth_error(400, message),
    };

    let name = input
        .name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| input.email.split('@').next().unwrap_or("User"));

    match create_user(name, &input.email, &input.password) {
        Ok(user) => match create_session(user.id) {
            Ok(token) => {
                Response::json(json!({ "ok": true, "user": user_json(&user) }).to_string())
                    .with_header("Set-Cookie", session_cookie(&token))
            }
            Err(e) => server_error(e),
        },
        Err(DbError::DuplicateEmail) => auth_error(409, "email already registered"),
        Err(e) => server_error(e),
    }
}

pub async fn logout(req: &Request) -> Response {
    if let Some(token) = session_token(req) {
        let _ = db::auth_store().delete_session(&token);
    }

    Response::json(json!({ "ok": true }).to_string())
        .with_header("Set-Cookie", expired_session_cookie())
}

pub async fn logout_redirect(req: &Request) -> Response {
    if let Some(token) = session_token(req) {
        let _ = db::auth_store().delete_session(&token);
    }

    Response::redirect("/auth/screen").with_header("Set-Cookie", expired_session_cookie())
}

pub fn current_user(req: &Request) -> Result<Option<User>, DbError> {
    current_user_from_cookie(req.cookie.as_deref())
}

fn current_user_from_cookie(cookie: Option<&str>) -> Result<Option<User>, DbError> {
    let Some(token) = session_token_from_cookie(cookie) else {
        return Ok(None);
    };

    db::auth_store().user_by_session(&token, unix_secs() as i64)
}

pub fn app_state_json(req: &Request) -> Result<serde_json::Value, DbError> {
    let Some(user) = current_user(req)? else {
        return Err(DbError::InvalidConfig("unauthenticated".into()));
    };

    let store = db::auth_store();
    let usage = store.usage_for_day(user.id, &current_usage_day())?;
    let chats = store.recent_chats(user.id, 30)?;
    let limits = tier_limits(&user.tier);

    Ok(json!({
        "user": {
            "tier": user.tier,
            "name": user.name,
            "email": user.email,
            "usage": usage_json(&usage),
        },
        "limits": limits_json(limits),
        "recent_chats": chats.iter().map(chat_json).collect::<Vec<_>>(),
    }))
}

fn verify_login(email: &str, password: &str) -> Result<Option<User>, DbError> {
    let normalized = normalize_email(email).map_err(DbError::InvalidConfig)?;
    let Some(row) = db::auth_store().find_user_by_email(&normalized)? else {
        return Ok(None);
    };

    if verify_password(password, &row.password_hash) {
        Ok(Some(row.user))
    } else {
        Ok(None)
    }
}

fn create_user(name: &str, email: &str, password: &str) -> Result<User, DbError> {
    if password.len() < 8 {
        return Err(DbError::InvalidConfig(
            "password must be at least 8 characters".into(),
        ));
    }

    let normalized = normalize_email(email).map_err(DbError::InvalidConfig)?;
    let password_hash = hash_password(password);
    db::auth_store().create_user(name.trim(), &normalized, &password_hash)
}

fn parse_auth_input(req: &Request) -> Result<AuthInput, &'static str> {
    let body = String::from_utf8_lossy(&req.body);

    let mut input = if req
        .content_type
        .as_deref()
        .unwrap_or_default()
        .contains("application/json")
    {
        serde_json::from_str::<AuthInput>(&body).map_err(|_| "invalid json body")?
    } else {
        let mut email = String::new();
        let mut password = String::new();
        let mut name = None;

        for (key, value) in parse_form(&body) {
            match key.as_str() {
                "email" => email = value,
                "password" => password = value,
                "name" => name = Some(value),
                _ => {}
            }
        }

        AuthInput {
            email,
            password,
            name,
        }
    };

    input.email = input.email.trim().to_string();
    if input.email.is_empty() || input.password.is_empty() {
        return Err("email and password are required");
    }

    Ok(input)
}

fn parse_chat_input(req: &Request) -> Result<ChatInput, &'static str> {
    let body = String::from_utf8_lossy(&req.body);
    if req
        .content_type
        .as_deref()
        .unwrap_or_default()
        .contains("application/json")
    {
        serde_json::from_str::<ChatInput>(&body).map_err(|_| "invalid json body")
    } else {
        let mut message = String::new();
        let mut model = None;

        for (key, value) in parse_form(&body) {
            match key.as_str() {
                "message" => message = value,
                "model" => model = Some(value),
                _ => {}
            }
        }

        Ok(ChatInput {
            message,
            model,
            attachments: Vec::new(),
        })
    }
}

fn display_message_for(message: &str, attachments: &[ChatAttachmentInput]) -> String {
    let message = message.trim();
    if attachments.is_empty() {
        return message.to_string();
    }
    let suffix = if attachments.len() == 1 {
        format!("Attached file: {}", attachments[0].name.trim())
    } else {
        format!("Attached files: {}", attachments.len())
    };
    if message.is_empty() {
        suffix
    } else {
        format!("{message}\n\n{suffix}")
    }
}

fn prepare_user_message(
    message: &str,
    attachments: &[ChatAttachmentInput],
) -> Result<String, String> {
    validate_attachments(attachments)?;
    let mut out = message.trim().to_string();
    if attachments.is_empty() {
        return Ok(out);
    }

    let payload = attachments
        .iter()
        .map(|attachment| {
            json!({
                "name": attachment.name.trim(),
                "mime_type": attachment.mime_type.trim(),
                "size": attachment.size,
                "encoding": attachment.encoding.trim(),
                "content": attachment.content,
            })
        })
        .collect::<Vec<_>>();

    if !out.is_empty() {
        out.push_str("\n\n");
    }
    out.push_str("--- MIXER_ATTACHMENTS_JSON ---\n");
    out.push_str(
        &serde_json::to_string_pretty(&payload)
            .map_err(|_| "attachments could not be serialized".to_string())?,
    );
    out.push_str("\n--- END_MIXER_ATTACHMENTS_JSON ---");
    Ok(out)
}

fn validate_attachments(attachments: &[ChatAttachmentInput]) -> Result<(), String> {
    const MAX_ATTACHMENTS: usize = 8;
    const MAX_TOTAL_CHARS: usize = 1_500_000;
    if attachments.len() > MAX_ATTACHMENTS {
        return Err(format!(
            "too many attachments; maximum is {MAX_ATTACHMENTS}"
        ));
    }
    let mut total = 0usize;
    for attachment in attachments {
        if attachment.name.trim().is_empty() {
            return Err("attachment name is required".to_string());
        }
        if attachment.size > 2_000_000 {
            return Err(format!("attachment '{}' is too large", attachment.name));
        }
        total = total.saturating_add(attachment.content.chars().count());
        if total > MAX_TOTAL_CHARS {
            return Err("attachments are too large for one message".to_string());
        }
    }
    Ok(())
}

fn parse_model_input(req: &Request) -> Result<ModelInput, &'static str> {
    let body = String::from_utf8_lossy(&req.body);
    if req
        .content_type
        .as_deref()
        .unwrap_or_default()
        .contains("application/json")
    {
        serde_json::from_str::<ModelInput>(&body).map_err(|_| "invalid json body")
    } else {
        let model = parse_form(&body)
            .into_iter()
            .find_map(|(key, value)| (key == "model").then_some(value));
        Ok(ModelInput { model })
    }
}

fn parse_form(body: &str) -> Vec<(String, String)> {
    body.split('&')
        .map(|pair| {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            (percent_decode(key), percent_decode(value))
        })
        .collect()
}

fn percent_decode(value: &str) -> String {
    let value = value.replace('+', " ");
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(hex) = u8::from_str_radix(&value[i + 1..i + 3], 16) {
                out.push(hex);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }

    String::from_utf8_lossy(&out).into_owned()
}

fn normalize_email(email: &str) -> Result<String, String> {
    let email = email.trim().to_ascii_lowercase();
    if email.contains('@') && email.len() <= 254 {
        Ok(email)
    } else {
        Err("valid email is required".into())
    }
}

fn hash_password(password: &str) -> String {
    let salt = random_bytes(16);
    let mut hasher = Sha256::new();
    hasher.update(&salt);
    hasher.update(password.as_bytes());
    format!("{}:{}", to_hex(&salt), to_hex(&hasher.finalize()))
}

fn verify_password(password: &str, stored: &str) -> bool {
    let Some((salt_hex, hash_hex)) = stored.split_once(':') else {
        return false;
    };
    let Some(salt) = from_hex(salt_hex) else {
        return false;
    };

    let mut hasher = Sha256::new();
    hasher.update(&salt);
    hasher.update(password.as_bytes());
    constant_eq(to_hex(&hasher.finalize()).as_bytes(), hash_hex.as_bytes())
}

fn create_session(user_id: i64) -> Result<String, DbError> {
    let token = random_token();
    let expires_at = unix_secs() as i64 + SESSION_TTL_SECS as i64;
    db::auth_store().create_session(&token, user_id, expires_at)?;
    Ok(token)
}

fn session_token(req: &Request) -> Option<String> {
    session_token_from_cookie(req.cookie.as_deref())
}

fn session_token_from_cookie(cookie: Option<&str>) -> Option<String> {
    cookie?
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find_map(|(key, value)| (key == SESSION_COOKIE).then(|| value.to_string()))
}

fn session_cookie(token: &str) -> String {
    format!("{SESSION_COOKIE}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={SESSION_TTL_SECS}")
}

fn expired_session_cookie() -> String {
    format!("{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0")
}

fn user_json(user: &User) -> serde_json::Value {
    json!({
        "id": user.id,
        "name": user.name,
        "email": user.email,
        "tier": user.tier,
        "initials": initials(&user.name),
        "role": "Member",
    })
}

fn usage_json(usage: &db::Usage) -> serde_json::Value {
    json!({
        "messages": usage.messages,
        "tokens": usage.tokens,
        "last_updated": if usage.last_updated > 0 {
            iso_from_unix(usage.last_updated)
        } else {
            iso_from_unix(unix_secs() as i64)
        },
    })
}

fn chat_json(chat: &db::ChatSummary) -> serde_json::Value {
    json!({
        "id": chat.uuid,
        "title": chat.title,
        "preview": chat.preview,
        "updated_at": iso_from_unix(chat.updated_at),
    })
}

fn message_json(message: &db::ChatMessage) -> serde_json::Value {
    json!({
        "id": message.uuid,
        "role": message.role,
        "content": message.content,
        "created_at": iso_from_unix(message.created_at),
        "model": message.model,
    })
}

fn visible_messages(messages: Vec<db::ChatMessage>) -> Vec<db::ChatMessage> {
    messages
        .into_iter()
        .filter(|message| !is_retired_placeholder(message))
        .collect()
}

fn is_retired_placeholder(message: &db::ChatMessage) -> bool {
    message.role == "assistant"
        && message
            .content
            .trim()
            .eq_ignore_ascii_case(RETIRED_PLACEHOLDER_REPLY)
}

fn needs_agent_continuation(processed: &str) -> bool {
    if !processed.contains("<tool_result") {
        return false;
    }
    if processed.contains("<response>") {
        return false;
    }
    // <end> is an explicit termination signal — stop the loop
    if processed.contains("<tool_result tool=\"end\"") {
        return false;
    }
    let Some(last_close_pos) = processed.rfind("</tool_result>") else {
        return false;
    };
    let tail = processed[last_close_pos + "</tool_result>".len()..].trim();
    tail.len() < 80
}

fn stream_assistant_response(
    user_id: i64,
    chat_uuid: String,
    chat: db::ChatSummary,
    user_message: db::ChatMessage,
    turns: Vec<ChatTurn>,
    model: String,
    usage: db::Usage,
    limits: TierLimits,
) -> Response {
    let transcript = turns
        .iter()
        .map(|turn| format!("{}: {}", turn.role, turn.content))
        .collect::<Vec<_>>()
        .join("\n");
    let (client_tx, client_rx) = std::sync::mpsc::channel::<String>();

    let user_msg_uuid = user_message.uuid.clone();
    let chat_clone = chat.clone();
    let user_msg_clone = user_message.clone();
    let model_clone = model.clone();

    std::thread::spawn(move || {
        let _ = client_tx.send(sse(
            "meta",
            json!({
                "chat": chat_json(&chat_clone),
                "user_message": message_json(&user_msg_clone),
                "usage": usage_json(&usage),
                "limits": limits_json(limits),
                "model": model_clone,
            }),
        ));

        // Subscribe BEFORE spawning so we don't miss the first events.
        let (run_tx, run_rx) = std::sync::mpsc::channel::<String>();
        let run_uuid = {
            let uuid = run::spawn_run(
                user_id,
                chat_uuid.clone(),
                user_msg_uuid.clone(),
                transcript,
                turns,
                model_clone.clone(),
            );
            run::subscribe(&uuid, run_tx);
            uuid
        };

        // Tell the client the run uuid so it can reconnect on refresh.
        let _ = client_tx.send(sse("run_started", json!({ "run_uuid": run_uuid })));

        // Pure structured-event forwarding. The frontend reads agent_event
        // envelopes and renders from those. No legacy delta/replace synthesis.
        let mut final_text: Option<String> = None;
        let mut assistant_message: Option<db::ChatMessage> = None;
        let mut run_error: Option<String> = None;
        let mut run_cancelled = false;
        let mut last_seq = 0i64;

        if let Ok(events) = db::auth_store().events_for_run_since(&run_uuid, 0) {
            for event in events {
                let payload = serde_json::from_str::<serde_json::Value>(&event.payload_json)
                    .unwrap_or_else(|_| json!({}));
                let envelope = json!({
                    "seq": event.seq,
                    "type": event.event_type,
                    "payload": payload,
                    "created_at": event.created_at,
                });
                last_seq = last_seq.max(event.seq);
                let _ = client_tx.send(sse("agent_event", envelope.clone()));
                let event_type = envelope.get("type").and_then(|v| v.as_str()).unwrap_or("");
                if matches!(event_type, "run_completed" | "run_failed" | "run_cancelled") {
                    break;
                }
            }
        }

        loop {
            let envelope_str = match run_rx.recv() {
                Ok(s) if s.is_empty() => continue,
                Ok(s) => s,
                Err(_) => break,
            };

            let envelope: serde_json::Value = match serde_json::from_str(&envelope_str) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let seq = envelope.get("seq").and_then(|v| v.as_i64()).unwrap_or(0);
            if seq <= last_seq {
                continue;
            }
            last_seq = seq;

            // Forward verbatim.
            let _ = client_tx.send(sse("agent_event", envelope.clone()));

            let event_type = envelope.get("type").and_then(|v| v.as_str()).unwrap_or("");
            let payload = envelope.get("payload").cloned().unwrap_or(json!({}));

            match event_type {
                "run_completed" => {
                    if let Some(t) = payload.get("final_text").and_then(|v| v.as_str()) {
                        final_text = Some(t.to_string());
                    }
                    if let Some(msg_uuid) = payload
                        .get("assistant_message_uuid")
                        .and_then(|v| v.as_str())
                    {
                        if let Ok(msgs) = db::auth_store().chat_messages(user_id, &chat_uuid) {
                            assistant_message = msgs.into_iter().find(|m| m.uuid == msg_uuid);
                        }
                    }
                    break;
                }
                "run_failed" => {
                    let err = payload
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("run failed");
                    run_error = Some(err.to_string());
                    break;
                }
                "run_cancelled" => {
                    run_cancelled = true;
                    run_error = Some(
                        payload
                            .get("reason")
                            .and_then(|v| v.as_str())
                            .unwrap_or("Stopped by user")
                            .to_string(),
                    );
                    break;
                }
                _ => {}
            }
        }

        let final_message = final_text
            .or_else(|| {
                run_error.clone().map(|e| {
                    if e == "Stopped by user" {
                        "Stopped by user".to_string()
                    } else {
                        format!("Run failed: {e}")
                    }
                })
            })
            .unwrap_or_default();
        let assistant_msg = if run_cancelled {
            None
        } else {
            Some(assistant_message.unwrap_or_else(|| db::ChatMessage {
                uuid: String::new(),
                role: "assistant".into(),
                content: final_message,
                created_at: unix_secs() as i64,
                model: Some(model_clone.clone()),
            }))
        };
        if let Some(err) = run_error.filter(|err| err != "Stopped by user") {
            let _ = client_tx.send(sse("error", json!({ "error": err })));
        }
        let _ = client_tx.send(sse(
            "done",
            json!({ "assistant_message": assistant_msg.as_ref().map(message_json) }),
        ));
    });

    Response::event_stream(client_rx)
}

fn sse(event: &str, data: serde_json::Value) -> String {
    format!("event: {event}\ndata: {}\n\n", data)
}

fn persist_assistant_or_error(
    user_id: i64,
    chat_uuid: &str,
    content: &str,
    model: &str,
) -> Option<db::ChatMessage> {
    match db::auth_store().append_message(
        user_id,
        chat_uuid,
        "assistant",
        content,
        unix_secs() as i64,
        Some(model),
    ) {
        Ok(message) => Some(message),
        Err(err) => {
            // Previously this used `.ok()` — DB write failures were silently
            // swallowed, which is exactly how "messages disappear after
            // refresh" symptoms slip through unnoticed. Log loudly so we can
            // see the failure in dev (and fix the schema/connection if it
            // happens in prod).
            eprintln!(
                "[chat/persist] FAILED to save assistant message for chat={chat_uuid}: {err}"
            );
            None
        }
    }
}

fn reserve_usage(user: &User, message: &str) -> Result<(db::Usage, TierLimits, i64), Response> {
    let day = current_usage_day();
    let store = db::auth_store();
    let usage = store.usage_for_day(user.id, &day).map_err(server_error)?;
    let limits = tier_limits(&user.tier);
    let estimated_tokens = estimate_tokens(message);

    if usage.messages >= limits.messages {
        return Err(Response::json(
            json!({
                "ok": false,
                "error": "message limit reached",
                "resets_at": iso_from_unix(next_reset_unix()),
            })
            .to_string(),
        )
        .with_status(429));
    }

    if usage.tokens + estimated_tokens > limits.tokens {
        return Err(Response::json(
            json!({
                "ok": false,
                "error": "token limit reached",
                "resets_at": iso_from_unix(next_reset_unix()),
            })
            .to_string(),
        )
        .with_status(429));
    }

    let now = unix_secs() as i64;
    let updated_usage = store
        .increment_usage(user.id, &day, 1, estimated_tokens, now)
        .map_err(server_error)?;

    Ok((updated_usage, limits, now))
}

#[derive(Clone, Copy)]
struct TierLimits {
    messages: i64,
    tokens: i64,
}

fn tier_limits(tier: &str) -> TierLimits {
    match tier {
        "enterprise" => TierLimits {
            messages: 5000,
            tokens: 5_000_000,
        },
        "pro" => TierLimits {
            messages: 500,
            tokens: 500_000,
        },
        _ => TierLimits {
            messages: 25,
            tokens: 25_000,
        },
    }
}

fn limits_json(limits: TierLimits) -> serde_json::Value {
    json!({
        "messages": limits.messages,
        "tokens": limits.tokens,
        "resets_at": iso_from_unix(next_reset_unix()),
    })
}

fn current_usage_day() -> String {
    let now = Utc::now();
    format!("{:04}-{:02}-{:02}", now.year(), now.month(), now.day())
}

fn next_reset_unix() -> i64 {
    let now = Utc::now();
    Utc.with_ymd_and_hms(now.year(), now.month(), now.day(), 0, 0, 0)
        .single()
        .map(|start| start + chrono::Duration::days(1))
        .unwrap_or(now)
        .timestamp()
}

fn iso_from_unix(timestamp: i64) -> String {
    Utc.timestamp_opt(timestamp, 0)
        .single()
        .unwrap_or_else(Utc::now)
        .to_rfc3339()
}

fn estimate_tokens(message: &str) -> i64 {
    ((message.chars().count() as i64 + 3) / 4).max(1) + 16
}

fn chat_title(message: &str) -> String {
    let mut title: String = message.chars().take(56).collect();
    if message.chars().count() > 56 {
        title.push_str("...");
    }
    title
}

fn initials(name: &str) -> String {
    let initials: String = name
        .split_ascii_whitespace()
        .filter_map(|part| part.chars().next())
        .take(2)
        .collect();

    if initials.is_empty() {
        "U".into()
    } else {
        initials.to_ascii_uppercase()
    }
}

fn auth_error(status: u16, message: impl Into<String>) -> Response {
    Response::json(json!({ "ok": false, "error": message.into() }).to_string()).with_status(status)
}

fn server_error(message: impl ToString) -> Response {
    Response::json(json!({ "ok": false, "error": message.to_string() }).to_string())
        .with_status(500)
}

fn random_token() -> String {
    to_hex(&random_bytes(32))
}

fn random_uuid_v4() -> String {
    let mut bytes = random_bytes(16);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0],
        bytes[1],
        bytes[2],
        bytes[3],
        bytes[4],
        bytes[5],
        bytes[6],
        bytes[7],
        bytes[8],
        bytes[9],
        bytes[10],
        bytes[11],
        bytes[12],
        bytes[13],
        bytes[14],
        bytes[15]
    )
}

fn is_uuid_like(value: &str) -> bool {
    value.len() == 36
        && value.chars().enumerate().all(|(i, c)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                c == '-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

fn random_bytes(len: usize) -> Vec<u8> {
    let mut bytes = vec![0u8; len];
    if File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut bytes))
        .is_ok()
    {
        return bytes;
    }

    let mut hasher = Sha256::new();
    hasher.update(unix_nanos().to_le_bytes());
    hasher.update(std::process::id().to_le_bytes());
    let digest = hasher.finalize();
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = digest[i % digest.len()];
    }
    bytes
}

fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn from_hex(hex: &str) -> Option<Vec<u8>> {
    if hex.len() % 2 != 0 {
        return None;
    }

    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

fn constant_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }

    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn unix_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

// ── Settings page handlers ───────────────────────────────────────────────────

const MAX_CUSTOM_INSTRUCTIONS_LEN: usize = 2000;
const MAX_DISPLAY_NAME_LEN: usize = 80;

/// GET /api/settings — returns the user's profile + personalization.
pub async fn get_settings(req: &Request) -> Response {
    let user = match current_user(req) {
        Ok(Some(user)) => user,
        Ok(None) => return auth_error(401, "unauthenticated"),
        Err(e) => return server_error(e),
    };
    let settings = match db::auth_store().get_user_settings(user.id) {
        Ok(s) => s,
        Err(e) => return server_error(e),
    };
    let characteristics: serde_json::Value =
        serde_json::from_str(&settings.characteristics_json).unwrap_or_else(|_| json!({}));
    Response::json(
        json!({
            "ok": true,
            "user": user_json(&user),
            "settings": {
                "base_style": settings.base_style,
                "characteristics": characteristics,
                "custom_instructions": settings.custom_instructions,
                "updated_at": settings.updated_at,
            },
        })
        .to_string(),
    )
}

/// PUT /api/settings — updates personalization.
/// Body: { base_style?, characteristics?, custom_instructions? }
pub async fn put_settings(req: &Request) -> Response {
    let user = match current_user(req) {
        Ok(Some(user)) => user,
        Ok(None) => return auth_error(401, "unauthenticated"),
        Err(e) => return server_error(e),
    };

    #[derive(serde::Deserialize)]
    struct Input {
        base_style: Option<String>,
        characteristics: Option<serde_json::Value>,
        custom_instructions: Option<String>,
    }

    let body = String::from_utf8_lossy(&req.body);
    let input: Input = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => return auth_error(400, "invalid json body"),
    };

    let store = db::auth_store();
    let mut settings = match store.get_user_settings(user.id) {
        Ok(s) => s,
        Err(e) => return server_error(e),
    };

    if let Some(style) = input.base_style {
        let style = style.trim().to_string();
        if !is_valid_base_style(&style) {
            return auth_error(400, "invalid base_style");
        }
        settings.base_style = style;
    }
    if let Some(value) = input.characteristics {
        // Only persist if it's a JSON object; reject arrays/scalars.
        if !value.is_object() {
            return auth_error(400, "characteristics must be a JSON object");
        }
        settings.characteristics_json = value.to_string();
    }
    if let Some(mut text) = input.custom_instructions {
        if text.len() > MAX_CUSTOM_INSTRUCTIONS_LEN {
            text.truncate(MAX_CUSTOM_INSTRUCTIONS_LEN);
        }
        settings.custom_instructions = text;
    }
    settings.updated_at = unix_secs() as i64;

    match store.upsert_user_settings(user.id, &settings) {
        Ok(()) => Response::json(json!({ "ok": true }).to_string()),
        Err(e) => server_error(e),
    }
}

/// PATCH /api/user/name — change the display name on the current user.
pub async fn update_name(req: &Request) -> Response {
    let user = match current_user(req) {
        Ok(Some(user)) => user,
        Ok(None) => return auth_error(401, "unauthenticated"),
        Err(e) => return server_error(e),
    };

    #[derive(serde::Deserialize)]
    struct Input {
        name: String,
    }
    let body = String::from_utf8_lossy(&req.body);
    let input: Input = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => return auth_error(400, "invalid json body"),
    };

    let trimmed = input.name.trim();
    if trimmed.is_empty() {
        return auth_error(400, "name cannot be empty");
    }
    if trimmed.chars().count() > MAX_DISPLAY_NAME_LEN {
        return auth_error(400, "name too long");
    }

    match db::auth_store().update_user_name(user.id, trimmed) {
        Ok(()) => Response::json(json!({ "ok": true, "name": trimmed }).to_string()),
        Err(e) => server_error(e),
    }
}

/// DELETE /api/chats — wipes ALL chat history for the current user.
pub async fn delete_all_chats(req: &Request) -> Response {
    let user = match current_user(req) {
        Ok(Some(user)) => user,
        Ok(None) => return auth_error(401, "unauthenticated"),
        Err(e) => return server_error(e),
    };
    match db::auth_store().delete_all_chats_for_user(user.id) {
        Ok(count) => Response::json(json!({ "ok": true, "deleted": count }).to_string()),
        Err(e) => server_error(e),
    }
}

/// DELETE /api/workspaces — removes ALL workspaces (DB rows + on-disk files)
/// for the current user.
pub async fn delete_all_workspaces(req: &Request) -> Response {
    let user = match current_user(req) {
        Ok(Some(user)) => user,
        Ok(None) => return auth_error(401, "unauthenticated"),
        Err(e) => return server_error(e),
    };
    let store = db::auth_store();

    // List first so we know which dirs to remove on disk, then drop DB rows.
    let workspaces = match store.list_workspaces(user.id) {
        Ok(list) => list,
        Err(e) => return server_error(e),
    };

    for ws in &workspaces {
        let dir = crate::workspace::workspace_dir(user.id, &ws.uuid);
        let _ = std::fs::remove_dir_all(&dir); // best-effort
    }

    let count = match store.delete_all_workspaces_for_user(user.id) {
        Ok(c) => c,
        Err(e) => return server_error(e),
    };

    Response::json(json!({ "ok": true, "deleted": count }).to_string())
}

fn is_valid_base_style(value: &str) -> bool {
    matches!(
        value,
        "default" | "concise" | "detailed" | "friendly" | "professional" | "witty" | "direct"
    )
}
