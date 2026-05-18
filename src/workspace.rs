use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Component, Path, PathBuf},
    process::{Child, Command as StdCommand},
    sync::{mpsc, Mutex, OnceLock},
    thread,
    time::{Duration, Instant},
};

const MAX_OUTPUT_BYTES: usize = 64_000;
const MAX_FILE_READ_BYTES: usize = 128_000;
const DEFAULT_TIMEOUT_SECS: u64 = 10;
const MAX_TIMEOUT_SECS: u64 = 120;
const DEFAULT_READ_LINES: usize = 500;
const MAX_READ_LINES: usize = 1000;

// ── Directory resolution ────────────────────────────────────────────────────

pub fn workspaces_root() -> PathBuf {
    let dir = std::env::var("WORKSPACES_DIR").unwrap_or_else(|_| "workspaces".to_string());
    PathBuf::from(dir)
}

pub fn workspace_dir(user_id: i64, workspace_uuid: &str) -> PathBuf {
    workspaces_root()
        .join(user_id.to_string())
        .join(workspace_uuid)
}

/// Resolves a relative path inside a workspace, rejecting any path that
/// escapes the workspace boundary (absolute paths, `..` components, etc.).
pub fn safe_path(workspace_dir: &Path, relative: &str) -> Result<PathBuf, String> {
    let workspace_root = canonical_workspace_root(workspace_dir)?;
    let path = Path::new(relative.trim_start_matches('/'));
    if path.is_absolute() {
        return Err("Absolute paths are not permitted inside a workspace".to_string());
    }
    let mut result = workspace_root.clone();
    let components = path.components().collect::<Vec<_>>();
    for (index, component) in components.iter().enumerate() {
        match component {
            Component::Normal(name) => {
                result.push(name);
                if let Ok(meta) = std::fs::symlink_metadata(&result) {
                    if meta.file_type().is_symlink() {
                        return Err("Symlinks are not permitted inside workspace paths".to_string());
                    }
                    if index + 1 < components.len() && !meta.is_dir() {
                        return Err("A path component is not a directory".to_string());
                    }
                    if let Ok(canonical) = std::fs::canonicalize(&result) {
                        if !canonical.starts_with(&workspace_root) {
                            return Err("Resolved path escaped the workspace boundary".to_string());
                        }
                    }
                }
            }
            Component::CurDir => {}
            Component::ParentDir => return Err("Path traversal (..) is not allowed".to_string()),
            _ => return Err("Invalid path component".to_string()),
        }
    }
    if !result.starts_with(&workspace_root) {
        return Err("Resolved path escaped the workspace boundary".to_string());
    }
    Ok(result)
}

fn canonical_workspace_root(workspace_dir: &Path) -> Result<PathBuf, String> {
    let original_meta = std::fs::symlink_metadata(workspace_dir)
        .map_err(|e| format!("Could not inspect workspace root: {e}"))?;
    if original_meta.file_type().is_symlink() {
        return Err("Workspace root may not be a symlink".to_string());
    }
    if !original_meta.is_dir() {
        return Err("Workspace root is not a directory".to_string());
    }
    let root = std::fs::canonicalize(workspace_dir)
        .map_err(|e| format!("Workspace root is not available: {e}"))?;
    Ok(root)
}

// ── Workspace directory setup ───────────────────────────────────────────────

pub fn init_workspace_dir(user_id: i64, workspace_uuid: &str) -> Result<PathBuf, String> {
    let dir = workspace_dir(user_id, workspace_uuid);
    std::fs::create_dir_all(&dir).map_err(|e| format!("Could not create workspace: {e}"))?;
    Ok(dir)
}

// ── File operations ─────────────────────────────────────────────────────────

pub fn create_file(workspace_dir: &Path, relative: &str, content: &str) -> Result<(), String> {
    let target = safe_path(workspace_dir, relative)?;
    ensure_safe_existing_file(&target)?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Could not create parent dir: {e}"))?;
    }
    std::fs::write(&target, content).map_err(|e| format!("Could not write file: {e}"))?;
    Ok(())
}

pub fn read_file(workspace_dir: &Path, relative: &str) -> Result<String, String> {
    read_file_window(workspace_dir, relative, 1, DEFAULT_READ_LINES)
}

