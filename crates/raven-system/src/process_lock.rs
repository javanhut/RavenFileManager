use std::path::Path;

use raven_core::error::{RavenError, RavenResult};
use raven_core::system_types::{FdType, ProcessLock};

/// Detects which processes have a given file open by scanning /proc.
pub struct ProcessLockDetector;

impl ProcessLockDetector {
    /// Check which processes have the given file open.
    ///
    /// Scans `/proc/*/fd` symlinks and compares targets against the canonical
    /// path of the given file. Permission errors on inaccessible PIDs are
    /// silently skipped.
    pub async fn check_locks(path: &Path) -> RavenResult<Vec<ProcessLock>> {
        Self::check_locks_with_proc(path, Path::new("/proc")).await
    }

    /// Check locks using a custom proc root (for testing).
    pub async fn check_locks_with_proc(
        path: &Path,
        proc_root: &Path,
    ) -> RavenResult<Vec<ProcessLock>> {
        let canonical = tokio::fs::canonicalize(path)
            .await
            .map_err(|e| RavenError::System {
                message: format!("failed to canonicalize {}: {}", path.display(), e),
            })?;

        let mut locks = Vec::new();

        let mut proc_entries = match tokio::fs::read_dir(proc_root).await {
            Ok(entries) => entries,
            Err(e) => {
                return Err(RavenError::System {
                    message: format!("failed to read {}: {}", proc_root.display(), e),
                });
            }
        };

        while let Ok(Some(entry)) = proc_entries.next_entry().await {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();

            // Only process numeric directories (PIDs)
            let pid: u32 = match name_str.parse() {
                Ok(p) => p,
                Err(_) => continue,
            };

            let fd_dir = proc_root.join(name_str.as_ref()).join("fd");
            let mut fd_entries = match tokio::fs::read_dir(&fd_dir).await {
                Ok(entries) => entries,
                Err(_) => continue, // Permission denied or process exited
            };

            while let Ok(Some(fd_entry)) = fd_entries.next_entry().await {
                let fd_path = fd_entry.path();
                let link_target = match tokio::fs::read_link(&fd_path).await {
                    Ok(target) => target,
                    Err(_) => continue,
                };

                if link_target == canonical {
                    let process_name = read_process_name(proc_root, pid).await;
                    let fd_num = fd_entry.file_name().to_string_lossy().to_string();
                    let fd_type = read_fd_type(proc_root, pid, &fd_num).await;

                    locks.push(ProcessLock {
                        pid,
                        process_name,
                        fd_type,
                        fd_path: link_target.clone(),
                    });
                }
            }
        }

        Ok(locks)
    }
}

/// Read the process name from /proc/<pid>/comm.
async fn read_process_name(proc_root: &Path, pid: u32) -> String {
    let comm_path = proc_root.join(pid.to_string()).join("comm");
    match tokio::fs::read_to_string(&comm_path).await {
        Ok(name) => name.trim().to_string(),
        Err(_) => format!("[pid {}]", pid),
    }
}

/// Determine the fd type by reading /proc/<pid>/fdinfo/<fd>.
///
/// The `flags` field in fdinfo contains the open flags:
/// - 0 (O_RDONLY) → Read
/// - 1 (O_WRONLY) → Write
/// - 2 (O_RDWR) → ReadWrite
async fn read_fd_type(proc_root: &Path, pid: u32, fd: &str) -> FdType {
    let fdinfo_path = proc_root.join(pid.to_string()).join("fdinfo").join(fd);
    match tokio::fs::read_to_string(&fdinfo_path).await {
        Ok(content) => parse_fdinfo_flags(&content),
        Err(_) => FdType::Read, // Default if we can't read fdinfo
    }
}

/// Parse the flags field from fdinfo content.
pub fn parse_fdinfo_flags(content: &str) -> FdType {
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("flags:") {
            let flags_str = rest.trim();
            // The flags are in octal. The low 2 bits encode access mode.
            if let Ok(flags) = u32::from_str_radix(flags_str.trim_start_matches('0'), 8) {
                return match flags & 0o3 {
                    0 => FdType::Read,
                    1 => FdType::Write,
                    2 => FdType::ReadWrite,
                    _ => FdType::Read,
                };
            }
            // Try decimal parse as fallback
            if let Ok(flags) = flags_str.parse::<u32>() {
                return match flags & 0o3 {
                    0 => FdType::Read,
                    1 => FdType::Write,
                    2 => FdType::ReadWrite,
                    _ => FdType::Read,
                };
            }
        }
    }
    FdType::Read
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_fdinfo_flags_readonly() {
        let content = "pos:\t0\nflags:\t0100000\nmnt_id:\t28\n";
        assert_eq!(parse_fdinfo_flags(content), FdType::Read);
    }

    #[test]
    fn test_parse_fdinfo_flags_writeonly() {
        let content = "pos:\t0\nflags:\t0100001\nmnt_id:\t28\n";
        assert_eq!(parse_fdinfo_flags(content), FdType::Write);
    }

    #[test]
    fn test_parse_fdinfo_flags_readwrite() {
        let content = "pos:\t0\nflags:\t0100002\nmnt_id:\t28\n";
        assert_eq!(parse_fdinfo_flags(content), FdType::ReadWrite);
    }

    #[test]
    fn test_parse_fdinfo_flags_empty() {
        assert_eq!(parse_fdinfo_flags(""), FdType::Read);
    }

    #[tokio::test]
    async fn test_check_locks_nonexistent_file() {
        let result = ProcessLockDetector::check_locks(Path::new("/tmp/nonexistent_file_99999")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_detect_self_opened_file() {
        use std::io::Write;

        // Create a temp file and keep it open
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        write!(tmpfile, "test content").unwrap();
        let path = tmpfile.path().to_path_buf();

        // Our process has this file open; check_locks should find us
        let locks = ProcessLockDetector::check_locks(&path).await.unwrap();

        let my_pid = std::process::id();
        let found_self = locks.iter().any(|l| l.pid == my_pid);
        assert!(
            found_self,
            "Expected to find own PID {} in locks: {:?}",
            my_pid, locks
        );
    }

    #[tokio::test]
    async fn test_check_locks_permission_handling() {
        // Even if we can't read all /proc entries, the function should not error
        use std::io::Write;

        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        write!(tmpfile, "test").unwrap();
        let path = tmpfile.path().to_path_buf();

        let result = ProcessLockDetector::check_locks(&path).await;
        assert!(result.is_ok());
    }
}
