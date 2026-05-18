mod ai;
mod assets;
mod auth;
mod db;
mod handlers;
mod prompt;
mod router;
mod run;
mod server;
mod skills;
mod ssr;
mod tools;
mod workspace;

pub mod dev;

use monoio::net::TcpListener;
use std::io;

#[monoio::main(timer_enabled = true)]
async fn main() -> io::Result<()> {
    load_dotenv();
    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".to_string());
    let addr = format!("0.0.0.0:{port}");

    if dev::is_dev() {
        let vite_port = dev::vite_port();

        println!();
        println!("  Mixer chat [dev]");
        println!("  ────────────────────────────────────────");
        println!("  ► app       : http://localhost:{port}");
        println!("  ► vite HMR  : http://localhost:{vite_port}");
        println!("  ► runtime   : monoio");
        println!();

        // Spawn Vite dev server — same trick as `php artisan serve` + `npm run dev`.
        // Ctrl-C sends SIGINT to the whole process group, so Vite dies with us.
        dev::spawn_vite(vite_port);
    } else {
        let asset_count = assets::count();
        println!();
        println!("  [Mixer chat]");
        println!("  ────────────────────────────────────────");
        println!("  ► http://localhost:{port}");
        println!("  ► runtime  : monoio (completion I/O)");
        println!("  ► assets   : {asset_count} files embedded");
        println!("  ► binary   : {}", current_exe_size());
        println!();
    }

    let listener = TcpListener::bind(&addr)?;

    loop {
        let (stream, peer) = listener.accept().await?;
        monoio::spawn(async move {
            if let Err(e) = server::handle(stream).await {
                if !is_benign_io_error(&e) {
                    eprintln!("  [{peer}] {e}");
                }
            }
        });
    }
}

fn load_dotenv() {
    let Ok(contents) = std::fs::read_to_string(".env") else {
        return;
    };

    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if std::env::var_os(key).is_none() {
            std::env::set_var(key.trim(), value.trim().trim_matches('"'));
        }
    }
}

fn is_benign_io_error(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::UnexpectedEof | io::ErrorKind::ConnectionReset | io::ErrorKind::BrokenPipe
    )
}

fn current_exe_size() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| format!("{:.1} MB", m.len() as f64 / 1_048_576.0))
        .unwrap_or_else(|| "?".to_string())
}
