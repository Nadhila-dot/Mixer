use crate::{
    auth,
    handlers::pages::shell,
    router::{Request, Response},
    ssr::{inject, SsrPayload},
};
use serde_json::json;

pub async fn screen(req: &Request) -> Response {
    let next = query_param(req, "next").unwrap_or_else(|| "/".into());

    if auth::current_user(req).ok().flatten().is_some() {
        return Response::redirect(safe_next(&next));
    }

    let data = json!({
        "next": safe_next(&next),
        "loginEndpoint": "/api/auth/login",
        "registerEndpoint": "/api/auth/register",
        "sessionEndpoint": "/api/auth/me",
    });
    Response::html(inject(&shell(), SsrPayload { page: "auth-screen", data }))
}

fn query_param(req: &Request, key: &str) -> Option<String> {
    req.query.as_deref()?.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        (percent_decode(k) == key).then(|| percent_decode(v))
    })
}

fn safe_next(next: &str) -> String {
    if next.starts_with('/') && !next.starts_with("//") {
        next.to_string()
    } else {
        "/".into()
    }
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
