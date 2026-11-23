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
    let input = std::io::stdin();
    if input.is_terminal() {
        bail!("pipe the secret through stdin; interactive terminal input is disabled");
    }
    let value = read_secret(input.lock())?;
    home.store_secret(&path, &value)?;
    println!("Secret stored in system credentials.");
    Ok(())
}
pub(crate) fn delete(home: &HostHome, path: &str) -> Result<()> {
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