pub fn read_file_raw(workspace_dir: &Path, relative: &str) -> Result<String, String> {
    let target = safe_path(workspace_dir, relative)?;
    ensure_safe_existing_file(&target)?;
    let bytes = std::fs::read(&target).map_err(|e| format!("Could not read file: {e}"))?;
    if bytes.len() > MAX_FILE_READ_BYTES * 8 {
        return Err(format!(
            "File is too large to edit ({} bytes, max {})",
            bytes.len(),
            MAX_FILE_READ_BYTES * 8
        ));
    }
    String::from_utf8(bytes).map_err(|_| "File contains non-UTF-8 bytes".to_string())
}

pub fn read_file_window(
    workspace_dir: &Path,
    relative: &str,
    start_line: usize,
    max_lines: usize,
) -> Result<String, String> {
    let target = safe_path(workspace_dir, relative)?;
    ensure_safe_existing_file(&target)?;
    let file = std::fs::File::open(&target).map_err(|e| format!("Could not read file: {e}"))?;
    let start_line = start_line.max(1);
    let max_lines = max_lines.clamp(1, MAX_READ_LINES);
    let end_line = start_line.saturating_add(max_lines).saturating_sub(1);
    let mut out = String::new();
    let mut total_lines = 0usize;
    let mut shown_lines = 0usize;
    let mut truncated_by_bytes = false;

    for (idx, line) in BufReader::new(file).lines().enumerate() {
        let line_no = idx + 1;
        let line = line.map_err(|_| "File contains non-UTF-8 bytes".to_string())?;
        total_lines = line_no;
        if line_no < start_line {
            continue;
        }
        if line_no > end_line {
            break;
        }
        if out.len().saturating_add(line.len()).saturating_add(16) > MAX_FILE_READ_BYTES {
            truncated_by_bytes = true;
            break;
        }
        out.push_str(&format!("{line_no}: {line}\n"));
        shown_lines += 1;
    }

    if shown_lines == 0 {
        return Ok(format!(
            "(no lines in requested window; requested lines {start_line}-{end_line})"
        ));
    }

    if truncated_by_bytes {
        out.push_str("[read truncated by byte limit]\n");
    } else if total_lines >= end_line {
        out.push_str(&format!(
            "[showing lines {start_line}-{end_line}; ask for a later start line to continue]\n"
        ));
    }

    Ok(out)
}

pub fn append_file(workspace_dir: &Path, relative: &str, content: &str) -> Result<(), String> {
    let target = safe_path(workspace_dir, relative)?;
    ensure_safe_existing_file(&target)?;
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Could not create parent dir: {e}"))?;
    }
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&target)
        .map_err(|e| format!("Could not open file for append: {e}"))?;
    file.write_all(content.as_bytes())
        .map_err(|e| format!("Could not append to file: {e}"))
}

fn ensure_safe_existing_file(path: &Path) -> Result<(), String> {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return Ok(());
    };
    let kind = meta.file_type();
    if kind.is_symlink() {
        return Err("Symlink files are not permitted in workspace operations".to_string());
    }
    if !kind.is_file() {
        return Err("Only regular files are permitted for this operation".to_string());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() > 1 {
            return Err("Hard-linked files are not permitted in workspace operations".to_string());
        }
    }
    Ok(())
}

pub fn patch_file(workspace_dir: &Path, relative: &str, patch: &str) -> Result<String, String> {
    let current = read_file_raw(workspace_dir, relative)?;
    let updated = apply_unified_patch(&current, patch)?;
    create_file(workspace_dir, relative, &updated)?;
    Ok(make_simple_diff(&current, &updated))
}

pub fn delete_file(workspace_dir: &Path, relative: &str) -> Result<(), String> {
    let target = safe_path(workspace_dir, relative)?;
    std::fs::remove_file(&target).map_err(|e| format!("Could not delete file: {e}"))?;
    Ok(())
}

pub fn create_directory(workspace_dir: &Path, relative: &str) -> Result<(), String> {
    let target = safe_path(workspace_dir, relative)?;
    std::fs::create_dir_all(&target).map_err(|e| format!("Could not create directory: {e}"))?;
    Ok(())
}

pub fn delete_directory(workspace_dir: &Path, relative: &str) -> Result<(), String> {
    let target = safe_path(workspace_dir, relative)?;
    let root = canonical_workspace_root(workspace_dir)?;
    if target == root {
        return Err("Cannot delete the workspace root".to_string());
    }
    std::fs::remove_dir_all(&target).map_err(|e| format!("Could not delete directory: {e}"))?;
    Ok(())
}

// ── Command execution ───────────────────────────────────────────────────────

