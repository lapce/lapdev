use std::{fs, io, process::Command, thread, time::Duration};

pub const LAPDEV_SSH_PUBLIC_KEY: &str = "LAPDEV_SSH_PUBLIC_KEY";
pub const LAPDEV_IDE_CMDS: &str = "LAPDEV_IDE_CMDS";
pub const LAPDEV_CMDS: &str = "LAPDEV_CMDS";

pub fn run() {
    thread::spawn(move || {
        if let Err(e) = run_sshd() {
            eprintln!("run sshd error: {e:?}");
        }
    });
    thread::spawn(move || {
        if let Err(e) = run_ide_cmds() {
            eprintln!("run ide cmds error: {e:?}");
        }
    });
    thread::spawn(move || {
        if let Err(e) = run_cmds() {
            eprintln!("run cmds error: {e:?}");
        }
    });

    loop {
        thread::sleep(Duration::from_secs(60));
    }
}

#[derive(Debug)]
enum LapdevGuestAgentError {
    SshPublicKey(String),
    Cmds(String),
    IoError(io::Error),
}

impl From<io::Error> for LapdevGuestAgentError {
    fn from(err: io::Error) -> Self {
        Self::IoError(err)
    }
}

impl std::fmt::Display for LapdevGuestAgentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LapdevGuestAgentError::SshPublicKey(err) => f.write_str(err),
            LapdevGuestAgentError::Cmds(err) => f.write_str(err),
            LapdevGuestAgentError::IoError(e) => f.write_str(&e.to_string()),
        }
    }
}

fn run_sshd() -> Result<(), LapdevGuestAgentError> {
    let public_key = std::env::var(LAPDEV_SSH_PUBLIC_KEY)
        .map_err(|e| LapdevGuestAgentError::SshPublicKey(e.to_string()))?;
    fs::create_dir_all("/root/.ssh/")?;
    fs::write("/root/.ssh/authorized_keys", public_key)?;
    Command::new("/usr/sbin/sshd")
        .arg("-D")
        .arg("-o")
        .arg("AcceptEnv=*")
        .status()?;
    Ok(())
}

fn run_cmds() -> Result<(), LapdevGuestAgentError> {
    let cmds =
        std::env::var(LAPDEV_CMDS).map_err(|e| LapdevGuestAgentError::Cmds(e.to_string()))?;
    let cmds: Vec<Vec<String>> =
        serde_json::from_str(&cmds).map_err(|e| LapdevGuestAgentError::Cmds(e.to_string()))?;
    for cmd in cmds {
        if let Err(e) = run_cmd(cmd) {
            eprintln!("run cmds error: {e:?}");
        }
    }
    Ok(())
}

fn run_cmd(cmd: Vec<String>) -> Result<(), LapdevGuestAgentError> {
    if cmd.is_empty() {
        return Err(LapdevGuestAgentError::Cmds("empty cmd".to_string()));
    }
    thread::spawn(move || {
        if let Err(e) = Command::new("sh").arg("-c").arg(cmd.join(" ")).status() {
            eprintln!("run cmd error: {e:?}");
        }
    });
    Ok(())
}

fn run_ide_cmds() -> Result<(), LapdevGuestAgentError> {
    let cmds = std::env::var(LAPDEV_IDE_CMDS)
        .map_err(|e| LapdevGuestAgentError::Cmds(format!("can't get env error: {e}")))?;
    let cmds: Vec<String> = serde_json::from_str(&cmds)
        .map_err(|e| LapdevGuestAgentError::Cmds(format!("can't parse json error: {e}")))?;
    if cmds.is_empty() {
        return Err(LapdevGuestAgentError::Cmds("empty cmd".to_string()));
    }
    Command::new(&cmds[0]).args(&cmds[1..]).status()?;
    Ok(())
}
