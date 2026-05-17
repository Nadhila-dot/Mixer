use crate::{
    auth,
    db,
    router::{Request, Response},
};
use serde::Deserialize;
use serde_json::json;
use std::{
    sync::OnceLock,
    time::SystemTime,
};

// ── API handlers ─────────────────────────────────────────────────────────────

pub async fn health() -> Response {
    Response::json(json!({ "ok": true }).to_string())
}

pub async fn stats() -> Response {
    let body = json!({
        "version":         env!("CARGO_PKG_VERSION"),
        "runtime":         "monoio",
        "uptime_seconds":  uptime_seconds(),
        "memory_rss_mb":   current_rss_mb(),
    });
    Response::json(body.to_string())
}

#[derive(Deserialize)]
struct SaveFileInput {
    path: String,
    content: String,
}

pub async fn workspace_file(req: &Request, workspace_id: &str) -> Response {
    let Some(user) = authed_user(req) else {
        return json_error(401, "unauthenticated");
    };
    let Some(path) = query_param(req.query.as_deref(), "path") else {
        return json_error(400, "path is required");
    };
    let Some(dir) = workspace_dir_for(user.id, workspace_id) else {
        return json_error(404, "workspace not found");
    };

    match crate::workspace::read_file_raw(&dir, &path) {
        Ok(content) => Response::json(json!({ "ok": true, "path": path, "content": content }).to_string()),
        Err(error) => json_error(400, &error),
    }
}

pub async fn save_workspace_file(req: &Request, workspace_id: &str) -> Response {
    let Some(user) = authed_user(req) else {
        return json_error(401, "unauthenticated");
    };
    let input = match serde_json::from_slice::<SaveFileInput>(&req.body) {
        Ok(input) => input,
        Err(_) => return json_error(400, "invalid json body"),
    };
    let Some(dir) = workspace_dir_for(user.id, workspace_id) else {
        return json_error(404, "workspace not found");
    };

    match crate::workspace::create_file(&dir, &input.path, &input.content) {
        Ok(()) => {
            let tree = crate::workspace::file_tree_json(&dir);
            Response::json(json!({ "ok": true, "path": input.path, "file_tree": tree }).to_string())
        }
        Err(error) => json_error(400, &error),
    }
}

pub async fn workspace_preview(req: &Request, workspace_id: &str) -> Response {
    let Some(user) = authed_user(req) else {
        return json_error(401, "unauthenticated");
    };
    let path = query_param(req.query.as_deref(), "path").unwrap_or_else(|| "index.html".into());
    let Some(dir) = workspace_dir_for(user.id, workspace_id) else {
        return json_error(404, "workspace not found");
    };

    match crate::workspace::read_file_raw(&dir, &path) {
        Ok(content) => {
            let mime = mime_guess::from_path(&path)
                .first_or_octet_stream()
                .essence_str()
                .to_string();
            Response::bytes(content.into_bytes(), mime)
                .with_header("Cache-Control", "no-store")
                .with_header("X-Frame-Options", "SAMEORIGIN")
        }
        Err(error) => json_error(404, &error),
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn authed_user(req: &Request) -> Option<db::User> {
    auth::current_user(req).ok().flatten()
}

fn workspace_dir_for(user_id: i64, workspace_id: &str) -> Option<std::path::PathBuf> {
    match db::auth_store().get_workspace(user_id, workspace_id) {
        Ok(Some(_)) => Some(crate::workspace::workspace_dir(user_id, workspace_id)),
        _ => None,
    }
}

fn json_error(status: u16, error: &str) -> Response {
    Response::json(json!({ "ok": false, "error": error }).to_string()).with_status(status)
}

fn query_param(query: Option<&str>, key: &str) -> Option<String> {
    let query = query?;
    query.split('&').find_map(|part| {
        let (k, v) = part.split_once('=').unwrap_or((part, ""));
        (percent_decode(k) == key).then(|| percent_decode(v))
    })
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

fn uptime_seconds() -> u64 {
    static STARTED_AT: OnceLock<SystemTime> = OnceLock::new();
    SystemTime::now()
        .duration_since(*STARTED_AT.get_or_init(SystemTime::now))
        .unwrap_or_default()
        .as_secs()
}

fn current_rss_mb() -> f64 {
    // On Linux, read /proc/self/statm (page-accurate); elsewhere return zero.
    #[cfg(target_os = "linux")]
    {
        if let Ok(s) = std::fs::read_to_string("/proc/self/statm") {
            if let Some(pages) = s.split_ascii_whitespace().next() {
                if let Ok(n) = pages.parse::<u64>() {
                    return (n * 4096) as f64 / 1_048_576.0;
                }
            }
        }
    }
    0.0
}
