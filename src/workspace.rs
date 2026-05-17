use std::{
    io::{BufRead, BufReader, Read},
    path::{Component, Path, PathBuf},
    process::Command as StdCommand,
    sync::mpsc,
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
    workspaces_root().join(user_id.to_string()).join(workspace_uuid)
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
    file.write_all(content.as_bytes()).map_err(|e| format!("Could not append to file: {e}"))
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
    if exe == "git" && argv.iter().any(|arg| matches!(arg.as_str(), "clean" | "reset" | "checkout" | "restore")) {
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

    let timeout = if timeout_secs == 0 { DEFAULT_TIMEOUT_SECS } else { timeout_secs.min(MAX_TIMEOUT_SECS) };
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
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
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
            Some(FileEntry { name, path: rel, is_dir, size, children })
        })
        .collect()
}

pub fn file_tree_json(workspace_dir: &Path) -> String {
    let tree = file_tree(workspace_dir, workspace_dir, 0);
    serde_json::to_string(&tree).unwrap_or_else(|_| "[]".to_string())
}

fn apply_unified_patch(original: &str, patch: &str) -> Result<String, String> {
    let original_lines = split_lines_lossless(original);
    let patch_lines = split_lines_lossless(patch);
    let mut out = Vec::<String>::new();
    let mut src_idx = 0usize;
    let mut i = 0usize;
    let mut saw_hunk = false;

    while i < patch_lines.len() {
        let line = strip_line_ending(&patch_lines[i]);
        if line.starts_with("--- ") || line.starts_with("+++ ") || line.starts_with("diff ") || line.starts_with("index ") {
            i += 1;
            continue;
        }
        if !line.starts_with("@@") {
            i += 1;
            continue;
        }

        saw_hunk = true;
        let (old_start, _old_count) = parse_hunk_header(line)?;
        let target_idx = old_start.saturating_sub(1);
        while src_idx < target_idx && src_idx < original_lines.len() {
            out.push(original_lines[src_idx].clone());
            src_idx += 1;
        }
        i += 1;

        while i < patch_lines.len() {
            let raw = &patch_lines[i];
            let marker = raw.chars().next().unwrap_or(' ');
            let body = raw.get(1..).unwrap_or_default().to_string();
            let headerish = strip_line_ending(raw);
            if headerish.starts_with("@@") {
                break;
            }
            match marker {
                ' ' => {
                    let Some(src) = original_lines.get(src_idx) else {
                        return Err("Patch context extends past end of file".into());
                    };
                    if strip_line_ending(src) != strip_line_ending(&body) {
                        return Err(format!(
                            "Patch context mismatch near line {}. Expected {:?}, got {:?}",
                            src_idx + 1,
                            strip_line_ending(&body),
                            strip_line_ending(src),
                        ));
                    }
                    out.push(src.clone());
                    src_idx += 1;
                }
                '-' => {
                    let Some(src) = original_lines.get(src_idx) else {
                        return Err("Patch deletion extends past end of file".into());
                    };
                    if strip_line_ending(src) != strip_line_ending(&body) {
                        return Err(format!(
                            "Patch deletion mismatch near line {}. Expected {:?}, got {:?}",
                            src_idx + 1,
                            strip_line_ending(&body),
                            strip_line_ending(src),
                        ));
                    }
                    src_idx += 1;
                }
                '+' => out.push(body),
                '\\' => {}
                _ => return Err(format!("Unsupported patch line: {headerish}")),
            }
            i += 1;
        }
    }

    if !saw_hunk {
        return Err("Patch contains no unified diff hunks".into());
    }

    while src_idx < original_lines.len() {
        out.push(original_lines[src_idx].clone());
        src_idx += 1;
    }

    Ok(out.concat())
}

fn parse_hunk_header(header: &str) -> Result<(usize, usize), String> {
    let rest = header
        .strip_prefix("@@")
        .and_then(|value| value.split("@@").next())
        .ok_or_else(|| "Invalid patch hunk header".to_string())?
        .trim();
    let old = rest
        .split_ascii_whitespace()
        .find(|part| part.starts_with('-'))
        .ok_or_else(|| "Patch hunk is missing old range".to_string())?;
    let old = old.trim_start_matches('-');
    let (start, count) = old.split_once(',').unwrap_or((old, "1"));
    Ok((
        start.parse::<usize>().map_err(|_| "Invalid hunk start".to_string())?,
        count.parse::<usize>().map_err(|_| "Invalid hunk count".to_string())?,
    ))
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
