use std::path::{Path, PathBuf};

use chrono::Local;
use tracing::{info, warn};

use raven_core::automation_types::Action;

/// Execute an action on a file. Returns a description of what was done.
pub async fn execute_action(action: &Action, file_path: &Path) -> Result<String, String> {
    match action {
        Action::MoveTo { destination } => execute_move(file_path, destination).await,
        Action::CopyTo { destination } => execute_copy(file_path, destination).await,
        Action::Rename { pattern } => execute_rename(file_path, pattern).await,
        Action::Delete => execute_delete(file_path).await,
        Action::Trash => execute_trash(file_path).await,
        Action::RunCommand { command, args } => {
            execute_run_command(file_path, command, args).await
        }
        Action::Notify { message } => execute_notify(file_path, message),
    }
}

async fn execute_move(file_path: &Path, destination: &Path) -> Result<String, String> {
    tokio::fs::create_dir_all(destination)
        .await
        .map_err(|e| format!("failed to create destination directory: {}", e))?;

    let file_name = file_path
        .file_name()
        .ok_or_else(|| "file has no name".to_string())?;
    let dest_path = destination.join(file_name);

    // Try rename first, fall back to copy+delete for cross-device moves
    match tokio::fs::rename(file_path, &dest_path).await {
        Ok(()) => {}
        Err(e) => {
            // EXDEV (errno 18) or other cross-device errors
            if e.raw_os_error() == Some(18) {
                tokio::fs::copy(file_path, &dest_path)
                    .await
                    .map_err(|e| format!("failed to copy file during cross-device move: {}", e))?;
                tokio::fs::remove_file(file_path)
                    .await
                    .map_err(|e| format!("failed to remove source after cross-device move: {}", e))?;
            } else {
                return Err(format!("failed to move file: {}", e));
            }
        }
    }

    let desc = format!(
        "Moved {} to {}",
        file_path.display(),
        dest_path.display()
    );
    info!("{}", desc);
    Ok(desc)
}

async fn execute_copy(file_path: &Path, destination: &Path) -> Result<String, String> {
    tokio::fs::create_dir_all(destination)
        .await
        .map_err(|e| format!("failed to create destination directory: {}", e))?;

    let file_name = file_path
        .file_name()
        .ok_or_else(|| "file has no name".to_string())?;
    let dest_path = destination.join(file_name);

    tokio::fs::copy(file_path, &dest_path)
        .await
        .map_err(|e| format!("failed to copy file: {}", e))?;

    let desc = format!(
        "Copied {} to {}",
        file_path.display(),
        dest_path.display()
    );
    info!("{}", desc);
    Ok(desc)
}

async fn execute_rename(file_path: &Path, pattern: &str) -> Result<String, String> {
    let name = file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let ext = file_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    let date = Local::now().format("%Y-%m-%d").to_string();

    let new_name = pattern
        .replace("{name}", name)
        .replace("{ext}", ext)
        .replace("{date}", &date);

    let parent = file_path
        .parent()
        .ok_or_else(|| "file has no parent directory".to_string())?;
    let new_path = parent.join(&new_name);

    tokio::fs::rename(file_path, &new_path)
        .await
        .map_err(|e| format!("failed to rename file: {}", e))?;

    let desc = format!(
        "Renamed {} to {}",
        file_path.display(),
        new_path.display()
    );
    info!("{}", desc);
    Ok(desc)
}

async fn execute_delete(file_path: &Path) -> Result<String, String> {
    tokio::fs::remove_file(file_path)
        .await
        .map_err(|e| format!("failed to delete file: {}", e))?;

    let desc = format!("Deleted {}", file_path.display());
    info!("{}", desc);
    Ok(desc)
}

