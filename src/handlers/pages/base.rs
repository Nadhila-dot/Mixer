use crate::{
    auth,
    handlers::pages::shell,
    router::{Request, Response},
    ssr::{inject, SsrPayload},
};
use serde_json::json;

pub async fn home(req: &Request) -> Response {
    let data = match auth::app_state_json(req) {
        Ok(data) => data,
        Err(e) => {
            return Response::json(json!({ "ok": false, "error": e.to_string() }).to_string())
                .with_status(500);
        }
    };

    Response::html(inject(&shell(), SsrPayload { page: "home", data }))
}

pub async fn spa_shell(path: &str) -> Response {
    let data = json!({ "requestedPath": path });
    Response::html(inject(
        &shell(),
        SsrPayload {
            page: "not-found",
            data,
        },
    ))
}
