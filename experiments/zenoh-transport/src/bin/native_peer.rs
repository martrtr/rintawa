//! Native process exercise: only inherited pipes, no Zenoh session or World identity.
use std::io::{self, BufRead, Write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let namespace = std::fs::read_link("/proc/self/ns/net")?;
    if std::path::Path::new("/home").exists() {
        return Err("host home unexpectedly visible to the managed process".into());
    }
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let stdout = io::stdout();
    let mut output = stdout.lock();

    writeln!(output, "NS {}", namespace.display())?;
    output.flush()?;
    if std::env::args().any(|arg| arg == "--hang-before-first-request") {
        std::thread::park();
    }

    for (name, expected_prefix) in [
        ("controls", "VALUE "),
        ("../../other", "DENIED"),
        ("**", "DENIED"),
        ("rintawa/probe/world/world-b/public/controls", "DENIED"),
    ] {
        writeln!(output, "GET {name}")?;
        output.flush()?;
        let mut result = String::new();
        if input.read_line(&mut result)? == 0 {
            return Err("managed host disconnected before replying".into());
        }
        if !result.starts_with(expected_prefix) {
            return Err(format!("unexpected result for {name}: {result:?}").into());
        }
    }

    if std::env::args().any(|arg| arg == "--hang-after-protocol") {
        std::thread::park();
    }
    Ok(())
}