async fn execute_trash(file_path: &Path) -> Result<String, String> {
    let home = std::env::var("HOME")
        .map_err(|_| "HOME environment variable not set".to_string())?;
    let trash_dir = PathBuf::from(&home).join(".local/share/Trash");
    let files_dir = trash_dir.join("files");
    let info_dir = trash_dir.join("info");

    tokio::fs::create_dir_all(&files_dir)
        .await
        .map_err(|e| format!("failed to create trash files dir: {}", e))?;
    tokio::fs::create_dir_all(&info_dir)
        .await
        .map_err(|e| format!("failed to create trash info dir: {}", e))?;

    let file_name = file_path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| "file has no name".to_string())?;

    // Generate unique trash name
    let (trash_name, trash_path) = unique_trash_name(&files_dir, file_name).await?;

    // Canonicalize path for trashinfo
    let canonical = tokio::fs::canonicalize(file_path)
        .await
        .map_err(|e| format!("failed to canonicalize path: {}", e))?;

    let deletion_date = Local::now().format("%Y-%m-%dT%H:%M:%S").to_string();
    let trashinfo_content = format!(
        "[Trash Info]\nPath={}\nDeletionDate={}\n",
        canonical.display(),
        deletion_date
    );
    let trashinfo_path = info_dir.join(format!("{}.trashinfo", trash_name));

    tokio::fs::write(&trashinfo_path, trashinfo_content.as_bytes())
        .await
        .map_err(|e| format!("failed to write trashinfo: {}", e))?;

    match tokio::fs::rename(file_path, &trash_path).await {
        Ok(()) => {}
        Err(e) if e.raw_os_error() == Some(18) => {
            // Cross-device: copy then delete
            tokio::fs::copy(file_path, &trash_path)
                .await
                .map_err(|e| format!("failed to copy file to trash: {}", e))?;
            tokio::fs::remove_file(file_path)
                .await
                .map_err(|e| format!("failed to remove original after trash: {}", e))?;
        }
        Err(e) => {
            // Clean up trashinfo on failure
            let _ = tokio::fs::remove_file(&trashinfo_path).await;
            return Err(format!("failed to move file to trash: {}", e));
        }
    }

    let desc = format!("Trashed {}", file_path.display());
    info!("{}", desc);
    Ok(desc)
}

async fn unique_trash_name(files_dir: &Path, name: &str) -> Result<(String, PathBuf), String> {
    let candidate = files_dir.join(name);
    if !candidate.exists() {
        return Ok((name.to_string(), candidate));
    }

    let (stem, ext) = match name.rfind('.') {
        Some(pos) => (&name[..pos], Some(&name[pos..])),
        None => (name, None),
    };

    for i in 2u32..100_000 {
        let new_name = match ext {
            Some(ext) => format!("{}.{}{}", stem, i, ext),
            None => format!("{}.{}", stem, i),
        };
        let candidate = files_dir.join(&new_name);
        if !candidate.exists() {
            return Ok((new_name, candidate));
        }
    }

    Err(format!(
        "unable to generate unique trash name for {}",
        name
    ))
}

