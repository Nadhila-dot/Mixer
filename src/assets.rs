/// Embedded frontend assets (production) and dev-mode disk helpers.
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "frontend/dist/"]
struct Dist;

/// Fetch an embedded file by its URL path (leading `/` stripped).
pub fn get(url_path: &str) -> Option<rust_embed::EmbeddedFile> {
    Dist::get(url_path.trim_start_matches('/'))
}

/// The production index.html shell — page handlers inject SSR data into this.
pub fn index_html() -> String {
    let f = Dist::get("index.html")
        .expect("index.html missing from frontend/dist — run `make build-fe`");
    String::from_utf8_lossy(&f.data).into_owned()
}

/// Number of embedded files (shown in the startup banner).
pub fn count() -> usize {
    Dist::iter().count()
}

/// Read a file from `frontend/public/` off disk — used in dev mode to serve
/// static public assets (favicon, etc.) without needing the embedded bundle.
pub fn public_file(url_path: &str) -> Option<Vec<u8>> {
    let rel = url_path.trim_start_matches('/');
    std::fs::read(format!("frontend/public/{rel}")).ok()
}
