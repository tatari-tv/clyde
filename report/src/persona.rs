use log::{debug, warn};
use serde::{Deserialize, Serialize};
use std::io::ErrorKind;
use std::process::{Child, Command, Stdio};
use std::thread::sleep;
use std::time::Duration;
use wait_timeout::ChildExt;

const PERSONA_BIN: &str = "persona";
const TIMEOUT: Duration = Duration::from_secs(5);
// execve of a file that is still open for writing fails with ETXTBSY
// (ExecutableFileBusy). This happens when the `persona` binary is being
// (re)installed, and in the test suite when a parallel thread holds a
// just-written fixture open across its own fork/exec. The busy window is
// sub-millisecond, so a few short retries clear it.
const SPAWN_RETRIES: u32 = 5;
const SPAWN_RETRY_DELAY: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub struct PersonaBlock {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub organization: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub department: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manager: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub github: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawPersona {
    preferred_full_name: Option<String>,
    business_title: Option<String>,
    team_org: Option<String>,
    organization_org: Option<String>,
    department_org: Option<String>,
    supervisor_name: Option<String>,
    supervisor_email: Option<String>,
    work_email: Option<String>,
    github_username: Option<String>,
    primary_home_address_state: Option<String>,
}

pub fn whoami() -> Option<PersonaBlock> {
    whoami_via(PERSONA_BIN)
}

/// Spawn `persona whoami --json`, retrying a bounded number of times on
/// ETXTBSY (see `SPAWN_RETRIES`). Every other spawn error is returned as-is.
fn spawn_persona(bin: &str) -> std::io::Result<Child> {
    let mut attempt = 0;
    loop {
        match Command::new(bin)
            .arg("whoami")
            .arg("--json")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Err(e) if e.kind() == ErrorKind::ExecutableFileBusy && attempt < SPAWN_RETRIES => {
                attempt += 1;
                debug!(
                    "persona::whoami: {} busy (ETXTBSY), retry {}/{}",
                    bin, attempt, SPAWN_RETRIES
                );
                sleep(SPAWN_RETRY_DELAY);
            }
            other => return other,
        }
    }
}

pub(crate) fn whoami_via(bin: &str) -> Option<PersonaBlock> {
    debug!("persona::whoami: spawning {} whoami --json", bin);
    let mut child = match spawn_persona(bin) {
        Ok(c) => c,
        Err(e) => {
            warn!("persona::whoami: spawn failed: {}", e);
            eprintln!("persona whoami failed; rendering anonymously");
            return None;
        }
    };

    let status = match child.wait_timeout(TIMEOUT) {
        Ok(Some(s)) => s,
        Ok(None) => {
            warn!("persona::whoami: timed out after {:?}, killing child", TIMEOUT);
            let _ = child.kill();
            let _ = child.wait();
            eprintln!("persona whoami failed; rendering anonymously");
            return None;
        }
        Err(e) => {
            warn!("persona::whoami: wait_timeout error: {}", e);
            let _ = child.kill();
            let _ = child.wait();
            eprintln!("persona whoami failed; rendering anonymously");
            return None;
        }
    };

    if !status.success() {
        warn!("persona::whoami: exited with status {}", status);
        eprintln!("persona whoami failed; rendering anonymously");
        return None;
    }

    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => {
            warn!("persona::whoami: child has no stdout pipe");
            eprintln!("persona whoami failed; rendering anonymously");
            return None;
        }
    };

    let raw: RawPersona = match serde_json::from_reader(stdout) {
        Ok(r) => r,
        Err(e) => {
            warn!("persona::whoami: JSON parse failed: {}", e);
            eprintln!("persona whoami failed; rendering anonymously");
            return None;
        }
    };

    Some(map_raw(raw))
}

fn map_raw(raw: RawPersona) -> PersonaBlock {
    let manager = match (raw.supervisor_name, raw.supervisor_email) {
        (Some(name), Some(email)) => Some(format!("{} ({})", name, email)),
        (Some(name), None) => Some(name),
        (None, Some(email)) => Some(email),
        (None, None) => None,
    };
    PersonaBlock {
        name: raw.preferred_full_name,
        title: raw.business_title,
        team: raw.team_org,
        organization: raw.organization_org,
        department: raw.department_org,
        manager,
        email: raw.work_email,
        github: raw.github_username,
        location: raw.primary_home_address_state,
    }
}

#[cfg(test)]
mod tests;