pub struct CommandResult {
    pub exit_code: i32,
    pub combined_output: String,
    pub duration_ms: u64,
    pub timed_out: bool,
}

pub struct LongProcessResult {
    pub exit_code: i32,
    pub combined_output: String,
    pub duration_ms: u64,
    pub port: u16,
    pub pid: Option<u32>,
    pub running: bool,
}

fn long_processes() -> &'static Mutex<HashMap<u32, Child>> {
    static MAP: OnceLock<Mutex<HashMap<u32, Child>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn is_forbidden_command(command: &str) -> Option<&'static str> {
    let cmd = command.trim();
    let lower = cmd.to_lowercase();
    if lower.is_empty() {
        return Some("empty commands are not permitted");
    }
    if lower.starts_with("su ") || lower == "su" {
        return Some("user switching is not permitted in workspaces");
    }
    None
}

fn uses_sshpass(command: &str) -> bool {
    shell_words(command)
        .map(|argv| {
            argv.iter()
                .find(|arg| !arg.contains('=') || arg.starts_with('-'))
                .map(String::as_str)
                == Some("sshpass")
        })
        .unwrap_or(false)
}

fn sshpass_available() -> bool {
    workspace_path_env()
        .split(':')
        .map(|dir| Path::new(dir).join("sshpass"))
        .any(|path| path.is_file())
}

fn approval_required_command(command: &str) -> Option<&'static str> {
    let argv = shell_words(command)?;
    let exe = argv.first()?.as_str();

    if matches!(
        exe,
        "rm" | "rmdir" | "mv" | "chmod" | "chown" | "ln" | "kill" | "pkill" | "killall"
    ) {
        return Some("destructive filesystem/process command requires user approval");
    }
    if exe == "git"
        && argv
            .iter()
            .any(|arg| matches!(arg.as_str(), "clean" | "reset" | "checkout" | "restore"))
    {
        return Some("destructive git operation requires user approval");
    }

    None
}

fn shell_words(command: &str) -> Option<Vec<String>> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut escaped = false;

    for ch in command.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            continue;
        }
        if ch.is_whitespace() {
            if !current.is_empty() {
                words.push(current.to_lowercase());
                current = String::new();
            }
            continue;
        }
        current.push(ch);
    }

    if quote.is_some() || escaped {
        return None;
    }
    if !current.is_empty() {
        words.push(current.to_lowercase());
    }
    Some(words)
}

fn workspace_path_env() -> &'static str {
    "/Users/nadhi/.bun/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/local/sbin:/Users/nadhi/.cargo/bin"
}

fn command_program(_workspace_dir: &Path, _tmp_dir: &Path) -> StdCommand {
    let mut command = StdCommand::new("/bin/sh");
    command.arg("-c");
    command
}

fn prepare_command_env(command: &mut StdCommand, workspace_dir: &Path, tmp_dir: &Path) {
    command
        .current_dir(workspace_dir)
        .env_clear()
        .env("HOME", workspace_dir)
        .env("PWD", workspace_dir.to_string_lossy().as_ref())
        .env("TMPDIR", tmp_dir)
        .env("PATH", workspace_path_env())
        .env("TERM", "xterm-256color")
        .env("LANG", "en_US.UTF-8");
}

fn prepare_tmp_dir(workspace_dir: &Path) -> PathBuf {
    let tmp = workspace_dir.join(".tmp");
    let _ = std::fs::create_dir_all(&tmp);
    tmp
}

fn run_command_blocking(workspace_dir: &Path, command: &str) -> CommandResult {
    let start = Instant::now();
    let tmp_dir = prepare_tmp_dir(workspace_dir);
    let mut cmd = command_program(workspace_dir, &tmp_dir);
    cmd.arg(command);
    prepare_command_env(&mut cmd, workspace_dir, &tmp_dir);
    let result = cmd.output();

    let duration_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(out) => {
            let mut combined = String::new();
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !stdout.is_empty() {
                combined.push_str(&stdout);
            }
            if !stderr.is_empty() {
                if !combined.is_empty() {
                    combined.push('\n');
                }
                combined.push_str(&stderr);
            }
            if combined.len() > MAX_OUTPUT_BYTES {
                combined.truncate(MAX_OUTPUT_BYTES);
                combined.push_str("\n[output truncated]");
            }
            CommandResult {
                exit_code: out.status.code().unwrap_or(-1),
                combined_output: combined,
                duration_ms,
                timed_out: false,
            }
        }
        Err(e) => CommandResult {
            exit_code: -1,
            combined_output: format!("Failed to execute command: {e}"),
            duration_ms,
            timed_out: false,
        },
    }
}

