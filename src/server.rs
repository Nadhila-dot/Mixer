/// HTTP/1.1 connection handler — built directly on Monoio's completion I/O.
///
/// Monoio uses io_uring's submission/completion model: buffers are *owned* by
/// the kernel during I/O and returned to us on completion — zero-copy all the
/// way down.
use chrono::Utc;
use monoio::{
    io::{AsyncReadRent, AsyncWriteRentExt},
    net::TcpStream,
};
use serde_json::json;
use serde::Deserialize;
use std::{
    io,
    io::{Read, Write},
    os::fd::{FromRawFd, IntoRawFd},
    sync::mpsc::Receiver,
    time::Duration,
};

use crate::{
    auth,
    router::{self, Request, Response, ResponseBody},
};

const READ_BUF: usize = 16_384;
const MAX_HEADERS: usize = 48;

pub async fn handle(mut stream: TcpStream) -> io::Result<()> {
    let buf = vec![0u8; READ_BUF];
    let (result, buf) = stream.read(buf).await;
    let n = result?;

    if n == 0 {
        return Ok(());
    }

    let mut raw = buf[..n].to_vec();
    read_remaining_body(&mut stream, &mut raw).await?;

    let request = parse_request(&raw)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    if is_websocket_request(&request) {
        return handle_websocket(stream, request).await;
    }

    let response = router::dispatch(&request).await;
    flush(stream, response).await
}

async fn read_remaining_body(stream: &mut TcpStream, raw: &mut Vec<u8>) -> io::Result<()> {
    let Some(header_end) = find_header_end(raw) else {
        return Ok(());
    };

    let headers = String::from_utf8_lossy(&raw[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            if name.eq_ignore_ascii_case("content-length") {
                value.trim().parse::<usize>().ok()
            } else {
                None
            }
        })
        .unwrap_or(0);

    let body_start = header_end + 4;
    while raw.len().saturating_sub(body_start) < content_length {
        let remaining = content_length - raw.len().saturating_sub(body_start);
        let buf = vec![0u8; remaining.min(READ_BUF)];
        let (result, buf) = stream.read(buf).await;
        let n = result?;
        if n == 0 {
            break;
        }
        raw.extend_from_slice(&buf[..n]);
    }

    Ok(())
}

fn parse_request(raw: &[u8]) -> Result<Request, String> {
    let mut header_buf = [httparse::EMPTY_HEADER; MAX_HEADERS];
    let mut req = httparse::Request::new(&mut header_buf);

    req.parse(raw).map_err(|e| e.to_string())?;

    let method = req.method.unwrap_or("GET").to_string();
    let raw_path = req.path.unwrap_or("/");

    let (path, query) = match raw_path.find('?') {
        Some(i) => (raw_path[..i].to_string(), Some(raw_path[i + 1..].to_string())),
        None => (raw_path.to_string(), None),
    };

    let find = |name: &str| -> Option<String> {
        req.headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| String::from_utf8_lossy(h.value).into_owned())
    };

    Ok(Request {
        method,
        path,
        query,
        host:  find("host"),
        cookie: find("cookie"),
        content_type: find("content-type"),
        sec_websocket_key: find("sec-websocket-key"),
        body: find_header_end(raw)
            .map(|i| raw[i + 4..].to_vec())
            .unwrap_or_default(),
        range: find("range"),
    })
}

fn find_header_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4).position(|w| w == b"\r\n\r\n")
}

fn is_websocket_request(req: &Request) -> bool {
    req.method == "GET"
        && req.sec_websocket_key.is_some()
        && (req.path == "/api/chat/ws" || workspace_ws_id(&req.path).is_some())
}

