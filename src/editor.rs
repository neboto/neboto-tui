use crate::error::{Error, Result};
use std::env;
use std::io::Write;
use std::process::Command;

/// Spawn an editor with arbitrary text content — the single editor entry
/// point (what to open is decided in `App::open_in_editor`).
/// The `suffix` hint (e.g. ".yaml", ".json") sets the temp file extension so the
/// editor can pick the right syntax highlighting.
pub fn spawn_editor_with_content(content: &str, suffix: &str) -> Result<()> {
    let mut temp_file = tempfile::Builder::new()
        .suffix(suffix)
        .tempfile()
        .map_err(|e| Error::EditorFailed(format!("Failed to create temp file: {}", e)))?;

    temp_file
        .write_all(content.as_bytes())
        .map_err(|e| Error::EditorFailed(format!("Failed to write to temp file: {}", e)))?;

    temp_file
        .flush()
        .map_err(|e| Error::EditorFailed(format!("Failed to flush temp file: {}", e)))?;

    let (editor, args) = get_editor_command()?;
    spawn_editor(&editor, &args, temp_file.path().to_str().unwrap())?;
    Ok(())
}

/// Get the editor command from $EDITOR environment variable, or fallback to vim
/// Returns (program, args) tuple where args can contain multiple arguments
fn get_editor_command() -> Result<(String, Vec<String>)> {
    let editor_str = env::var("EDITOR").unwrap_or_else(|_| "vim".to_string());

    // Split the editor string into program and arguments
    let parts: Vec<String> = editor_str
        .split_whitespace()
        .map(|s| s.to_string())
        .collect();

    if parts.is_empty() {
        return Ok(("vim".to_string(), vec![]));
    }

    let program = parts[0].clone();
    let args = parts[1..].to_vec();

    Ok((program, args))
}

/// Spawn the editor process and wait for it to complete
fn spawn_editor(editor: &str, args: &[String], file_path: &str) -> Result<()> {
    use std::process::Stdio;

    // Launch editor with inherited stdio
    let mut cmd = Command::new(editor);

    // Add any arguments from $EDITOR
    for arg in args {
        cmd.arg(arg);
    }

    // Add the file path as the last argument
    cmd.arg(file_path);

    // Inherit stdio to allow full interaction with the editor
    cmd.stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());

    let status = cmd.status().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            Error::EditorNotFound(editor.to_string())
        } else {
            Error::EditorFailed(format!("Failed to spawn editor: {}", e))
        }
    })?;

    if !status.success() {
        return Err(Error::EditorFailed(format!(
            "Editor exited with non-zero status: {}",
            status
        )));
    }

    Ok(())
}