pub fn run_command(workspace_dir: &Path, command: &str, timeout_secs: u64) -> CommandResult {
    run_command_streaming(workspace_dir, command, timeout_secs, |_| {})
}

pub fn run_command_streaming<F>(
    workspace_dir: &Path,
    command: &str,
    timeout_secs: u64,
    mut on_chunk: F,
) -> CommandResult
where
    F: FnMut(&str),
{
    if let Some(reason) = is_forbidden_command(command) {
        return CommandResult {
            exit_code: 126,
            combined_output: format!("Blocked: {reason}"),
            duration_ms: 0,
            timed_out: false,
        };
    }
    if let Some(reason) = approval_required_command(command) {
        return CommandResult {
            exit_code: 125,
            combined_output: format!("Approval required: {reason}. Ask the user to approve this command before running it."),
            duration_ms: 0,
            timed_out: false,
        };
    }
    if uses_sshpass(command) && !sshpass_available() {
        return CommandResult {
            exit_code: 127,
            combined_output: "Command requires sshpass, but sshpass is not installed on this macOS host. Prefer SSH keys, ask for an interactive-safe connection method, or ask for approval to install sshpass. On macOS, sshpass is usually not installed by default and may require a third-party Homebrew tap.".to_string(),
            duration_ms: 0,
            timed_out: false,
        };
    }

    let timeout = if timeout_secs == 0 {
        DEFAULT_TIMEOUT_SECS
    } else {
        timeout_secs.min(MAX_TIMEOUT_SECS)
    };
    let start = Instant::now();
    let tmp_dir = prepare_tmp_dir(workspace_dir);
    let mut cmd = command_program(workspace_dir, &tmp_dir);
    cmd.arg(command)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    prepare_command_env(&mut cmd, workspace_dir, &tmp_dir);

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            return CommandResult {
                exit_code: -1,
                combined_output: format!("Failed to execute command: {e}"),
                duration_ms: 0,
                timed_out: false,
            }
        }
    };

    let (tx, rx) = mpsc::channel::<String>();

    if let Some(mut stdout) = child.stdout.take() {
        let tx = tx.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 1024];
            loop {
                match stdout.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let _ = tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());
                    }
                    Err(_) => break,
                }
            }
        });
    }

    if let Some(mut stderr) = child.stderr.take() {
        let tx = tx.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 1024];
            loop {
                match stderr.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let _ = tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());
                    }
                    Err(_) => break,
                }
            }
        });
    }

    drop(tx);

    let mut combined = String::new();
    let mut exit_code = None;
    let mut timed_out = false;

    loop {
        while let Ok(chunk) = rx.try_recv() {
            on_chunk(&chunk);
            combined.push_str(&chunk);
            if combined.len() > MAX_OUTPUT_BYTES {
                combined.truncate(MAX_OUTPUT_BYTES);
                combined.push_str("\n[output truncated]");
            }
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                exit_code = Some(status.code().unwrap_or(-1));
                break;
            }
            Ok(None) => {
                if start.elapsed() >= Duration::from_secs(timeout) {
                    let _ = child.kill();
                    let _ = child.wait();
                    timed_out = true;
                    exit_code = Some(124);
                    if !combined.is_empty() && !combined.ends_with('\n') {
                        combined.push('\n');
                    }
                    combined.push_str(&format!("Command timed out after {timeout}s"));
                    break;
                }
                thread::sleep(Duration::from_millis(30));
            }
            Err(e) => {
                exit_code = Some(-1);
                if !combined.is_empty() && !combined.ends_with('\n') {
                    combined.push('\n');
                }
                combined.push_str(&format!("Failed while waiting for command: {e}"));
                break;
            }
        }
    }

    while let Ok(chunk) = rx.try_recv() {
        on_chunk(&chunk);
        combined.push_str(&chunk);
        if combined.len() > MAX_OUTPUT_BYTES {
            combined.truncate(MAX_OUTPUT_BYTES);
            combined.push_str("\n[output truncated]");
            break;
        }
    }

    CommandResult {
        exit_code: exit_code.unwrap_or(-1),
        combined_output: combined,
        duration_ms: start.elapsed().as_millis() as u64,
        timed_out,
    }
}