async fn handle_websocket(mut stream: TcpStream, req: Request) -> io::Result<()> {
    let Some(key) = req.sec_websocket_key.as_deref() else {
        return flush(
            stream,
            Response::json(json!({ "ok": false, "error": "missing websocket key" }).to_string())
                .with_status(400),
        )
        .await;
    };

    let workspace_watch = if let Some(workspace_id) = workspace_ws_id(&req.path) {
        let user = match auth::current_user(&req) {
            Ok(Some(user)) => user,
            Ok(None) => {
                return flush(
                    stream,
                    Response::json(json!({ "ok": false, "error": "unauthenticated" }).to_string())
                        .with_status(401),
                )
                .await;
            }
            Err(error) => {
                return flush(
                    stream,
                    Response::json(json!({ "ok": false, "error": error.to_string() }).to_string())
                        .with_status(500),
                )
                .await;
            }
        };

        match crate::db::auth_store().get_workspace(user.id, &workspace_id) {
            Ok(Some(_)) => Some((user.id, workspace_id)),
            Ok(None) => {
                return flush(
                    stream,
                    Response::json(json!({ "ok": false, "error": "workspace not found" }).to_string())
                        .with_status(404),
                )
                .await;
            }
            Err(error) => {
                return flush(
                    stream,
                    Response::json(json!({ "ok": false, "error": error.to_string() }).to_string())
                        .with_status(500),
                )
                .await;
            }
        }
    } else {
        None
    };

    let accept = websocket_accept_key(key);
    let head = format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {accept}\r\n\
         X-Powered-By: mixer-chat/beta\r\n\
         \r\n"
    );
    let (result, _head) = stream.write_all(head.into_bytes()).await;
    result?;

    let cookie = req.cookie.clone();
    let raw_fd = stream.into_raw_fd();
    std::thread::spawn(move || {
        let result = if let Some((user_id, workspace_id)) = workspace_watch {
            unsafe { run_workspace_websocket(raw_fd, user_id, workspace_id) }
        } else {
            unsafe { run_chat_websocket(raw_fd, cookie) }
        };
        if let Err(error) = result {
            if !is_disconnect_error(&error) {
                eprintln!("[ws] failed: {error}");
            }
        }
    });

    Ok(())
}

fn workspace_ws_id(path: &str) -> Option<String> {
    path.strip_prefix("/api/workspaces/")
        .and_then(|rest| rest.strip_suffix("/ws"))
        .filter(|id| !id.is_empty() && !id.contains('/'))
        .map(str::to_string)
}

async fn flush(mut stream: TcpStream, res: Response) -> io::Result<()> {
    let reason = reason_phrase(res.status);
    match res.body {
        ResponseBody::Fixed(body) => {
            let body_len = body.len();
            let mut head = format!(
                "HTTP/1.1 {status} {reason}\r\n\
                 Content-Type: {ct}\r\n\
                 Content-Length: {body_len}\r\n\
                 Connection: close\r\n\
                 X-Powered-By: mixer-chat/beta\r\n",
                status = res.status,
                ct = res.content_type,
            );

            for (k, v) in &res.extra_headers {
                head.push_str(&format!("{k}: {v}\r\n"));
            }
            head.push_str("\r\n");

            let mut wire = head.into_bytes();
            wire.extend_from_slice(&body);

            let (result, _wire) = stream.write_all(wire).await;
            result.map(|_| ())
        }
        ResponseBody::Stream(chunks) => {
            // ── CRITICAL: disable Nagle for SSE ──────────────────────────
            // Without TCP_NODELAY the kernel can coalesce our small SSE
            // frames waiting for ACKs, which is exactly the "stream dumps
            // all-at-once" symptom. SSE frames are tiny (~100 bytes each)
            // so every one of them gets batched up until the connection
            // closes — completely defeating the streaming protocol.
            let _ = stream.set_nodelay(true);

            // Also tell every proxy in the chain (nginx, cloudflare, etc.)
            // not to buffer the stream — this is the SSE-side complement
            // to TCP_NODELAY and matters once the binary is behind a CDN.
            let mut head = format!(
                "HTTP/1.1 {status} {reason}\r\n\
                 Content-Type: {ct}\r\n\
                 Transfer-Encoding: chunked\r\n\
                 Cache-Control: no-cache, no-transform\r\n\
                 X-Accel-Buffering: no\r\n\
                 Connection: close\r\n\
                 X-Powered-By: mixer-chat/beta\r\n",
                status = res.status,
                ct = res.content_type,
            );

            for (k, v) in &res.extra_headers {
                head.push_str(&format!("{k}: {v}\r\n"));
            }
            head.push_str("\r\n");

            let raw_fd = stream.into_raw_fd();
            std::thread::spawn(move || {
                // Streaming responses are intentionally written by a normal
                // blocking socket. The rest of the server stays on monoio,
                // but SSE needs immediate small writes that browser fetch can
                // observe without waiting for the whole assistant response.
                let result = unsafe { write_sse_response(raw_fd, head, chunks) };
                if let Err(error) = result {
                    eprintln!("[sse/std-writer] failed: {error}");
                }
            });

            Ok(())
        }
    }
}

#[cfg(unix)]
unsafe fn write_sse_response(
    raw_fd: std::os::fd::RawFd,
    head: String,
    chunks: Receiver<String>,
) -> io::Result<()> {
    let mut stream = std::net::TcpStream::from_raw_fd(raw_fd);
    stream.set_nonblocking(false)?;
    stream.set_nodelay(true)?;
    stream.write_all(head.as_bytes())?;
    stream.flush()?;

    loop {
        match chunks.recv() {
            Ok(chunk) => write_sse_chunk(&mut stream, chunk)?,
            Err(_) => {
                stream.write_all(b"0\r\n\r\n")?;
                stream.flush()?;
                let _ = stream.shutdown(std::net::Shutdown::Write);
                return Ok(());
            }
        }
    }
}

