use crate::{auth, handlers::pages};
use serde_json::json;
use std::sync::mpsc::Receiver;

// ── Request / Response types ────────────────────────────────────────────────

#[allow(dead_code)]
pub struct Request {
    pub method: String,
    pub path: String,
    pub query: Option<String>,
    pub host: Option<String>,
    pub cookie: Option<String>,
    pub content_type: Option<String>,
    pub sec_websocket_key: Option<String>,
    pub body: Vec<u8>,
    /// Raw value of the HTTP Range header, e.g. "bytes=0-1048575"
    pub range: Option<String>,
}

pub struct Response {
    pub status: u16,
    pub content_type: String,
    pub body: ResponseBody,
    pub extra_headers: Vec<(String, String)>,
}

pub enum ResponseBody {
    Fixed(Vec<u8>),
    Stream(Receiver<String>),
}

impl Response {
    pub fn html(body: impl Into<Vec<u8>>) -> Self {
        Self {
            status: 200,
            content_type: "text/html; charset=utf-8".into(),
            body: ResponseBody::Fixed(body.into()),
            extra_headers: vec![],
        }
    }

    pub fn json(body: impl Into<Vec<u8>>) -> Self {
        Self {
            status: 200,
            content_type: "application/json".into(),
            body: ResponseBody::Fixed(body.into()),
            extra_headers: vec![],
        }
    }

    pub fn bytes(body: Vec<u8>, mime: String) -> Self {
        Self {
            status: 200,
            content_type: mime,
            body: ResponseBody::Fixed(body),
            extra_headers: vec![],
        }
    }

    pub fn event_stream(chunks: Receiver<String>) -> Self {
        Self {
            status: 200,
            content_type: "text/event-stream; charset=utf-8".into(),
            body: ResponseBody::Stream(chunks),
            extra_headers: vec![
                ("Cache-Control".into(), "no-cache".into()),
                ("X-Accel-Buffering".into(), "no".into()),
            ],
        }
    }

    pub fn not_found() -> Self {
        Self {
            status: 404,
            content_type: "text/plain".into(),
            body: ResponseBody::Fixed(b"404 not found".to_vec()),
            extra_headers: vec![],
        }
    }

    pub fn redirect(location: impl Into<String>) -> Self {
        let location = location.into();
        Self {
            status: 302,
            content_type: "text/plain".into(),
            body: ResponseBody::Fixed(format!("Redirecting to {location}").into_bytes()),
            extra_headers: vec![("Location".into(), location)],
        }
    }

    #[allow(dead_code)]
    pub fn with_status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }

    pub fn with_header(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.extra_headers.push((key.into(), value.into()));
        self
    }
}

// ── Dispatch ────────────────────────────────────────────────────────────────