pub fn run_long_process_streaming<F>(
    workspace_dir: &Path,
    command: &str,
    timeout_secs: u64,
    mut on_chunk: F,
) -> LongProcessResult
where
    F: FnMut(&str),
{
    if let Some(reason) = is_forbidden_command(command) {
        return LongProcessResult {
            exit_code: 126,
            combined_output: format!("Command blocked: {reason}"),
            duration_ms: 0,
            port: 0,
            pid: None,
            running: false,
        };
    }
    if let Some(reason) = approval_required_command(command) {
        return LongProcessResult {
            exit_code: 125,
            combined_output: format!("Approval required: {reason}. Ask the user to approve this command before running it."),
            duration_ms: 0,
            port: 0,
            pid: None,
            running: false,
        };
    }
    if uses_sshpass(command) && !sshpass_available() {
        return LongProcessResult {
            exit_code: 127,
            combined_output: "Command requires sshpass, but sshpass is not installed on this macOS host. Prefer SSH keys, ask for an interactive-safe connection method, or ask for approval to install sshpass. On macOS, sshpass is usually not installed by default and may require a third-party Homebrew tap.".to_string(),
            duration_ms: 0,
            port: 0,
            pid: None,
            running: false,
        };
    }

    let port = find_available_port().unwrap_or(4173);
    let timeout = if timeout_secs == 0 {
        20
    } else {
        timeout_secs.min(MAX_TIMEOUT_SECS)
    };
    let start = Instant::now();
    let tmp_dir = prepare_tmp_dir(workspace_dir);
    let mut cmd = command_program(workspace_dir, &tmp_dir);
    cmd.arg(command)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    prepare_command_env(&mut cmd, workspace_dir, &tmp_dir);
    cmd.env("PORT", port.to_string())
        .env("HOST", "0.0.0.0")
        .env("BIND_HOST", "0.0.0.0");

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            return LongProcessResult {
                exit_code: -1,
                combined_output: format!("Failed to execute command: {e}"),
                duration_ms: 0,
                port,
                pid: None,
                running: false,
            }
        }
    };
    let pid = child.id();

    let (tx, rx) = mpsc::channel::<String>();
    if let Some(mut stdout) = child.stdout.take() {
        let tx = tx.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 1024];
            loop {
                match stdout.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let _ = tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());
                    }
                    Err(_) => break,
                }
            }
        });
    }
    if let Some(mut stderr) = child.stderr.take() {
        let tx = tx.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 1024];
            loop {
                match stderr.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let _ = tx.send(String::from_utf8_lossy(&buf[..n]).into_owned());
                    }
                    Err(_) => break,
                }
            }
        });
    }
    drop(tx);

    let mut combined = String::new();
    loop {
        while let Ok(chunk) = rx.try_recv() {
            on_chunk(&chunk);
            combined.push_str(&chunk);
            if combined.len() > MAX_OUTPUT_BYTES {
                combined.truncate(MAX_OUTPUT_BYTES);
                combined.push_str("\n[output truncated]");
            }
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                return LongProcessResult {
                    exit_code: status.code().unwrap_or(-1),
                    combined_output: combined,
                    duration_ms: start.elapsed().as_millis() as u64,
                    port,
                    pid: Some(pid),
                    running: false,
                };
            }
            Ok(None) => {
                if port_accepts_connections(port) || start.elapsed() >= Duration::from_secs(timeout)
                {
                    let running = port_accepts_connections(port);
                    if !combined.is_empty() && !combined.ends_with('\n') {
                        combined.push('\n');
                    }
                    if running {
                        combined.push_str(&format!(
                            "Long-running process is active on port {port} (pid {pid})."
                        ));
                    } else {
                        combined.push_str(&format!("Long-running process started (pid {pid}), but port {port} was not reachable within {timeout}s."));
                    }
                    long_processes().lock().unwrap().insert(pid, child);
                    return LongProcessResult {
                        exit_code: if running { 0 } else { 124 },
                        combined_output: combined,
                        duration_ms: start.elapsed().as_millis() as u64,
                        port,
                        pid: Some(pid),
                        running,
                    };
                }
                thread::sleep(Duration::from_millis(80));
            }
            Err(e) => {
                combined.push_str(&format!("\nFailed while waiting for process: {e}"));
                return LongProcessResult {
                    exit_code: -1,
                    combined_output: combined,
                    duration_ms: start.elapsed().as_millis() as u64,
                    port,
                    pid: Some(pid),
                    running: false,
                };
            }
        }
    }
}

fn find_available_port() -> Option<u16> {
    for port in 4100..4999 {
        if TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return Some(port);
        }
    }
    None
}

