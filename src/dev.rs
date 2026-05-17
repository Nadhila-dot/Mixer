/// Dev-mode helpers — Vite integration.
///
/// When running as a debug binary (`cargo run`), the server:
///   1. Detects it's in dev mode via `debug_assertions`
///   2. Spawns `bun run dev` in `frontend/` (the Vite HMR server)
///   3. Returns a lightweight HTML shell that loads JS/CSS from Vite's port
///      instead of the embedded, production-built assets
///
/// The SSR data injection is identical in both modes — `window.__SSR_DATA__`
/// is spliced in before `</head>` regardless of which shell is used.
///
/// Ctrl-C sends SIGINT to the terminal process group, which includes the
/// spawned `bun` child, so everything shuts down cleanly together.
use std::process::Command;

/// True in debug builds (`cargo run`); false in release builds (`cargo build --release`).
/// Override with `FORGE_PROD=1` to force prod mode even while debugging.
pub fn is_dev() -> bool {
    #[cfg(debug_assertions)]
    {
        std::env::var("FORGE_PROD").is_err()
    }
    #[cfg(not(debug_assertions))]
    {
        std::env::var("FORGE_DEV").is_ok()
    }
}

/// Vite's dev server port. Override with `VITE_PORT=<n>`.
pub fn vite_port() -> u16 {
    std::env::var("VITE_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(5173)
}

/// Spawn `bun run dev` in `frontend/` and wait briefly for Vite to bind its port.
/// The child is intentionally left running — the OS delivers SIGINT to the whole
/// process group on Ctrl-C, which terminates both processes.
pub fn spawn_vite(port: u16) {
    let child = Command::new("bun")
        .args(["run", "dev"])
        .current_dir("frontend")
        .spawn();

    match child {
        Ok(_) => {
            // Give Vite enough time to start before the first browser request arrives.
            // 600 ms is conservative; Vite typically binds in ~200 ms.
            std::thread::sleep(std::time::Duration::from_millis(600));
            println!("  ✓ Vite dev server ready on :{port}");
            println!();
        }
        Err(e) => {
            eprintln!("  ✗ Could not start Vite: {e}");
            eprintln!("    Run `cd frontend && bun run dev` manually.");
            println!();
        }
    }
}

/// The HTML shell served in dev mode.
///
/// When the page HTML is served from a different origin than the Vite dev
/// server (our case: `:3000` vs `:5173`), `@vitejs/plugin-react`'s Fast
/// Refresh runtime is never bootstrapped automatically — Vite's own
/// `transformIndexHtml` hook only runs when Vite owns the HTML.
///
/// We replicate that hook manually: the `@react-refresh` preamble script
/// must execute and set `window.__vite_plugin_react_preamble_installed__`
/// *before* any component module is evaluated. That's what the inline
/// `type="module"` block below does.
pub fn shell(vite_port: u16) -> String {
    format!(
        r#"<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <link rel="icon" type="image/svg+xml" href="/favicon.svg" />
    <title>Forge ⚡ dev</title>
    <link rel="preconnect" href="https://fonts.googleapis.com" />
    <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin />
    <link href="https://fonts.googleapis.com/css2?family=DM+Mono:ital,wght@0,300;0,400;0,500;1,300&family=DM+Sans:ital,opsz,wght@0,9..40,300;0,9..40,400;0,9..40,500;1,9..40,300&family=Quicksand:wght@300;400;500;600;700&display=swap" rel="stylesheet" />
    <!-- Chillax is served locally from /fonts/Chillax-Variable.ttf via @font-face in index.css -->
    <!-- window.__SSR_DATA__ injected here -->
  </head>
  <body>
    <div id="root"></div>

    <!--
      React Fast Refresh preamble — mirrors what @vitejs/plugin-react injects
      via transformIndexHtml when it owns the HTML file. Must run before any
      component module so window.__vite_plugin_react_preamble_installed__ is
      set when the first JSX file is evaluated.
    -->
    <script type="module">
      import RefreshRuntime from 'http://localhost:{vite_port}/@react-refresh'
      RefreshRuntime.injectIntoGlobalHook(window)
      window.$RefreshReg$ = () => {{}}
      window.$RefreshSig$ = () => (type) => type
      window.__vite_plugin_react_preamble_installed__ = true
    </script>

    <!-- Vite HMR websocket client -->
    <script type="module" src="http://localhost:{vite_port}/@vite/client"></script>
    <!-- Live entry point — Vite transpiles on-demand -->
    <script type="module" src="http://localhost:{vite_port}/src/main.tsx"></script>
  </body>
</html>"#
    )
}
