use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::tools::config;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FsSandboxConfig {
    pub enabled: bool,
    pub allow_read: Vec<String>,
    pub deny_read: Vec<String>,
    pub allow_write: Vec<String>,
    pub deny_write: Vec<String>,
}

impl FsSandboxConfig {
    pub fn check_read(&self, path: &Path) -> FsAccess {
        if !self.enabled {
            return FsAccess::Allowed;
        }
        let path_str = path.to_string_lossy();
        if !self.allow_read.is_empty() && !self.allow_read.iter().any(|p| path_str.starts_with(p)) {
            return FsAccess::Denied("Path not in read allowlist".into());
        }
        if self.deny_read.iter().any(|p| path_str.starts_with(p)) {
            return FsAccess::Denied("Path in read denylist".into());
        }
        FsAccess::Allowed
    }

    pub fn check_write(&self, path: &Path) -> FsAccess {
        if !self.enabled {
            return FsAccess::Allowed;
        }
        let path_str = path.to_string_lossy();
        if !self.allow_write.is_empty() && !self.allow_write.iter().any(|p| path_str.starts_with(p))
        {
            return FsAccess::Denied("Path not in write allowlist".into());
        }
        if self.deny_write.iter().any(|p| path_str.starts_with(p)) {
            return FsAccess::Denied("Path in write denylist".into());
        }
        FsAccess::Allowed
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum FsAccess {
    Allowed,
    Denied(String),
}

pub fn check_read_access(path: &Path) -> Result<(), String> {
    let cfg = &config().sandbox.filesystem;
    cfg.check_read(path).into_result()
}

pub fn check_write_access(path: &Path) -> Result<(), String> {
    let cfg = &config().sandbox.filesystem;
    cfg.check_write(path).into_result()
}

impl FsAccess {
    pub fn is_allowed(&self) -> bool {
        matches!(self, FsAccess::Allowed)
    }

    pub fn into_result(self) -> Result<(), String> {
        match self {
            FsAccess::Allowed => Ok(()),
            FsAccess::Denied(msg) => Err(msg),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_disabled_allows_all() {
        let cfg = FsSandboxConfig::default();
        assert_eq!(cfg.check_read(Path::new("/etc/passwd")), FsAccess::Allowed);
        assert_eq!(cfg.check_write(Path::new("/etc/passwd")), FsAccess::Allowed);
    }

    #[test]
    fn test_deny_read() {
        let mut cfg = FsSandboxConfig {
            enabled: true,
            ..Default::default()
        };
        cfg.deny_read.push("/etc/".to_string());
        assert_eq!(
            cfg.check_read(Path::new("/etc/passwd")),
            FsAccess::Denied("Path in read denylist".into())
        );
        assert_eq!(
            cfg.check_read(Path::new("/home/user/file.txt")),
            FsAccess::Allowed
        );
    }

    #[test]
    fn test_allow_write_whitelist() {
        let mut cfg = FsSandboxConfig {
            enabled: true,
            ..Default::default()
        };
        cfg.allow_write.push("/home/".to_string());
        assert_eq!(
            cfg.check_write(Path::new("/home/user/file.txt")),
            FsAccess::Allowed
        );
        assert_eq!(
            cfg.check_write(Path::new("/etc/hosts")),
            FsAccess::Denied("Path not in write allowlist".into())
        );
    }
}
