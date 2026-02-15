use std::path::Path;

use raven_core::error::{RavenError, RavenResult};
use raven_core::system_types::SystemdUnit;

/// Parses systemd unit files and queries unit status.
pub struct SystemdInspector;

impl SystemdInspector {
    /// Parse a systemd unit file (.service, .timer, .socket, etc.).
    pub async fn parse_unit_file(path: &Path) -> RavenResult<SystemdUnit> {
        let content =
            tokio::fs::read_to_string(path)
                .await
                .map_err(|e| RavenError::System {
                    message: format!("failed to read unit file {}: {}", path.display(), e),
                })?;

        parse_unit_content(&content, path)
    }

    /// Query whether a systemd unit is active.
    pub async fn get_unit_status(unit_name: &str) -> RavenResult<Option<String>> {
        let output = tokio::process::Command::new("systemctl")
            .args(["is-active", unit_name])
            .output()
            .await
            .map_err(|e| RavenError::System {
                message: format!("failed to run systemctl: {}", e),
            })?;

        let status = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if status.is_empty() {
            Ok(None)
        } else {
            Ok(Some(status))
        }
    }
}

/// Parse systemd unit file content into a SystemdUnit struct.
pub fn parse_unit_content(content: &str, path: &Path) -> RavenResult<SystemdUnit> {
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();

    // Determine unit type from extension
    let unit_type = path
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    let mut description = None;
    let mut properties = Vec::new();
    let mut current_section = String::new();

    for line in content.lines() {
        let trimmed = line.trim();

        // Skip empty lines and comments
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with(';') {
            continue;
        }

        // Section header
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            current_section = trimmed[1..trimmed.len() - 1].to_string();
            continue;
        }

        // Key=Value pair
        if let Some(eq_pos) = trimmed.find('=') {
            let key = trimmed[..eq_pos].trim().to_string();
            let value = trimmed[eq_pos + 1..].trim().to_string();

            // Capture Description from [Unit] section
            if current_section == "Unit" && key == "Description" {
                description = Some(value.clone());
            }

            let full_key = if current_section.is_empty() {
                key
            } else {
                format!("{}.{}", current_section, key)
            };
            properties.push((full_key, value));
        }
    }

    Ok(SystemdUnit {
        name: file_name,
        unit_type,
        description,
        active_state: None,
        properties,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const SAMPLE_SERVICE: &str = "\
[Unit]
Description=My Test Service
After=network.target
Requires=network-online.target

[Service]
Type=simple
ExecStart=/usr/bin/myapp --config /etc/myapp.conf
ExecReload=/bin/kill -HUP $MAINPID
Restart=on-failure
User=myapp
Group=myapp

[Install]
WantedBy=multi-user.target
";

    const SAMPLE_TIMER: &str = "\
[Unit]
Description=Run backup daily

[Timer]
OnCalendar=daily
Persistent=true
RandomizedDelaySec=1h

[Install]
WantedBy=timers.target
";

    #[test]
    fn test_parse_service_file() {
        let path = PathBuf::from("/etc/systemd/system/myapp.service");
        let unit = parse_unit_content(SAMPLE_SERVICE, &path).unwrap();

        assert_eq!(unit.name, "myapp.service");
        assert_eq!(unit.unit_type, "service");
        assert_eq!(
            unit.description,
            Some("My Test Service".to_string())
        );

        // Check some properties
        let props: std::collections::HashMap<_, _> =
            unit.properties.iter().cloned().collect();
        assert_eq!(
            props.get("Service.ExecStart"),
            Some(&"/usr/bin/myapp --config /etc/myapp.conf".to_string())
        );
        assert_eq!(
            props.get("Service.Restart"),
            Some(&"on-failure".to_string())
        );
        assert_eq!(
            props.get("Unit.After"),
            Some(&"network.target".to_string())
        );
        assert_eq!(
            props.get("Install.WantedBy"),
            Some(&"multi-user.target".to_string())
        );
    }

    #[test]
    fn test_parse_timer_file() {
        let path = PathBuf::from("/etc/systemd/system/backup.timer");
        let unit = parse_unit_content(SAMPLE_TIMER, &path).unwrap();

        assert_eq!(unit.name, "backup.timer");
        assert_eq!(unit.unit_type, "timer");
        assert_eq!(
            unit.description,
            Some("Run backup daily".to_string())
        );

        let props: std::collections::HashMap<_, _> =
            unit.properties.iter().cloned().collect();
        assert_eq!(
            props.get("Timer.OnCalendar"),
            Some(&"daily".to_string())
        );
        assert_eq!(
            props.get("Timer.Persistent"),
            Some(&"true".to_string())
        );
    }

    #[test]
    fn test_parse_empty_file() {
        let path = PathBuf::from("empty.service");
        let unit = parse_unit_content("", &path).unwrap();
        assert_eq!(unit.name, "empty.service");
        assert!(unit.description.is_none());
        assert!(unit.properties.is_empty());
    }

    #[test]
    fn test_parse_comments_and_blank_lines() {
        let content = "\
# This is a comment
; Another comment

[Unit]
Description=Test

# Comment inside section
";
        let path = PathBuf::from("test.service");
        let unit = parse_unit_content(content, &path).unwrap();
        assert_eq!(unit.description, Some("Test".to_string()));
        assert_eq!(unit.properties.len(), 1);
    }

    #[test]
    fn test_parse_value_with_equals() {
        let content = "\
[Service]
Environment=FOO=bar=baz
";
        let path = PathBuf::from("test.service");
        let unit = parse_unit_content(content, &path).unwrap();
        let props: std::collections::HashMap<_, _> =
            unit.properties.iter().cloned().collect();
        assert_eq!(
            props.get("Service.Environment"),
            Some(&"FOO=bar=baz".to_string())
        );
    }

    #[test]
    fn test_parse_no_extension() {
        let path = PathBuf::from("myunit");
        let unit = parse_unit_content("[Unit]\nDescription=Test\n", &path).unwrap();
        assert_eq!(unit.unit_type, "unknown");
    }

    #[tokio::test]
    async fn test_parse_unit_file_from_disk() {
        let dir = tempfile::TempDir::new().unwrap();
        let unit_path = dir.path().join("test.service");
        std::fs::write(&unit_path, SAMPLE_SERVICE).unwrap();

        let unit = SystemdInspector::parse_unit_file(&unit_path).await.unwrap();
        assert_eq!(unit.name, "test.service");
        assert_eq!(unit.description, Some("My Test Service".to_string()));
    }

    #[tokio::test]
    async fn test_parse_nonexistent_file() {
        let result = SystemdInspector::parse_unit_file(Path::new("/nonexistent/unit.service")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_unit_status() {
        // This will work on any systemd-based system
        if !Path::new("/usr/bin/systemctl").exists() {
            return;
        }
        // systemd-journald should always be running
        let result = SystemdInspector::get_unit_status("systemd-journald.service").await;
        assert!(result.is_ok());
        if let Ok(Some(status)) = result {
            // Could be "active" or "inactive" etc.
            assert!(!status.is_empty());
        }
    }

    #[tokio::test]
    async fn test_get_unit_status_nonexistent() {
        if !Path::new("/usr/bin/systemctl").exists() {
            return;
        }
        let result =
            SystemdInspector::get_unit_status("this-unit-surely-does-not-exist.service").await;
        assert!(result.is_ok());
        // systemctl is-active returns "inactive" or "unknown" for nonexistent units
    }
}
