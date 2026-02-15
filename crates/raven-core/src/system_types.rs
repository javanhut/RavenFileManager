use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Information about a package that owns a file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageInfo {
    pub name: String,
    pub version: String,
    pub manager: PackageManager,
    pub description: Option<String>,
}

/// Supported Linux package managers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PackageManager {
    Pacman,
    Dpkg,
    Rpm,
    Unknown,
}

impl std::fmt::Display for PackageManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pacman => write!(f, "pacman"),
            Self::Dpkg => write!(f, "dpkg"),
            Self::Rpm => write!(f, "rpm"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// A process that has a file descriptor open to a file.
#[derive(Debug, Clone)]
pub struct ProcessLock {
    pub pid: u32,
    pub process_name: String,
    pub fd_type: FdType,
    pub fd_path: PathBuf,
}

/// Type of file descriptor access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FdType {
    Read,
    Write,
    ReadWrite,
}

impl std::fmt::Display for FdType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Read => write!(f, "read"),
            Self::Write => write!(f, "write"),
            Self::ReadWrite => write!(f, "read/write"),
        }
    }
}

/// A single entry in a disk usage calculation.
#[derive(Debug, Clone)]
pub struct DiskUsageEntry {
    pub path: PathBuf,
    pub size: u64,
    pub is_dir: bool,
    pub depth: u32,
}

/// Information about the container/sandbox environment.
#[derive(Debug, Clone)]
pub struct ContainerInfo {
    pub is_container: bool,
    pub container_type: ContainerType,
    pub app_id: Option<String>,
}

/// Type of container environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerType {
    Flatpak,
    Docker,
    Podman,
    SystemdNspawn,
    None,
}

impl std::fmt::Display for ContainerType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Flatpak => write!(f, "Flatpak"),
            Self::Docker => write!(f, "Docker"),
            Self::Podman => write!(f, "Podman"),
            Self::SystemdNspawn => write!(f, "systemd-nspawn"),
            Self::None => write!(f, "none"),
        }
    }
}

/// Parsed systemd unit file.
#[derive(Debug, Clone)]
pub struct SystemdUnit {
    pub name: String,
    pub unit_type: String,
    pub description: Option<String>,
    pub active_state: Option<String>,
    pub properties: Vec<(String, String)>,
}