fn port_accepts_connections(port: u16) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(120)).is_ok()
}

// ── File tree ───────────────────────────────────────────────────────────────

#[derive(serde::Serialize, Clone)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<FileEntry>>,
}

pub fn file_tree(dir: &Path, workspace_root: &Path, depth: usize) -> Vec<FileEntry> {
    if depth > 8 {
        return vec![];
    }
    let Ok(rd) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
    entries.sort_by(|a, b| {
        let a_is_file = a.path().is_file();
        let b_is_file = b.path().is_file();
        if a_is_file != b_is_file {
            a_is_file.cmp(&b_is_file) // dirs first
        } else {
            a.file_name().cmp(&b.file_name())
        }
    });

    entries
        .iter()
        .filter_map(|entry| {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                return None;
            };
            if meta.file_type().is_symlink() {
                return None;
            }
            // Skip hidden files at root to keep the tree clean
            if depth == 0 && name.starts_with('.') {
                return None;
            }
            let rel = path
                .strip_prefix(workspace_root)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();
            let is_dir = meta.is_dir();
            let size = if is_dir { 0 } else { meta.len() };
            let children = if is_dir {
                Some(file_tree(&path, workspace_root, depth + 1))
            } else {
                None
            };
            Some(FileEntry {
                name,
                path: rel,
                is_dir,
                size,
                children,
            })
        })
        .collect()
}

pub fn file_tree_json(workspace_dir: &Path) -> String {
    let tree = file_tree(workspace_dir, workspace_dir, 0);
    serde_json::to_string(&tree).unwrap_or_else(|_| "[]".to_string())
}

fn apply_unified_patch(original: &str, patch: &str) -> Result<String, String> {
    let original_lines = split_lines_lossless(original);
    let patch = normalize_patch_text(patch);
    let hunks = parse_patch_hunks(&patch)?;
    if hunks.is_empty() {
        return Err("Patch contains no unified diff hunks".into());
    }

    let mut out = Vec::<String>::new();
    let mut src_idx = 0usize;

    for hunk in &hunks {
        let apply_idx = locate_hunk_start(&original_lines, src_idx, hunk)?;
        while src_idx < apply_idx && src_idx < original_lines.len() {
            out.push(original_lines[src_idx].clone());
            src_idx += 1;
        }
        apply_hunk_at(&original_lines, &mut src_idx, &mut out, hunk)?;
    }

    while src_idx < original_lines.len() {
        out.push(original_lines[src_idx].clone());
        src_idx += 1;
    }

    Ok(out.concat())
}

#[derive(Debug, Clone)]
struct ParsedHunk {
    old_start: usize,
    new_start: usize,
    ops: Vec<PatchOp>,
}

#[derive(Debug, Clone)]
enum PatchOpKind {
    Context,
    Delete,
    Add,
}

#[derive(Debug, Clone)]
struct PatchOp {
    kind: PatchOpKind,
    text: String,
}

fn normalize_patch_text(patch: &str) -> String {
    let patch = patch.replace("\r\n", "\n");
    let mut lines = split_lines_lossless(&patch);
    if let Some(first) = lines.first() {
        let fence = strip_line_ending(first);
        if fence == "```" || fence == "```diff" || fence == "```patch" {
            lines.remove(0);
            if lines
                .last()
                .map(|line| strip_line_ending(line) == "```")
                .unwrap_or(false)
            {
                lines.pop();
            }
        }
    }
    lines.concat()
}

