/// Injects server-side data into the HTML shell before it reaches the browser.
///
/// Pattern: the Rust handler fetches/computes data, serialises it to JSON, and
/// splices a <script> tag into the HTML just before </head>.  React reads
/// `window.__SSR_DATA__` synchronously during its first render — no waterfall,
/// no loading spinner on the initial page load.
use serde_json::Value;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct SsrPayload<'a> {
    /// Matches React Router's current route name (e.g. "home", "auth-screen")
    pub page: &'a str,
    /// Arbitrary page-specific data that the frontend will read
    pub data: Value,
}

/// Splice the SSR payload into the HTML shell.
/// Returns the modified HTML; the original string is unchanged.
pub fn inject(shell: &str, payload: SsrPayload<'_>) -> String {
    let envelope = serde_json::json!({
        "page": payload.page,
        "ts":   unix_ms(),
        "data": payload.data,
    });

    let script = format!(
        r#"<script>window.__SSR_DATA__={json};</script>"#,
        json = serde_json::to_string(&envelope).unwrap_or_else(|_| "null".into())
    );

    // Splice just before </head> so the data is available before any module scripts run
    shell.replacen("</head>", &format!("{script}\n  </head>"), 1)
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