#[cfg(unix)]
fn write_sse_chunk(stream: &mut std::net::TcpStream, chunk: String) -> io::Result<()> {
    let bytes = chunk.into_bytes();
    println!(
        "[sse->frontend {}]\n{}\n",
        Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        crate::tools::redact_sensitive(&String::from_utf8_lossy(&bytes))
    );

    let frame_head = format!("{:x}\r\n", bytes.len());
    stream.write_all(frame_head.as_bytes())?;
    stream.write_all(&bytes)?;
    stream.write_all(b"\r\n")?;
    stream.flush()
}

#[cfg(unix)]
unsafe fn run_chat_websocket(raw_fd: std::os::fd::RawFd, cookie: Option<String>) -> io::Result<()> {
    let mut stream = std::net::TcpStream::from_raw_fd(raw_fd);
    stream.set_nonblocking(false)?;
    stream.set_nodelay(true)?;

    let Some(text) = read_ws_text_frame(&mut stream)? else {
        return Ok(());
    };

    let input = match serde_json::from_str::<auth::WsChatInput>(&text) {
        Ok(input) => input,
        Err(_) => {
            write_ws_event(&mut stream, "error", json!({ "error": "invalid websocket payload" }))?;
            write_ws_event(&mut stream, "done", json!({ "assistant_message": null }))?;
            let _ = stream.shutdown(std::net::Shutdown::Write);
            return Ok(());
        }
    };

    let rx = match auth::websocket_chat_stream(cookie, input) {
        Ok(rx) => rx,
        Err((_, error)) => {
            write_ws_event(&mut stream, "error", json!({ "error": error }))?;
            write_ws_event(&mut stream, "done", json!({ "assistant_message": null }))?;
            let _ = stream.shutdown(std::net::Shutdown::Write);
            return Ok(());
        }
    };

    for frame in rx {
        let message = websocket_message_from_sse(&frame);
        println!(
            "[ws->frontend {}]\n{}\n",
            Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            crate::tools::redact_sensitive(&message)
        );
        if let Err(error) = write_ws_text_frame(&mut stream, message.as_bytes()) {
            if is_disconnect_error(&error) {
                return Ok(());
            }
            return Err(error);
        }
    }

    let _ = stream.shutdown(std::net::Shutdown::Write);
    Ok(())
}

#[derive(Debug, Deserialize)]
struct WsWorkspaceInput {
    path: Option<String>,
}

#[cfg(unix)]
unsafe fn run_workspace_websocket(
    raw_fd: std::os::fd::RawFd,
    user_id: i64,
    workspace_id: String,
) -> io::Result<()> {
    let mut stream = std::net::TcpStream::from_raw_fd(raw_fd);
    stream.set_nonblocking(false)?;
    stream.set_nodelay(true)?;

    let Some(text) = read_ws_text_frame(&mut stream)? else {
        return Ok(());
    };

    let input = match serde_json::from_str::<WsWorkspaceInput>(&text) {
        Ok(input) => input,
        Err(_) => {
            write_ws_event(&mut stream, "error", json!({ "error": "invalid workspace websocket payload" }))?;
            let _ = stream.shutdown(std::net::Shutdown::Write);
            return Ok(());
        }
    };

    let workspace_dir = crate::workspace::workspace_dir(user_id, &workspace_id);
    let path = input.path.unwrap_or_default();
    let mut last_message = String::new();

    loop {
        let file_tree = serde_json::from_str::<serde_json::Value>(
            &crate::workspace::file_tree_json(&workspace_dir),
        )
            .unwrap_or_else(|_| json!([]));

        let content = if path.trim().is_empty() {
            None
        } else {
            crate::workspace::read_file_raw(&workspace_dir, &path).ok()
        };

        let message = json!({
            "event": "workspace_snapshot",
            "data": {
                "workspace_id": workspace_id,
                "file_tree": file_tree,
                "path": path,
                "content": content,
            }
        })
        .to_string();

        if message != last_message {
            println!(
                "[ws/workspace->frontend {}]\n{}\n",
                Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                crate::tools::redact_sensitive(&message)
            );
            if let Err(error) = write_ws_text_frame(&mut stream, message.as_bytes()) {
                if is_disconnect_error(&error) {
                    return Ok(());
                }
                return Err(error);
            }
            last_message = message;
        }

        std::thread::sleep(Duration::from_millis(900));
    }
}

fn websocket_message_from_sse(frame: &str) -> String {
    let mut event = "message";
    let mut data_lines = Vec::new();

    for line in frame.lines() {
        if let Some(value) = line.strip_prefix("event:") {
            event = value.trim();
        } else if let Some(value) = line.strip_prefix("data:") {
            data_lines.push(value.trim_start());
        }
    }

    let raw_data = data_lines.join("\n");
    let data = serde_json::from_str::<serde_json::Value>(&raw_data)
        .unwrap_or_else(|_| serde_json::Value::String(raw_data));
    json!({ "event": event, "data": data }).to_string()
}

