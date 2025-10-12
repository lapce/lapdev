use anyhow::{Context, Result};
use std::{
    fs::{self, OpenOptions},
    path::PathBuf,
};

use super::ServiceEndpoint;

const BEGIN_MARKER: &str = "# BEGIN lapdev-devbox";
const END_MARKER: &str = "# END lapdev-devbox";

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

    /// Write service endpoints to hosts file between markers
    pub fn write_entries(&self, endpoints: &[ServiceEndpoint]) -> Result<()> {
        // Read existing hosts file
        let content = fs::read_to_string(&self.hosts_path)
            .with_context(|| format!("Failed to read hosts file: {:?}", self.hosts_path))?;

        // Remove existing lapdev block if present
        let without_block = self.remove_block(&content);

        // Build new block
        let mut new_block = String::new();
        new_block.push_str(BEGIN_MARKER);
        new_block.push('\n');

        for endpoint in endpoints {
            let aliases = endpoint.aliases().join(" ");
            new_block.push_str(&format!("{} {}\n", endpoint.synthetic_ip, aliases));
        }

        new_block.push_str(END_MARKER);
        new_block.push('\n');

        // Combine
        let new_content = format!("{}{}", without_block, new_block);

        // Write back (requires elevated permissions)
        self.write_hosts_file(&new_content)?;

        Ok(())
    }

    /// Remove all lapdev entries from hosts file
    pub fn remove_entries(&self) -> Result<()> {
        let content = fs::read_to_string(&self.hosts_path)
            .with_context(|| format!("Failed to read hosts file: {:?}", self.hosts_path))?;

        let without_block = self.remove_block(&content);

        self.write_hosts_file(&without_block)?;

        Ok(())
    }

    /// Remove the lapdev block from content
    fn remove_block(&self, content: &str) -> String {
        let mut result = String::new();
        let mut in_block = false;

        for line in content.lines() {
            if line.trim() == BEGIN_MARKER {
                in_block = true;
                continue;
            }
            if line.trim() == END_MARKER {
                in_block = false;
                continue;
            }
            if !in_block {
                result.push_str(line);
                result.push('\n');
            }
        }

        result
    }

    /// Write content to hosts file with platform-specific line endings
    fn write_hosts_file(&self, content: &str) -> Result<()> {
        let content = if cfg!(windows) {
            // Windows requires CRLF
            content.replace('\n', "\r\n")
        } else {
            content.to_string()
        };

        fs::write(&self.hosts_path, content)
            .with_context(|| format!("Failed to write hosts file: {:?}", self.hosts_path))?;

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
        eprintln!("Please manually add the following entries to {:?}:\n", self.hosts_path);
        eprintln!("{}", BEGIN_MARKER);
        for endpoint in endpoints {
            let aliases = endpoint.aliases().join(" ");
            eprintln!("{} {}", endpoint.synthetic_ip, aliases);
        }
        eprintln!("{}", END_MARKER);
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
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn test_remove_block() {
        let manager = HostsManager::new();
        let content = r#"127.0.0.1 localhost
# BEGIN lapdev-devbox
127.77.0.1 service1
# END lapdev-devbox
127.0.1.1 hostname
"#;

        let result = manager.remove_block(content);
        assert!(!result.contains("BEGIN lapdev-devbox"));
        assert!(!result.contains("service1"));
        assert!(result.contains("localhost"));
        assert!(result.contains("hostname"));
    }
}