fn parse_patch_hunks(patch: &str) -> Result<Vec<ParsedHunk>, String> {
    let patch_lines = split_lines_lossless(patch);
    let mut hunks = Vec::new();
    let mut i = 0usize;

    while i < patch_lines.len() {
        let line = strip_line_ending(&patch_lines[i]);
        if should_skip_patch_metadata(line) {
            i += 1;
            continue;
        }
        if !line.starts_with("@@") {
            i += 1;
            continue;
        }

        let (old_start, old_count, new_start, new_count) = parse_hunk_header(line)?;
        i += 1;
        let mut ops: Vec<PatchOp> = Vec::new();

        while i < patch_lines.len() {
            let raw = &patch_lines[i];
            let line = strip_line_ending(raw);
            if line.starts_with("@@") {
                break;
            }
            if should_skip_patch_metadata(line) {
                break;
            }
            if line == r"\ No newline at end of file" {
                let Some(last) = ops.last_mut() else {
                    return Err("Patch has a no-newline marker without a preceding line".into());
                };
                last.text = strip_line_ending(&last.text).to_string();
                i += 1;
                continue;
            }

            let marker = raw
                .chars()
                .next()
                .ok_or_else(|| "Patch contains an empty hunk line".to_string())?;
            let text = raw.get(1..).unwrap_or_default().to_string();
            let kind = match marker {
                ' ' => PatchOpKind::Context,
                '-' => PatchOpKind::Delete,
                '+' => PatchOpKind::Add,
                _ => {
                    return Err(format!(
                        "Patch contains an invalid hunk line prefix {:?}",
                        marker
                    ))
                }
            };
            ops.push(PatchOp { kind, text });
            i += 1;
        }

        let counted_old = ops
            .iter()
            .filter(|op| matches!(op.kind, PatchOpKind::Context | PatchOpKind::Delete))
            .count();
        let counted_new = ops
            .iter()
            .filter(|op| matches!(op.kind, PatchOpKind::Context | PatchOpKind::Add))
            .count();
        if counted_old != old_count {
            return Err(format!(
                "Patch hunk old-count mismatch: header says {}, hunk contains {}",
                old_count, counted_old
            ));
        }
        if counted_new != new_count {
            return Err(format!(
                "Patch hunk new-count mismatch: header says {}, hunk contains {}",
                new_count, counted_new
            ));
        }

        hunks.push(ParsedHunk {
            old_start,
            new_start,
            ops,
        });
    }

    Ok(hunks)
}

fn should_skip_patch_metadata(line: &str) -> bool {
    line.starts_with("--- ")
        || line.starts_with("+++ ")
        || line.starts_with("diff ")
        || line.starts_with("index ")
}

fn parse_hunk_header(header: &str) -> Result<(usize, usize, usize, usize), String> {
    let rest = header
        .strip_prefix("@@")
        .and_then(|value| value.split("@@").next())
        .ok_or_else(|| "Invalid patch hunk header".to_string())?
        .trim();
    let old = rest
        .split_ascii_whitespace()
        .find(|part| part.starts_with('-'))
        .ok_or_else(|| "Patch hunk is missing old range".to_string())?;
    let new = rest
        .split_ascii_whitespace()
        .find(|part| part.starts_with('+'))
        .ok_or_else(|| "Patch hunk is missing new range".to_string())?;
    let (old_start, old_count) = parse_hunk_range(old, '-')?;
    let (new_start, new_count) = parse_hunk_range(new, '+')?;
    Ok((old_start, old_count, new_start, new_count))
}

fn parse_hunk_range(value: &str, prefix: char) -> Result<(usize, usize), String> {
    let value = value.trim_start_matches(prefix);
    let (start, count) = value.split_once(',').unwrap_or((value, "1"));
    Ok((
        start
            .parse::<usize>()
            .map_err(|_| "Invalid hunk start".to_string())?,
        count
            .parse::<usize>()
            .map_err(|_| "Invalid hunk count".to_string())?,
    ))
}

fn locate_hunk_start(
    original_lines: &[String],
    minimum_idx: usize,
    hunk: &ParsedHunk,
) -> Result<usize, String> {
    let search_start = minimum_idx.min(original_lines.len());
    let search_end = original_lines.len();
    let mut matches = Vec::new();
    for idx in search_start..=search_end {
        if hunk_matches_at(original_lines, idx, hunk) {
            matches.push(idx);
        }
    }

    match matches.as_slice() {
        [] => Err(format!(
            "Patch hunk near -{},+{} could not be matched against the current file",
            hunk.old_start, hunk.new_start
        )),
        [only] => Ok(*only),
        _ => Err(format!(
            "Patch hunk near -{},+{} matched multiple locations; refusing ambiguous apply",
            hunk.old_start, hunk.new_start
        )),
    }
}

fn hunk_matches_at(original_lines: &[String], start_idx: usize, hunk: &ParsedHunk) -> bool {
    let mut idx = start_idx;
    for op in &hunk.ops {
        match op.kind {
            PatchOpKind::Add => {}
            PatchOpKind::Context | PatchOpKind::Delete => {
                let Some(src) = original_lines.get(idx) else {
                    return false;
                };
                if src != &op.text {
                    return false;
                }
                idx += 1;
            }
        }
    }
    true
}

