use std::path::Path;

use raven_core::system_types::{ContainerInfo, ContainerType};

/// Detects if the application is running inside a container or sandbox.
pub struct ContainerInspector;

impl ContainerInspector {
    /// Detect the container environment using the real filesystem root.
    pub async fn detect() -> ContainerInfo {
        Self::detect_with_root(Path::new("/")).await
    }

    /// Detect the container environment using a custom root (for testing).
    pub async fn detect_with_root(root: &Path) -> ContainerInfo {
        // Check for Flatpak
        let flatpak_info = root.join(".flatpak-info");
        if flatpak_info.exists() {
            let app_id = parse_flatpak_info(&flatpak_info).await;
            return ContainerInfo {
                is_container: true,
                container_type: ContainerType::Flatpak,
                app_id,
            };
        }

        // Check for Docker
        let dockerenv = root.join(".dockerenv");
        if dockerenv.exists() {
            return ContainerInfo {
                is_container: true,
                container_type: ContainerType::Docker,
                app_id: None,
            };
        }

        // Check for Podman
        let containerenv = root.join("run").join(".containerenv");
        if containerenv.exists() {
            return ContainerInfo {
                is_container: true,
                container_type: ContainerType::Podman,
                app_id: None,
            };
        }

        // Check for systemd-nspawn
        if let Some(ct) = check_systemd_nspawn(root).await {
            return ct;
        }

        ContainerInfo {
            is_container: false,
            container_type: ContainerType::None,
            app_id: None,
        }
    }
}

/// Parse the Flatpak info file to extract the app ID.
async fn parse_flatpak_info(path: &Path) -> Option<String> {
    let content = tokio::fs::read_to_string(path).await.ok()?;
    parse_flatpak_app_id(&content)
}

/// Parse app-id from Flatpak info file content.
pub fn parse_flatpak_app_id(content: &str) -> Option<String> {
    let mut in_application_section = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed == "[Application]" {
            in_application_section = true;
            continue;
        }
        if trimmed.starts_with('[') {
            in_application_section = false;
            continue;
        }
        if in_application_section {
            if let Some(rest) = trimmed.strip_prefix("name=") {
                return Some(rest.trim().to_string());
            }
        }
    }
    None
}

/// Check /proc/1/environ for systemd-nspawn container marker.
async fn check_systemd_nspawn(root: &Path) -> Option<ContainerInfo> {
    let environ_path = root.join("proc").join("1").join("environ");
    let content = tokio::fs::read(&environ_path).await.ok()?;
    // environ entries are null-separated
    let environ_str = String::from_utf8_lossy(&content);
    for entry in environ_str.split('\0') {
        if entry == "container=systemd-nspawn" {
            return Some(ContainerInfo {
                is_container: true,
                container_type: ContainerType::SystemdNspawn,
                app_id: None,
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_flatpak_app_id_valid() {
        let content = "\
[Application]
name=org.gnome.Calculator
runtime=org.gnome.Platform/x86_64/45

[Instance]
app-path=/app
";
        let result = parse_flatpak_app_id(content);
        assert_eq!(result, Some("org.gnome.Calculator".to_string()));
    }

    #[test]
    fn test_parse_flatpak_app_id_no_application_section() {
        let content = "\
[Instance]
app-path=/app
";
        let result = parse_flatpak_app_id(content);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_flatpak_app_id_no_name() {
        let content = "\
[Application]
runtime=org.gnome.Platform/x86_64/45
";
        let result = parse_flatpak_app_id(content);
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_flatpak_app_id_empty() {
        assert!(parse_flatpak_app_id("").is_none());
    }

    #[tokio::test]
    async fn test_detect_with_empty_root() {
        let dir = tempfile::TempDir::new().unwrap();
        let info = ContainerInspector::detect_with_root(dir.path()).await;
        assert!(!info.is_container);
        assert_eq!(info.container_type, ContainerType::None);
        assert!(info.app_id.is_none());
    }

    #[tokio::test]
    async fn test_detect_flatpak() {
        let dir = tempfile::TempDir::new().unwrap();
        let flatpak_info = dir.path().join(".flatpak-info");
        std::fs::write(
            &flatpak_info,
            "[Application]\nname=com.example.TestApp\n",
        )
        .unwrap();

        let info = ContainerInspector::detect_with_root(dir.path()).await;
        assert!(info.is_container);
        assert_eq!(info.container_type, ContainerType::Flatpak);
        assert_eq!(info.app_id, Some("com.example.TestApp".to_string()));
    }

    #[tokio::test]
    async fn test_detect_docker() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join(".dockerenv"), "").unwrap();

        let info = ContainerInspector::detect_with_root(dir.path()).await;
        assert!(info.is_container);
        assert_eq!(info.container_type, ContainerType::Docker);
    }

    #[tokio::test]
    async fn test_detect_podman() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("run")).unwrap();
        std::fs::write(dir.path().join("run").join(".containerenv"), "").unwrap();

        let info = ContainerInspector::detect_with_root(dir.path()).await;
        assert!(info.is_container);
        assert_eq!(info.container_type, ContainerType::Podman);
    }

    #[tokio::test]
    async fn test_detect_systemd_nspawn() {
        let dir = tempfile::TempDir::new().unwrap();
        let proc_dir = dir.path().join("proc").join("1");
        std::fs::create_dir_all(&proc_dir).unwrap();
        // environ entries are null-separated
        std::fs::write(
            proc_dir.join("environ"),
            b"PATH=/usr/bin\0container=systemd-nspawn\0HOME=/root\0",
        )
        .unwrap();

        let info = ContainerInspector::detect_with_root(dir.path()).await;
        assert!(info.is_container);
        assert_eq!(info.container_type, ContainerType::SystemdNspawn);
    }

    #[tokio::test]
    async fn test_detect_priority_flatpak_over_docker() {
        // If both .flatpak-info and .dockerenv exist, Flatpak should win
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(
            dir.path().join(".flatpak-info"),
            "[Application]\nname=com.test.App\n",
        )
        .unwrap();
        std::fs::write(dir.path().join(".dockerenv"), "").unwrap();

        let info = ContainerInspector::detect_with_root(dir.path()).await;
        assert_eq!(info.container_type, ContainerType::Flatpak);
    }

    #[tokio::test]
    async fn test_detect_real_system() {
        // On a normal system, we should get None container type
        let info = ContainerInspector::detect().await;
        // We can't assert much about the real system, but it shouldn't panic
        let _ = info.is_container;
        let _ = info.container_type;
    }
}