fn write_ws_event(
    stream: &mut std::net::TcpStream,
    event: &str,
    data: serde_json::Value,
) -> io::Result<()> {
    write_ws_text_frame(stream, json!({ "event": event, "data": data }).to_string().as_bytes())
}

fn read_ws_text_frame(stream: &mut std::net::TcpStream) -> io::Result<Option<String>> {
    let mut head = [0u8; 2];
    stream.read_exact(&mut head)?;

    let opcode = head[0] & 0x0f;
    if opcode == 0x8 {
        return Ok(None);
    }
    if opcode != 0x1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "expected websocket text frame",
        ));
    }

    let masked = (head[1] & 0x80) != 0;
    if !masked {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "client websocket frame was not masked",
        ));
    }

    let mut len = (head[1] & 0x7f) as u64;
    if len == 126 {
        let mut buf = [0u8; 2];
        stream.read_exact(&mut buf)?;
        len = u16::from_be_bytes(buf) as u64;
    } else if len == 127 {
        let mut buf = [0u8; 8];
        stream.read_exact(&mut buf)?;
        len = u64::from_be_bytes(buf);
    }

    if len > 1024 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "websocket frame too large",
        ));
    }

    let mut mask = [0u8; 4];
    stream.read_exact(&mut mask)?;
    let mut payload = vec![0u8; len as usize];
    stream.read_exact(&mut payload)?;
    for (i, byte) in payload.iter_mut().enumerate() {
        *byte ^= mask[i % 4];
    }

    String::from_utf8(payload)
        .map(Some)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid websocket utf-8"))
}

fn write_ws_text_frame(stream: &mut std::net::TcpStream, payload: &[u8]) -> io::Result<()> {
    let mut frame = Vec::with_capacity(payload.len() + 10);
    frame.push(0x81);
    if payload.len() < 126 {
        frame.push(payload.len() as u8);
    } else if payload.len() <= u16::MAX as usize {
        frame.push(126);
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    } else {
        frame.push(127);
        frame.extend_from_slice(&(payload.len() as u64).to_be_bytes());
    }
    frame.extend_from_slice(payload);
    stream.write_all(&frame)?;
    stream.flush()
}

fn websocket_accept_key(key: &str) -> String {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    let mut value = Vec::with_capacity(key.len() + GUID.len());
    value.extend_from_slice(key.trim().as_bytes());
    value.extend_from_slice(GUID.as_bytes());
    STANDARD.encode(sha1_digest(&value))
}

fn sha1_digest(input: &[u8]) -> [u8; 20] {
    let mut h0: u32 = 0x67452301;
    let mut h1: u32 = 0xefcdab89;
    let mut h2: u32 = 0x98badcfe;
    let mut h3: u32 = 0x10325476;
    let mut h4: u32 = 0xc3d2e1f0;

    let bit_len = (input.len() as u64) * 8;
    let mut msg = input.to_vec();
    msg.push(0x80);
    while (msg.len() % 64) != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks(64) {
        let mut w = [0u32; 80];
        for (i, word) in w.iter_mut().take(16).enumerate() {
            let j = i * 4;
            *word = u32::from_be_bytes([chunk[j], chunk[j + 1], chunk[j + 2], chunk[j + 3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }

        let mut a = h0;
        let mut b = h1;
        let mut c = h2;
        let mut d = h3;
        let mut e = h4;

        for (i, word) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | ((!b) & d), 0x5a827999),
                20..=39 => (b ^ c ^ d, 0x6ed9eba1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8f1bbcdc),
                _ => (b ^ c ^ d, 0xca62c1d6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    let mut out = [0u8; 20];
    out[0..4].copy_from_slice(&h0.to_be_bytes());
    out[4..8].copy_from_slice(&h1.to_be_bytes());
    out[8..12].copy_from_slice(&h2.to_be_bytes());
    out[12..16].copy_from_slice(&h3.to_be_bytes());
    out[16..20].copy_from_slice(&h4.to_be_bytes());
    out
}

fn reason_phrase(code: u16) -> &'static str {
    match code {
        200 => "OK",
        206 => "Partial Content",
        204 => "No Content",
        301 => "Moved Permanently",
        302 => "Found",
        304 => "Not Modified",
        400 => "Bad Request",
        404 => "Not Found",
        416 => "Range Not Satisfiable",
        500 => "Internal Server Error",
        _ => "Unknown",
    }
}

fn is_disconnect_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::UnexpectedEof
    )
}