async fn execute_run_command(
    file_path: &Path,
    command: &str,
    args: &[String],
) -> Result<String, String> {
    let file_str = file_path.to_string_lossy().to_string();
    let resolved_args: Vec<String> = args
        .iter()
        .map(|a| a.replace("{file}", &file_str))
        .collect();

    let output = tokio::process::Command::new(command)
        .args(&resolved_args)
        .output()
        .await
        .map_err(|e| format!("failed to run command '{}': {}", command, e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let msg = format!(
            "command '{}' exited with status {}: {}",
            command,
            output.status,
            stderr.trim()
        );
        warn!("{}", msg);
        return Err(msg);
    }

    let desc = format!(
        "Ran command '{}' with args {:?} on {}",
        command, resolved_args, file_path.display()
    );
    info!("{}", desc);
    Ok(desc)
}

fn execute_notify(file_path: &Path, message: &str) -> Result<String, String> {
    let file_str = file_path.to_string_lossy().to_string();
    let resolved = message.replace("{file}", &file_str);
    let desc = format!("Notification: {}", resolved);
    info!("{}", desc);
    Ok(desc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn make_test_file(dir: &TempDir, name: &str, content: &[u8]) -> PathBuf {
        let path = dir.path().join(name);
        tokio::fs::write(&path, content).await.unwrap();
        path
    }

    #[tokio::test]
    async fn test_move_to() {
        let src_dir = TempDir::new().unwrap();
        let dst_dir = TempDir::new().unwrap();
        let file_path = make_test_file(&src_dir, "moveme.txt", b"move data").await;

        let action = Action::MoveTo {
            destination: dst_dir.path().to_path_buf(),
        };
        let result = execute_action(&action, &file_path).await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("Moved"));

        // Source should be gone
        assert!(!file_path.exists());
        // Destination should have the file
        assert!(dst_dir.path().join("moveme.txt").exists());
    }

    #[tokio::test]
    async fn test_move_to_creates_destination() {
        let src_dir = TempDir::new().unwrap();
        let dst_base = TempDir::new().unwrap();
        let dst_dir = dst_base.path().join("sub/dir");
        let file_path = make_test_file(&src_dir, "test.txt", b"data").await;

        let action = Action::MoveTo {
            destination: dst_dir.clone(),
        };
        let result = execute_action(&action, &file_path).await;
        assert!(result.is_ok());
        assert!(dst_dir.join("test.txt").exists());
    }

    #[tokio::test]
    async fn test_copy_to() {
        let src_dir = TempDir::new().unwrap();
        let dst_dir = TempDir::new().unwrap();
        let file_path = make_test_file(&src_dir, "copyme.txt", b"copy data").await;

        let action = Action::CopyTo {
            destination: dst_dir.path().to_path_buf(),
        };
        let result = execute_action(&action, &file_path).await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("Copied"));

        // Source should still exist
        assert!(file_path.exists());
        // Destination should have the file
        assert!(dst_dir.path().join("copyme.txt").exists());

        // Content should match
        let content = tokio::fs::read_to_string(dst_dir.path().join("copyme.txt"))
            .await
            .unwrap();
        assert_eq!(content, "copy data");
    }

    #[tokio::test]
    async fn test_rename() {
        let dir = TempDir::new().unwrap();
        let file_path = make_test_file(&dir, "original.txt", b"data").await;

        let action = Action::Rename {
            pattern: "{name}_backup.{ext}".to_string(),
        };
        let result = execute_action(&action, &file_path).await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("Renamed"));

        // Original should be gone
        assert!(!file_path.exists());
        // New name should exist
        assert!(dir.path().join("original_backup.txt").exists());
    }

    #[tokio::test]
    async fn test_rename_with_date() {
        let dir = TempDir::new().unwrap();
        let file_path = make_test_file(&dir, "report.pdf", b"pdf data").await;

        let action = Action::Rename {
            pattern: "{name}_{date}.{ext}".to_string(),
        };
        let result = execute_action(&action, &file_path).await;
        assert!(result.is_ok());

        let today = Local::now().format("%Y-%m-%d").to_string();
        let expected_name = format!("report_{}.pdf", today);
        assert!(dir.path().join(expected_name).exists());
    }

    #[tokio::test]
    async fn test_delete() {
        let dir = TempDir::new().unwrap();
        let file_path = make_test_file(&dir, "deleteme.txt", b"bye").await;

        let action = Action::Delete;
        let result = execute_action(&action, &file_path).await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("Deleted"));
        assert!(!file_path.exists());
    }

    #[tokio::test]
    async fn test_delete_nonexistent() {
        let path = PathBuf::from("/tmp/raven_nonexistent_file_test_12345");
        let action = Action::Delete;
        let result = execute_action(&action, &path).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_trash() {
        let dir = TempDir::new().unwrap();
        let file_path = make_test_file(&dir, "trashme.txt", b"trash data").await;

        let action = Action::Trash;
        let result = execute_action(&action, &file_path).await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("Trashed"));
        assert!(!file_path.exists());
    }

    #[tokio::test]
    async fn test_run_command() {
        let dir = TempDir::new().unwrap();
        let file_path = make_test_file(&dir, "test.txt", b"data").await;

        let action = Action::RunCommand {
            command: "echo".to_string(),
            args: vec!["processing".to_string(), "{file}".to_string()],
        };
        let result = execute_action(&action, &file_path).await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("Ran command"));
    }

    #[tokio::test]
    async fn test_run_command_failure() {
        let dir = TempDir::new().unwrap();
        let file_path = make_test_file(&dir, "test.txt", b"data").await;

        let action = Action::RunCommand {
            command: "false".to_string(),
            args: vec![],
        };
        let result = execute_action(&action, &file_path).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_run_command_not_found() {
        let dir = TempDir::new().unwrap();
        let file_path = make_test_file(&dir, "test.txt", b"data").await;

        let action = Action::RunCommand {
            command: "nonexistent_command_12345".to_string(),
            args: vec![],
        };
        let result = execute_action(&action, &file_path).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_notify() {
        let dir = TempDir::new().unwrap();
        let file_path = make_test_file(&dir, "test.txt", b"data").await;

        let action = Action::Notify {
            message: "File {file} was processed".to_string(),
        };
        let result = execute_action(&action, &file_path).await;
        assert!(result.is_ok());
        let desc = result.unwrap();
        assert!(desc.contains("Notification"));
        assert!(desc.contains("test.txt"));
    }
}