pub async fn dispatch(req: &Request) -> Response {
    if !is_public_route(req) {
        match auth::current_user(req) {
            Ok(Some(_)) => {}
            Ok(None) if req.path.starts_with("/api/") => {
                return Response::json(
                    json!({ "ok": false, "error": "unauthenticated" }).to_string(),
                )
                .with_status(401);
            }
            Ok(None) => return Response::redirect(auth_redirect(req)),
            Err(e) if req.path.starts_with("/api/") => {
                return Response::json(json!({ "ok": false, "error": e.to_string() }).to_string())
                    .with_status(500);
            }
            Err(e) => {
                return Response::json(json!({ "ok": false, "error": e.to_string() }).to_string())
                    .with_status(500);
            }
        }
    }

    match (req.method.as_str(), req.path.as_str()) {
        // Video / large public assets — streamed from disk with Range support
        ("GET", path) if path.starts_with("/background/") => {
            pages::assets::disk_file(path, req.range.as_deref()).await
        }

        // Vite-hashed static chunks
        ("GET", path) if is_asset(path) => pages::assets::asset(path).await,

        // JSON API
        ("GET", "/api/stats")  => pages::api::stats().await,
        ("GET", "/api/health") => pages::api::health().await,
        ("GET", "/api/app-state") => auth::app_state(req).await,
        ("GET", "/api/models") => auth::models().await,
        ("GET", path) if path.starts_with("/api/workspaces/") && path.ends_with("/file") => {
            let workspace_id = path
                .trim_start_matches("/api/workspaces/")
                .trim_end_matches("/file");
            pages::api::workspace_file(req, workspace_id).await
        }
        ("PUT", path) if path.starts_with("/api/workspaces/") && path.ends_with("/file") => {
            let workspace_id = path
                .trim_start_matches("/api/workspaces/")
                .trim_end_matches("/file");
            pages::api::save_workspace_file(req, workspace_id).await
        }
        ("GET", path) if path.starts_with("/api/workspaces/") && path.ends_with("/preview") => {
            let workspace_id = path
                .trim_start_matches("/api/workspaces/")
                .trim_end_matches("/preview");
            pages::api::workspace_preview(req, workspace_id).await
        }
        ("POST", "/api/chat/start") => auth::start_chat(req).await,
        ("POST", "/api/chat/stream") => auth::stream_new_chat(req).await,
        ("POST", "/api/chat") => auth::chat(req).await,
        ("POST", path) if path.starts_with("/api/runs/") && path.ends_with("/cancel") => {
            let run_uuid = path
                .trim_start_matches("/api/runs/")
                .trim_end_matches("/cancel");
            auth::cancel_run(req, run_uuid).await
        }
        ("GET", path) if path.starts_with("/api/runs/") => {
            let run_uuid = path.trim_start_matches("/api/runs/");
            auth::get_run(req, run_uuid).await
        }
        ("GET", path) if path.starts_with("/api/chats/") && path.ends_with("/runs") => {
            let chat_uuid = path
                .trim_start_matches("/api/chats/")
                .trim_end_matches("/runs");
            auth::list_runs_for_chat(req, chat_uuid).await
        }
        ("GET", path) if path.starts_with("/api/chats/") => {
            // GETs on /api/chats/<uuid>/<anything> should resolve the UUID part
            // cleanly. Without stripping known suffixes, a manual browser hit
            // on /api/chats/<uuid>/assistant-stream lands here with the
            // suffix glued to the UUID and fails validation.
            let chat_path = path.trim_start_matches("/api/chats/");
            let uuid = chat_path
                .strip_suffix("/messages")
                .or_else(|| chat_path.strip_suffix("/stream"))
                .or_else(|| chat_path.strip_suffix("/assistant-stream"))
                .unwrap_or(chat_path);

            // For the streaming endpoints, GET isn't supported — surface 405
            // instead of pretending it's a chat fetch so the browser address
            // bar gives a useful error.
            let is_stream_endpoint = chat_path.ends_with("/stream")
                || chat_path.ends_with("/assistant-stream");
            if is_stream_endpoint {
                return Response::json(
                    json!({ "ok": false, "error": "use POST for streaming endpoints" })
                        .to_string(),
                )
                .with_status(405)
                .with_header("Allow", "POST");
            }

            auth::get_chat(req, uuid).await
        }
        ("POST", path) if path.starts_with("/api/chats/") => {
            let chat_path = path.trim_start_matches("/api/chats/");
            match chat_path.strip_suffix("/messages") {
                Some(uuid) => auth::append_chat_message(req, uuid).await,
                None => match chat_path.strip_suffix("/stream") {
                    Some(uuid) => auth::stream_chat_message(req, uuid).await,
                    None => match chat_path.strip_suffix("/assistant-stream") {
                        Some(uuid) => auth::stream_existing_assistant(req, uuid).await,
                        None => Response::not_found(),
                    },
                },
            }
        }
        ("DELETE", path) if path.starts_with("/api/chats/") => {
            auth::delete_chat(req, path.trim_start_matches("/api/chats/")).await
        }
        ("GET", "/api/auth/me") => auth::me(req).await,
        ("POST", "/api/auth/login") => auth::login(req).await,
        ("POST", "/api/auth/register") => auth::register(req).await,
        ("POST", "/api/auth/logout") => auth::logout(req).await,
        ("GET", "/usr/logout") => auth::logout_redirect(req).await,

        // Settings + profile
        ("GET", "/api/settings") => auth::get_settings(req).await,
        ("PUT", "/api/settings") => auth::put_settings(req).await,
        ("PATCH", "/api/user/name") => auth::update_name(req).await,
        ("DELETE", "/api/chats") => auth::delete_all_chats(req).await,
        ("DELETE", "/api/workspaces") => auth::delete_all_workspaces(req).await,

        // SSR page shells
        ("GET", "/")           => pages::base::home(req).await,
        ("GET", path) if path.starts_with("/chat/") => pages::base::home(req).await,
        ("GET", "/auth/screen") => pages::auth::screen(req).await,

        // SPA catch-all
        ("GET", path) => pages::base::spa_shell(path).await,

        _ => Response::not_found(),
    }
}

fn is_public_route(req: &Request) -> bool {
    req.path.starts_with("/auth/")
        || req.path.starts_with("/api/auth/")
        || req.path == "/usr/logout"
        || req.path.starts_with("/background/")
        || is_asset(&req.path)
}

fn auth_redirect(req: &Request) -> String {
    let next = match &req.query {
        Some(query) => format!("{}?{query}", req.path),
        None => req.path.clone(),
    };
    format!("/auth/screen?next={}", percent_encode(&next))
}

fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'~' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn is_asset(path: &str) -> bool {
    path.starts_with("/assets/")
        || path.starts_with("/fonts/")
        || path == "/favicon.svg"
        || path == "/favicon.ico"
        || path.ends_with(".ico")
        || path.ends_with(".png")
        || path.ends_with(".webmanifest")
        || path.ends_with(".ttf")
        || path.ends_with(".woff")
        || path.ends_with(".woff2")
        || path.ends_with(".otf")
}
