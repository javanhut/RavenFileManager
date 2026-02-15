use std::path::Path;

use raven_core::error::{RavenError, RavenResult};
use raven_core::system_types::{PackageInfo, PackageManager};

/// Detects the system package manager and queries file ownership.
pub struct PackageLookup {
    manager: PackageManager,
}

impl PackageLookup {
    /// Create a new PackageLookup, auto-detecting the system package manager.
    pub fn new() -> Self {
        Self {
            manager: Self::detect(),
        }
    }

    /// Create a PackageLookup with a specific package manager.
    pub fn with_manager(manager: PackageManager) -> Self {
        Self { manager }
    }

    /// Detect which package manager is available on the system.
    pub fn detect() -> PackageManager {
        if Path::new("/usr/bin/pacman").exists() {
            PackageManager::Pacman
        } else if Path::new("/usr/bin/dpkg").exists() {
            PackageManager::Dpkg
        } else if Path::new("/usr/bin/rpm").exists() {
            PackageManager::Rpm
        } else {
            PackageManager::Unknown
        }
    }

    /// Query which package owns the given file path.
    pub async fn query_owner(&self, path: &Path) -> RavenResult<Option<PackageInfo>> {
        match self.manager {
            PackageManager::Pacman => self.query_pacman(path).await,
            PackageManager::Dpkg => self.query_dpkg(path).await,
            PackageManager::Rpm => self.query_rpm(path).await,
            PackageManager::Unknown => Ok(None),
        }
    }

    /// Returns the detected package manager.
    pub fn manager(&self) -> PackageManager {
        self.manager
    }

