use anyhow::{anyhow, Context, Result};
use std::{fs::OpenOptions, path::PathBuf};
use tempfile::NamedTempFile;
use tokio::io::AsyncWriteExt;

#[cfg(windows)]
use std::path::Path;

use super::ServiceEndpoint;

const LINE_MARKER: &str = "# lapdev-devbox";

/// Manages /etc/hosts file entries for devbox DNS
pub struct HostsManager {
    hosts_path: PathBuf,
}

impl Default for HostsManager {
    fn default() -> Self {
        Self::new()
    }
}

impl HostsManager {
    pub fn new() -> Self {
        Self {
            hosts_path: Self::get_hosts_path(),
        }
    }

    /// Get the platform-specific hosts file path
    fn get_hosts_path() -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(r"C:\Windows\System32\drivers\etc\hosts")
        } else {
            PathBuf::from("/etc/hosts")
        }
    }

    /// Write service endpoints to hosts file with per-line markers
    pub async fn write_entries(&self, endpoints: &[ServiceEndpoint]) -> Result<()> {
        // Read existing hosts file
        let content = tokio::fs::read_to_string(&self.hosts_path)
            .await
            .with_context(|| format!("Failed to read hosts file: {:?}", self.hosts_path))?;

        // Remove existing lapdev entries
        let mut cleaned = self.remove_existing_entries(&content);

        if !cleaned.ends_with('\n') && !cleaned.is_empty() {
            cleaned.push('\n');
        }

        for endpoint in endpoints {
            let aliases = endpoint.aliases().join(" ");
            cleaned.push_str(&format!(
                "{} {} {}\n",
                endpoint.synthetic_ip, aliases, LINE_MARKER
            ));
        }

        // Write back (requires elevated permissions)
        self.write_hosts_file(&cleaned).await?;

        Ok(())
    }

    /// Remove all lapdev entries from hosts file
    pub async fn remove_entries(&self) -> Result<()> {
        let content = tokio::fs::read_to_string(&self.hosts_path)
            .await
            .with_context(|| format!("Failed to read hosts file: {:?}", self.hosts_path))?;

        let cleaned = self.remove_existing_entries(&content);

        self.write_hosts_file(&cleaned).await?;

        Ok(())
    }

    fn remove_existing_entries(&self, content: &str) -> String {
        let mut result = String::new();

        for line in content.lines() {
            if line.trim_end().ends_with(LINE_MARKER) {
                continue;
            }
            result.push_str(line);
            result.push('\n');
        }

        result
    }

    /// Write content to hosts file with platform-specific line endings
    async fn write_hosts_file(&self, content: &str) -> Result<()> {
        #[cfg(windows)]
        let content = {
            // Preserve logical lines while enforcing CRLF endings
            let mut lines: Vec<String> = content
                .lines()
                .map(|line| line.trim_end_matches('\r').to_string())
                .collect();
            if let Some(true) = lines.last().map(|line| line.is_empty()) {
                lines.pop();
            }
            let mut normalized = lines.join("\r\n");
            normalized.push_str("\r\n");
            normalized
        };

        #[cfg(not(windows))]
        let content = content.to_string();

        let parent = self
            .hosts_path
            .parent()
            .context("Hosts path has no parent directory")?;

        let temp_file = NamedTempFile::new_in(parent)
            .with_context(|| format!("Failed to create temporary hosts file in {:?}", parent))?;
        let temp_path = temp_file.path().to_path_buf();

        let std_file = temp_file.as_file().try_clone().with_context(|| {
            format!(
                "Failed to clone temporary hosts file handle: {:?}",
                temp_path
            )
        })?;
        let mut async_file = tokio::fs::File::from_std(std_file);

        async_file
            .write_all(content.as_bytes())
            .await
            .with_context(|| format!("Failed to write temporary hosts file: {:?}", temp_path))?;
        async_file
            .sync_all()
            .await
            .with_context(|| format!("Failed to sync temporary hosts file: {:?}", temp_path))?;

        drop(async_file);

        #[cfg(unix)]
        if let Ok(metadata) = tokio::fs::metadata(&self.hosts_path).await {
            use std::os::unix::fs::PermissionsExt;
            let permissions = std::fs::Permissions::from_mode(metadata.permissions().mode());
            let _ = tokio::fs::set_permissions(&temp_path, permissions).await;
        }

        #[cfg(windows)]
        if let Ok(metadata) = tokio::fs::metadata(&self.hosts_path).await {
            let permissions = metadata.permissions();
            let _ = tokio::fs::set_permissions(&temp_path, permissions).await;
        }

        #[cfg(windows)]
        {
            use tempfile::TempPath;

            let temp_path_handle: TempPath = temp_file.into_temp_path();
            replace_file_windows(temp_path.as_path(), &self.hosts_path)?;
            let _ = temp_path_handle.close();
        }

        #[cfg(not(windows))]
        {
            temp_file.persist(&self.hosts_path).map_err(|err| {
                anyhow!(
                    "Failed to replace hosts file at {:?}: {}",
                    self.hosts_path,
                    err.error
                )
            })?;
        }

        #[cfg(unix)]
        if let Ok(dir) = tokio::fs::File::open(parent).await {
            let _ = dir.sync_all().await;
        }

        Ok(())
    }

    /// Check if we have permission to write to hosts file
    pub fn check_permissions(&self) -> bool {
        OpenOptions::new()
            .write(true)
            .append(true)
            .open(&self.hosts_path)
            .is_ok()
    }

    /// Print manual instructions if we can't modify hosts file
    pub fn print_manual_instructions(&self, endpoints: &[ServiceEndpoint]) {
        eprintln!("\n⚠️  Unable to automatically update hosts file.");
        eprintln!(
            "Please manually add the following entries to {:?}:",
            self.hosts_path
        );
        for endpoint in endpoints {
            let aliases = endpoint.aliases().join(" ");
            eprintln!("{} {} {}", endpoint.synthetic_ip, aliases, LINE_MARKER);
        }
        eprintln!();

        if cfg!(unix) {
            eprintln!("Run this command with sudo, or edit the file manually:");
            eprintln!("  sudo nano {:?}", self.hosts_path);
        } else if cfg!(windows) {
            eprintln!("Run this command as Administrator, or edit the file manually:");
            eprintln!("  notepad {:?}", self.hosts_path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remove_existing_entries_strips_tagged_lines() {
        let manager = HostsManager::new();
        let content = r#"127.0.0.1 localhost
127.77.0.1 svc1.default.svc.cluster.local # lapdev-devbox
127.0.0.2 other
"#;

        let cleaned = manager.remove_existing_entries(content);
        assert!(cleaned.contains("127.0.0.1 localhost"));
        assert!(cleaned.contains("127.0.0.2 other"));
        assert!(!cleaned.contains("lapdev-devbox"));
        assert!(cleaned.ends_with('\n'));
    }
}

#[cfg(windows)]
fn replace_file_windows(temp_path: &Path, target_path: &Path) -> Result<()> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{ReplaceFileW, REPLACEFILE_WRITE_THROUGH};

    let target_w: Vec<u16> = target_path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect();
    let temp_w: Vec<u16> = temp_path.as_os_str().encode_wide().chain(Some(0)).collect();

    let success = unsafe {
        ReplaceFileW(
            target_w.as_ptr(),
            temp_w.as_ptr(),
            std::ptr::null(),
            REPLACEFILE_WRITE_THROUGH,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };

    if success == 0 {
        let err = std::io::Error::last_os_error();
        Err(anyhow!(
            "Failed to replace hosts file at {:?}: {}",
            target_path,
            err
        ))
    } else {
        Ok(())
    }
}
