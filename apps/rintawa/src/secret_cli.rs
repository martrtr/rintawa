//! Bounded native stdin-to-vault commands, with no secret value in diagnostics.
use anyhow::{Result, anyhow, bail};
use rintawa_host::HostHome;
use rintawa_sdk::secrets::{SecretPath, SecretValue};
use std::io::{IsTerminal, Read};
const LIMIT: u64 = 64 * 1024;
fn read_secret(input: impl Read) -> Result<SecretValue> {
    let mut bytes = Vec::new();
    input
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow!("could not read secret stdin"))?;
    if bytes.is_empty() || bytes.len() as u64 > LIMIT {
        bail!("secret stdin must contain 1..65536 UTF-8 bytes");
    }
    let value =
        String::from_utf8(bytes).map_err(|_| anyhow!("secret stdin must be valid UTF-8"))?;
    Ok(SecretValue::new(value))
}
pub(crate) fn store(home: &HostHome, path: &str) -> Result<()> {
    let path = SecretPath::parse(path)?;
    require_legacy_backend(home)?;
    let value = stdin_secret()?;
    home.store_secret(&path, &value)?;
    println!("Secret stored in legacy account-global system credentials.");
    Ok(())
}
fn stdin_secret() -> Result<SecretValue> {
    let input = std::io::stdin();
    if input.is_terminal() {
        bail!("pipe the secret through stdin; interactive terminal input is disabled");
    }
    read_secret(input.lock())
}
pub(crate) fn delete(home: &HostHome, path: &str) -> Result<()> {
    require_legacy_backend(home)?;
    let existed = home.delete_secret(&SecretPath::parse(path)?)?;
    println!(
        "{}",
        if existed {
            "Secret deleted."
        } else {
            "Secret was not configured."
        }
    );
    Ok(())
}
fn require_legacy_backend(home: &HostHome) -> Result<()> {
    if home.secret_backend()? != rintawa_host::SecretBackend::System {
        bail!("direct paths use the system backend; use shared secret commands with local-file");
    }
    Ok(())
}
pub(crate) fn backend(home: &HostHome, backend: Option<rintawa_host::SecretBackend>) -> Result<()> {
    if let Some(backend) = backend {
        home.select_secret_backend(backend)?;
        println!("Backend selected; restart a running Host. No credentials copied or deleted.");
    }
    let selected = home.secret_backend()?;
    println!("backend = {}", selected.as_str());
    match selected {
        rintawa_host::SecretBackend::LocalFile => println!(
            "Not encrypted at rest; protected by OS permissions. Full-home backups contain secrets."
        ),
        rintawa_host::SecretBackend::System => {
            println!("Managed entries are home-local; legacy direct paths are account-global.")
        }
    }
    Ok(())
}
#[cfg(target_os = "linux")]
pub(crate) fn create(home: &HostHome, label: &str) -> Result<()> {
    let value = stdin_secret()?;
    let entry = home.create_shared_secret(label, &value)?;
    println!("{} revision={} {:?}", entry.id, entry.revision, entry.label);
    Ok(())
}
#[cfg(target_os = "linux")]
pub(crate) fn replace(home: &HostHome, id: &str, revision: u64) -> Result<()> {
    let value = stdin_secret()?;
    let entry = home.replace_shared_secret(id.parse()?, revision, &value)?;
    println!(
        "{} revision={}; all authorized readers use the replacement on their next read.",
        entry.id, entry.revision
    );
    Ok(())
}
#[cfg(target_os = "linux")]
pub(crate) fn list(home: &HostHome) -> Result<()> {
    backend(home, None)?;
    let entries = home.list_shared_secrets()?;
    for entry in &entries {
        println!("{} revision={} {:?}", entry.id, entry.revision, entry.label);
    }
    for grant in home.list_secret_consumers()? {
        let binding = match grant.entry_id {
            Some(id) => format!(
                "{}{}",
                id,
                if entries.iter().any(|entry| entry.id == id) {
                    ""
                } else {
                    " (missing in selected backend)"
                }
            ),
            None => "legacy direct path (account-global, system only)".to_owned(),
        };
        println!(
            "consumer {:?}/{:?}/{:?} slot={:?} -> {}",
            grant.scope_id.as_str(),
            grant.instance_id.as_str(),
            grant.component_id.as_str(),
            grant.path.as_str(),
            binding
        );
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_should_preserve_exact_stdin_and_hide_values_in_errors() {
        let text = "local test value\n";
        let secret = read_secret(text.as_bytes()).unwrap();
        assert_eq!(secret.expose_secret(), text);
        assert!(read_secret(&vec![b'x'; LIMIT as usize][..]).is_ok());
        for bytes in [vec![], vec![255], vec![b'x'; LIMIT as usize + 1]] {
            let error = read_secret(bytes.as_slice()).unwrap_err();
            let message = format!("{error:?}");
            assert!(!message.contains("local test value"));
            assert!(!message.contains("xxxxxxxx"));
        }
    }
}