    async fn query_pacman(&self, path: &Path) -> RavenResult<Option<PackageInfo>> {
        let output = tokio::process::Command::new("pacman")
            .args(["-Qo", &path.to_string_lossy()])
            .output()
            .await
            .map_err(|e| RavenError::System {
                message: format!("failed to run pacman: {}", e),
            })?;

        if !output.status.success() {
            // "error: No package owns <path>" is a normal result
            return Ok(None);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_pacman_output(&stdout)
    }

    async fn query_dpkg(&self, path: &Path) -> RavenResult<Option<PackageInfo>> {
        let output = tokio::process::Command::new("dpkg")
            .args(["-S", &path.to_string_lossy()])
            .output()
            .await
            .map_err(|e| RavenError::System {
                message: format!("failed to run dpkg: {}", e),
            })?;

        if !output.status.success() {
            return Ok(None);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_dpkg_output(&stdout, path)
    }

    async fn query_rpm(&self, path: &Path) -> RavenResult<Option<PackageInfo>> {
        let output = tokio::process::Command::new("rpm")
            .args(["-qf", "--queryformat", "%{NAME} %{VERSION}", &path.to_string_lossy()])
            .output()
            .await
            .map_err(|e| RavenError::System {
                message: format!("failed to run rpm: {}", e),
            })?;

        if !output.status.success() {
            return Ok(None);
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_rpm_output(&stdout)
    }
}

/// Parse pacman -Qo output: "<path> is owned by <package> <version>"
pub fn parse_pacman_output(output: &str) -> RavenResult<Option<PackageInfo>> {
    // Format: "/usr/bin/ls is owned by coreutils 9.4-3"
    let line = output.trim();
    if let Some(idx) = line.find("is owned by ") {
        let rest = &line[idx + "is owned by ".len()..];
        let parts: Vec<&str> = rest.rsplitn(2, ' ').collect();
        if parts.len() == 2 {
            return Ok(Some(PackageInfo {
                name: parts[1].to_string(),
                version: parts[0].to_string(),
                manager: PackageManager::Pacman,
                description: None,
            }));
        }
    }
    Ok(None)
}

/// Parse dpkg -S output: "<package>: <path>"
pub fn parse_dpkg_output(output: &str, _path: &Path) -> RavenResult<Option<PackageInfo>> {
    let line = output.lines().next().unwrap_or("").trim();
    // dpkg error messages start with "dpkg-query:" or "dpkg:"
    if line.starts_with("dpkg-query:") || line.starts_with("dpkg:") {
        return Ok(None);
    }
    if let Some(idx) = line.find(": ") {
        let pkg_name = &line[..idx];
        // dpkg -S doesn't include version, so we just return the name
        return Ok(Some(PackageInfo {
            name: pkg_name.to_string(),
            version: String::new(),
            manager: PackageManager::Dpkg,
            description: None,
        }));
    }
    Ok(None)
}

/// Parse rpm -qf output: "<name> <version>"
pub fn parse_rpm_output(output: &str) -> RavenResult<Option<PackageInfo>> {
    let line = output.trim();
    if line.is_empty() || line.starts_with("file ") {
        return Ok(None);
    }
    let parts: Vec<&str> = line.splitn(2, ' ').collect();
    if parts.len() == 2 {
        Ok(Some(PackageInfo {
            name: parts[0].to_string(),
            version: parts[1].to_string(),
            manager: PackageManager::Rpm,
            description: None,
        }))
    } else {
        Ok(Some(PackageInfo {
            name: line.to_string(),
            version: String::new(),
            manager: PackageManager::Rpm,
            description: None,
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn test_parse_pacman_output_valid() {
        let output = "/usr/bin/ls is owned by coreutils 9.4-3\n";
        let result = parse_pacman_output(output).unwrap();
        let pkg = result.unwrap();
        assert_eq!(pkg.name, "coreutils");
        assert_eq!(pkg.version, "9.4-3");
        assert_eq!(pkg.manager, PackageManager::Pacman);
    }

    #[test]
    fn test_parse_pacman_output_no_owner() {
        let output = "error: No package owns /tmp/doesnotexist\n";
        let result = parse_pacman_output(output).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_dpkg_output_valid() {
        let output = "coreutils: /usr/bin/ls\n";
        let path = PathBuf::from("/usr/bin/ls");
        let result = parse_dpkg_output(output, &path).unwrap();
        let pkg = result.unwrap();
        assert_eq!(pkg.name, "coreutils");
        assert_eq!(pkg.manager, PackageManager::Dpkg);
    }

    #[test]
    fn test_parse_dpkg_output_no_match() {
        let output = "dpkg-query: no path found matching pattern /nonexistent\n";
        let path = PathBuf::from("/nonexistent");
        let result = parse_dpkg_output(output, &path).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_rpm_output_valid() {
        let output = "coreutils 9.4\n";
        let result = parse_rpm_output(output).unwrap();
        let pkg = result.unwrap();
        assert_eq!(pkg.name, "coreutils");
        assert_eq!(pkg.version, "9.4");
        assert_eq!(pkg.manager, PackageManager::Rpm);
    }

    #[test]
    fn test_parse_rpm_output_not_owned() {
        let output = "file /nonexistent is not owned by any package\n";
        let result = parse_rpm_output(output).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_parse_rpm_output_empty() {
        let output = "";
        let result = parse_rpm_output(output).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_detect_package_manager() {
        let mgr = PackageLookup::detect();
        // On Arch Linux, this should be Pacman
        // On other systems, it may differ — we just verify it returns a valid variant
        match mgr {
            PackageManager::Pacman
            | PackageManager::Dpkg
            | PackageManager::Rpm
            | PackageManager::Unknown => {}
        }
    }

    #[test]
    fn test_with_manager() {
        let lookup = PackageLookup::with_manager(PackageManager::Dpkg);
        assert_eq!(lookup.manager(), PackageManager::Dpkg);
    }

    #[tokio::test]
    async fn test_query_owner_system_file() {
        let lookup = PackageLookup::new();
        if lookup.manager() == PackageManager::Unknown {
            return; // Skip on unsupported systems
        }
        // /usr/bin/ls should be owned by a package on any managed system
        let result = lookup.query_owner(Path::new("/usr/bin/ls")).await;
        assert!(result.is_ok());
        if let Ok(Some(pkg)) = result {
            assert!(!pkg.name.is_empty());
        }
    }

    #[tokio::test]
    async fn test_query_owner_nonexistent_file() {
        let lookup = PackageLookup::new();
        if lookup.manager() == PackageManager::Unknown {
            return;
        }
        let result = lookup
            .query_owner(Path::new("/tmp/this_file_surely_does_not_exist_12345"))
            .await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }
}