fn apply_hunk_at(
    original_lines: &[String],
    src_idx: &mut usize,
    out: &mut Vec<String>,
    hunk: &ParsedHunk,
) -> Result<(), String> {
    for op in &hunk.ops {
        match op.kind {
            PatchOpKind::Context => {
                let Some(src) = original_lines.get(*src_idx) else {
                    return Err("Patch context extends past end of file".into());
                };
                if src != &op.text {
                    return Err(format!(
                        "Patch context mismatch near line {}. Expected {:?}, got {:?}",
                        *src_idx + 1,
                        strip_line_ending(&op.text),
                        strip_line_ending(src),
                    ));
                }
                out.push(src.clone());
                *src_idx += 1;
            }
            PatchOpKind::Delete => {
                let Some(src) = original_lines.get(*src_idx) else {
                    return Err("Patch deletion extends past end of file".into());
                };
                if src != &op.text {
                    return Err(format!(
                        "Patch deletion mismatch near line {}. Expected {:?}, got {:?}",
                        *src_idx + 1,
                        strip_line_ending(&op.text),
                        strip_line_ending(src),
                    ));
                }
                *src_idx += 1;
            }
            PatchOpKind::Add => out.push(op.text.clone()),
        }
    }
    Ok(())
}

fn split_lines_lossless(value: &str) -> Vec<String> {
    if value.is_empty() {
        return vec![];
    }
    value
        .split_inclusive('\n')
        .map(ToString::to_string)
        .collect::<Vec<_>>()
}

fn strip_line_ending(value: &str) -> &str {
    value.trim_end_matches('\n').trim_end_matches('\r')
}

fn make_simple_diff(before: &str, after: &str) -> String {
    let before_lines = before.lines().collect::<Vec<_>>();
    let after_lines = after.lines().collect::<Vec<_>>();
    let max = before_lines.len().max(after_lines.len());
    let mut out = String::new();
    for idx in 0..max {
        let before = before_lines.get(idx);
        let after = after_lines.get(idx);
        match (before, after) {
            (Some(a), Some(b)) if a == b => {}
            (Some(a), Some(b)) => {
                out.push_str(&format!("-{}\n+{}\n", a, b));
            }
            (Some(a), None) => out.push_str(&format!("-{}\n", a)),
            (None, Some(b)) => out.push_str(&format!("+{}\n", b)),
            (None, None) => {}
        }
    }
    if out.is_empty() {
        "No textual changes detected.".into()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::apply_unified_patch;

    #[test]
    fn applies_basic_patch() {
        let original = "alpha\nbeta\ngamma\n";
        let patch = "\
@@ -1,3 +1,3 @@
 alpha
-beta
+delta
 gamma
";
        let updated = apply_unified_patch(original, patch).unwrap();
        assert_eq!(updated, "alpha\ndelta\ngamma\n");
    }

    #[test]
    fn applies_fenced_patch() {
        let original = "a\nb\n";
        let patch = "\
```diff
@@ -1,2 +1,2 @@
 a
-b
+c
```";
        let updated = apply_unified_patch(original, patch).unwrap();
        assert_eq!(updated, "a\nc\n");
    }

    #[test]
    fn applies_multiple_hunks() {
        let original = "a\nb\nc\nd\ne\n";
        let patch = "\
@@ -1,2 +1,2 @@
 a
-b
+bb
@@ -4,2 +4,2 @@
 d
-e
+ee
";
        let updated = apply_unified_patch(original, patch).unwrap();
        assert_eq!(updated, "a\nbb\nc\nd\nee\n");
    }

    #[test]
    fn relocates_hunk_when_line_numbers_drift() {
        let original = "header\nalpha\nbeta\ngamma\n";
        let patch = "\
@@ -1,3 +1,3 @@
 alpha
-beta
+delta
 gamma
";
        let updated = apply_unified_patch(original, patch).unwrap();
        assert_eq!(updated, "header\nalpha\ndelta\ngamma\n");
    }

    #[test]
    fn supports_no_newline_marker() {
        let original = "alpha";
        let patch = "\
@@ -1 +1 @@
-alpha
\\ No newline at end of file
+beta
\\ No newline at end of file
";
        let updated = apply_unified_patch(original, patch).unwrap();
        assert_eq!(updated, "beta");
    }

    #[test]
    fn rejects_ambiguous_hunk_matches() {
        let original = "same\nsame\n";
        let patch = "\
@@ -1,1 +1,1 @@
-same
+changed
";
        let error = apply_unified_patch(original, patch).unwrap_err();
        assert!(error.contains("matched multiple locations"));
    }
}
