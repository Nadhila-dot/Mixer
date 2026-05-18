use crate::{
    auth, db,
    router::{Request, Response},
};
use serde::Deserialize;
use serde_json::json;
use std::{io::Read, path::Path, sync::OnceLock, time::SystemTime};

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
        Ok(content) => {
            Response::json(json!({ "ok": true, "path": path, "content": content }).to_string())
        }
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
    let path = query_param(req.query.as_deref(), "path").unwrap_or_else(|| "index.html".into());
    if !is_public_preview_path(&path) {
        return json_error(400, "path is not previewable");
    }
    let Some(dir) = public_workspace_dir_for(workspace_id) else {
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

pub async fn workspace_port_proxy(
    req: &Request,
    workspace_id: &str,
    port: u16,
    tail: &str,
) -> Response {
    let Some(user) = authed_user(req) else {
        return json_error(401, "unauthenticated");
    };
    if !(1024..=65535).contains(&port) {
        return json_error(400, "invalid port");
    }
    let Some(_dir) = workspace_dir_for(user.id, workspace_id) else {
        return json_error(404, "workspace not found");
    };

    let target_path = if tail.is_empty() { "/" } else { tail };
    let query = req
        .query
        .as_deref()
        .map(|q| format!("?{q}"))
        .unwrap_or_default();
    let url = format!("http://127.0.0.1:{port}{target_path}{query}");

    let response = match ureq::get(&url).call() {
        Ok(response) => response,
        Err(ureq::Error::Status(code, response)) => {
            let content_type = response
                .header("content-type")
                .unwrap_or("text/plain; charset=utf-8")
                .to_string();
            let mut bytes = Vec::new();
            let _ = response.into_reader().read_to_end(&mut bytes);
            return Response::bytes(bytes, content_type)
                .with_status(code)
                .with_header("Cache-Control", "no-store");
        }
        Err(error) => return json_error(502, &format!("preview proxy failed: {error}")),
    };

    let content_type = response
        .header("content-type")
        .unwrap_or("application/octet-stream")
        .to_string();
    let mut bytes = Vec::new();
    match response.into_reader().read_to_end(&mut bytes) {
        Ok(_) => Response::bytes(bytes, content_type).with_header("Cache-Control", "no-store"),
        Err(error) => json_error(502, &format!("could not read preview response: {error}")),
    }
}

pub async fn workspace_download(req: &Request, workspace_id: &str) -> Response {
    let Some(user) = authed_user(req) else {
        return json_error(401, "unauthenticated");
    };
    let Some(dir) = workspace_dir_for(user.id, workspace_id) else {
        return json_error(404, "workspace not found");
    };

    match workspace_zip(&dir) {
        Ok(bytes) => Response::bytes(bytes, "application/zip".into())
            .with_header(
                "Content-Disposition",
                format!("attachment; filename=\"{workspace_id}.zip\""),
            )
            .with_header("Cache-Control", "no-store"),
        Err(error) => json_error(500, &error),
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

fn public_workspace_dir_for(workspace_id: &str) -> Option<std::path::PathBuf> {
    match db::auth_store().get_workspace_any(workspace_id) {
        Ok(Some(_)) => {
            let root = crate::workspace::workspaces_root();
            let users = std::fs::read_dir(&root).ok()?;
            for user in users.flatten() {
                let candidate = user.path().join(workspace_id);
                if candidate.is_dir() {
                    return Some(candidate);
                }
            }
            None
        }
        _ => None,
    }
}

fn is_public_preview_path(path: &str) -> bool {
    let path = path.to_ascii_lowercase();
    path.ends_with(".html")
        || path.ends_with(".htm")
        || path.ends_with(".svg")
        || path.ends_with(".md")
        || path.ends_with(".markdown")
        || path.ends_with(".mdx")
}

fn json_error(status: u16, error: &str) -> Response {
    Response::json(json!({ "ok": false, "error": error }).to_string()).with_status(status)
}

fn workspace_zip(root: &Path) -> Result<Vec<u8>, String> {
    let mut files = Vec::<(String, Vec<u8>)>::new();
    collect_zip_files(root, root, &mut files)?;
    build_zip(files)
}

fn collect_zip_files(
    root: &Path,
    dir: &Path,
    out: &mut Vec<(String, Vec<u8>)>,
) -> Result<(), String> {
    let entries = std::fs::read_dir(dir).map_err(|e| format!("could not read workspace: {e}"))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("could not inspect workspace entry: {e}"))?;
        let path = entry.path();
        let meta =
            std::fs::symlink_metadata(&path).map_err(|e| format!("could not inspect file: {e}"))?;
        if meta.file_type().is_symlink() {
            continue;
        }
        if meta.is_dir() {
            collect_zip_files(root, &path, out)?;
            continue;
        }
        if !meta.is_file() {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .map_err(|_| "workspace path escaped root".to_string())?
            .to_string_lossy()
            .replace('\\', "/");
        let bytes = std::fs::read(&path).map_err(|e| format!("could not read {rel}: {e}"))?;
        out.push((rel, bytes));
    }
    Ok(())
}

fn build_zip(files: Vec<(String, Vec<u8>)>) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    let mut entries = 0u16;
    for (name, data) in files {
        entries = entries
            .checked_add(1)
            .ok_or_else(|| "too many files for zip".to_string())?;
        let offset =
            u32::try_from(out.len()).map_err(|_| "workspace zip is too large".to_string())?;
        let crc = crc32(&data);
        let name_bytes = name.as_bytes();
        let name_len =
            u16::try_from(name_bytes.len()).map_err(|_| format!("file name too long: {name}"))?;
        let size =
            u32::try_from(data.len()).map_err(|_| format!("file too large for zip: {name}"))?;

        write_u32(&mut out, 0x0403_4b50);
        write_u16(&mut out, 20);
        write_u16(&mut out, 0);
        write_u16(&mut out, 0);
        write_u16(&mut out, 0);
        write_u16(&mut out, 0);
        write_u32(&mut out, crc);
        write_u32(&mut out, size);
        write_u32(&mut out, size);
        write_u16(&mut out, name_len);
        write_u16(&mut out, 0);
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(&data);

        write_u32(&mut central, 0x0201_4b50);
        write_u16(&mut central, 20);
        write_u16(&mut central, 20);
        write_u16(&mut central, 0);
        write_u16(&mut central, 0);
        write_u16(&mut central, 0);
        write_u16(&mut central, 0);
        write_u32(&mut central, crc);
        write_u32(&mut central, size);
        write_u32(&mut central, size);
        write_u16(&mut central, name_len);
        write_u16(&mut central, 0);
        write_u16(&mut central, 0);
        write_u16(&mut central, 0);
        write_u16(&mut central, 0);
        write_u32(&mut central, 0);
        write_u32(&mut central, offset);
        central.extend_from_slice(name_bytes);
    }

    let central_offset =
        u32::try_from(out.len()).map_err(|_| "workspace zip is too large".to_string())?;
    let central_size =
        u32::try_from(central.len()).map_err(|_| "workspace zip is too large".to_string())?;
    out.extend_from_slice(&central);
    write_u32(&mut out, 0x0605_4b50);
    write_u16(&mut out, 0);
    write_u16(&mut out, 0);
    write_u16(&mut out, entries);
    write_u16(&mut out, entries);
    write_u32(&mut out, central_size);
    write_u32(&mut out, central_offset);
    write_u16(&mut out, 0);
    Ok(out)
}

fn write_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn write_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for byte in bytes {
        crc ^= *byte as u32;
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
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
