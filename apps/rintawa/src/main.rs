use std::{env, path::PathBuf, sync::mpsc, time::Duration};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use rintawa_host::{HostHome, HostRuntime};

#[derive(Debug, Parser)]
#[command(name = "rintawa", about = "Rintawa host and local RTW bootstrap")]
struct Cli {
    /// Override the local Rintawa home directory.
    #[arg(long, global = true)]
    home: Option<PathBuf>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Import a local RTW extension and select its exact digest in the baseline profile.
    Install {
        /// Local RTW file.
        path: PathBuf,
        /// Install the activation disabled instead of starting it by default.
        #[arg(long)]
        disabled: bool,
    },
    /// Show baseline extension activations.
    List,
    /// Enable one baseline activation by extension ID.
    Enable { id: String },
    /// Disable one baseline activation by extension ID.
    Disable { id: String },
    /// Start the pre-world baseline composition from persisted exact digests.
    Run {
        /// Start, print endpoints, pump once, then shut down.
        #[arg(long)]
        once: bool,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let home = HostHome::open(resolve_home(cli.home)?)?;
    match cli.command {
        Commands::Install { path, disabled } => install(&home, path, disabled),
        Commands::List => list(&home),
        Commands::Enable { id } => set_enabled(&home, id, true),
        Commands::Disable { id } => set_enabled(&home, id, false),
        Commands::Run { once } => run(&home, once),
    }
}

fn install(home: &HostHome, path: PathBuf, disabled: bool) -> Result<()> {
    let result = home.install_local_rtw(path, disabled.then_some(false))?;
    let activation = result.activation;
    println!("subject = {}", activation.subject);
    println!("content = {}", activation.content);
    println!("name = {}", activation.name);
    if let Some(version) = activation.version.as_deref() {
        println!("version = {version}");
    }
    println!("digest = {}", activation.digest);
    println!("instance = {}", activation.instance_id);
    println!("scope = {}", activation.scope_id);
    println!("enabled = {}", activation.enabled);
    println!("store = {:?}", result.disposition);
    Ok(())
}

fn list(home: &HostHome) -> Result<()> {
    let activations = home.list_activations()?;
    if activations.is_empty() {
        println!("No baseline activations installed.");
        return Ok(());
    }
    for activation in activations {
        println!(
            "{} {} [{}] {} {} {}",
            activation.subject,
            activation.version.as_deref().unwrap_or("-"),
            if activation.enabled {
                "enabled"
            } else {
                "disabled"
            },
            activation.scope_id,
            activation.content,
            activation.digest
        );
    }
    Ok(())
}

fn set_enabled(home: &HostHome, id: String, enabled: bool) -> Result<()> {
    home.set_enabled(&id, enabled)?;
    println!("{id}: {}", if enabled { "enabled" } else { "disabled" });
    Ok(())
}

fn run(home: &HostHome, once: bool) -> Result<()> {
    let mut runtime = HostRuntime::start(home)?;
    for url in runtime.web_urls()? {
        println!("web = {url}");
    }
    runtime.pump()?;
    if !once {
        println!("running; press Ctrl+C to stop");
        let receiver = interrupt_receiver()?;
        loop {
            match receiver.recv_timeout(Duration::from_millis(50)) {
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => runtime.pump()?,
            }
        }
    }
    runtime.shutdown()?;
    Ok(())
}

fn resolve_home(override_home: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = override_home {
        return Ok(path);
    }
    if let Some(path) = env::var_os("RINTAWA_HOME") {
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = env::var_os("XDG_DATA_HOME") {
        return Ok(PathBuf::from(path).join("rintawa"));
    }
    if let Some(path) = env::var_os("HOME") {
        return Ok(PathBuf::from(path).join(".local/share/rintawa"));
    }
    bail!("cannot resolve Rintawa home; pass --home or set RINTAWA_HOME")
}

fn interrupt_receiver() -> Result<mpsc::Receiver<()>> {
    let (sender, receiver) = mpsc::channel();
    ctrlc::set_handler(move || {
        let _ = sender.send(());
    })
    .context("failed to install Ctrl+C handler")?;
    Ok(receiver)
}
