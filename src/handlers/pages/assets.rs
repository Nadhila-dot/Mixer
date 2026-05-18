use crate::{
    assets, dev,
    router::{Response, ResponseBody},
};

pub async fn asset(url_path: &str) -> Response {
    if dev::is_dev() {
        if let Some(data) = assets::public_file(url_path) {
            let mime = mime_guess::from_path(url_path)
                .first_or_octet_stream()
                .to_string();
            return Response::bytes(data, mime);
        }

        let vite_url = format!("http://localhost:{}{}", dev::vite_port(), url_path);
        return Response {
            status: 302,
            content_type: "text/plain".into(),
            body: ResponseBody::Fixed(vite_url.as_bytes().to_vec()),
            extra_headers: vec![("Location".into(), vite_url.clone())],
        };
    }

    let Some(file) = assets::get(url_path) else {
        return Response::not_found();
    };

    let mime = mime_guess::from_path(url_path)
        .first_or_octet_stream()
        .to_string();

    let mut res = Response::bytes(file.data.to_vec(), mime);

    if url_path.starts_with("/assets/") {
        res = res.with_header("Cache-Control", "public, max-age=31536000, immutable");
    }

    res
}

pub async fn disk_file(url_path: &str, range: Option<&str>) -> Response {
    let rel = url_path.trim_start_matches('/');
    let candidates = [
        format!("frontend/public/{rel}"),
        format!("public/{rel}"),
        format!("frontend/dist/{rel}"),
    ];

    // Try disk first so dev-mode edits to frontend/public/* are picked up live.
    // In production the binary runs from /etc/mixer and none of those paths exist,
    // so we fall back to the rust_embed bundle (Vite copies frontend/public/* into
    // frontend/dist/* at build, which is what rust_embed packages).
    let data = candidates
        .iter()
        .find_map(|p| std::fs::read(p).ok())
        .or_else(|| assets::get(url_path).map(|f| f.data.to_vec()))
        .unwrap_or_default();

    if data.is_empty() {
        return Response::not_found();
    }

    let mime = mime_guess::from_path(url_path)
        .first_or_octet_stream()
        .to_string();

    let total = data.len();

    if let Some(range_str) = range.and_then(|r| r.strip_prefix("bytes=")) {
        let mut parts = range_str.splitn(2, '-');
        let start: usize = parts.next().and_then(|s| s.parse().ok()).unwrap_or(0);
        let end: usize = parts
            .next()
            .and_then(|s| if s.is_empty() { None } else { s.parse().ok() })
            .unwrap_or(total.saturating_sub(1))
            .min(total.saturating_sub(1));

        if start > end || start >= total {
            return Response {
                status: 416,
                content_type: "text/plain".into(),
                body: ResponseBody::Fixed(format!("bytes */{total}").into_bytes()),
                extra_headers: vec![("Content-Range".into(), format!("bytes */{total}"))],
            };
        }

        let chunk = data[start..=end].to_vec();
        return Response {
            status: 206,
            content_type: mime,
            body: ResponseBody::Fixed(chunk),
            extra_headers: vec![
                (
                    "Content-Range".into(),
                    format!("bytes {start}-{end}/{total}"),
                ),
                ("Accept-Ranges".into(), "bytes".into()),
            ],
        };
    }

    Response::bytes(data, mime).with_header("Accept-Ranges", "bytes")
}
