use std::{path::PathBuf, sync::mpsc, time::Duration};

use anyhow::{Result, bail};
use clap::{Parser, Subcommand};
use rintawa_dev::{DevProject, DevSession, ReloadOutcome, ReloadingDevSession, SourceRevision};
use rintawa_sdk::types::{ExtensionInstanceId, RuntimeScopeId};

#[derive(Debug, Parser)]
#[command(name = "rintawa-dev", about = "Rintawa extension development tools")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Build and validate a local RTW project.
    Check {
        /// Project directory.
        #[arg(default_value = ".")]
        path: PathBuf,
    },
    /// Run a local extension through the RTW runtime path.
    Run {
        /// Project directory.
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Runtime instance ID.
        #[arg(long = "instance", default_value = "dev")]
        instance_id: String,
        /// Runtime scope ID.
        #[arg(long = "scope", default_value = "dev")]
        scope_id: String,
        /// Stop immediately after successful startup.
        #[arg(long = "once")]
        should_run_once: bool,
    },
    /// Rebuild and reload after stable source changes.
    Watch {
        /// Project directory.
        #[arg(default_value = ".")]
        path: PathBuf,
        /// Runtime instance ID.
        #[arg(long = "instance", default_value = "dev")]
        instance_id: String,
        /// Runtime scope ID.
        #[arg(long = "scope", default_value = "dev")]
        scope_id: String,
        /// Polling interval in milliseconds.
        #[arg(long = "poll-ms", default_value_t = 350)]
        poll_interval_ms: u64,
    },
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Commands::Check { path } => check(path),
        Commands::Run {
            path,
            instance_id,
            scope_id,
            should_run_once,
        } => run(path, instance_id, scope_id, should_run_once),
        Commands::Watch {
            path,
            instance_id,
            scope_id,
            poll_interval_ms,
        } => watch(path, instance_id, scope_id, poll_interval_ms),
    }
}

fn check(path: PathBuf) -> Result<()> {
    let project = DevProject::open(path)?;
    let snapshot = project.prepare_snapshot()?;
    let archive = snapshot.store().open_artifact(snapshot.digest())?;
    println!("digest = {}", snapshot.digest());
    println!("content = {}", archive.manifest().content);
    Ok(())
}

fn run(path: PathBuf, instance_id: String, scope_id: String, should_run_once: bool) -> Result<()> {
    let interrupt = if should_run_once {
        None
    } else {
        Some(interrupt_receiver()?)
    };
    let project = DevProject::open(path)?;
    let session = DevSession::start(
        &project,
        ExtensionInstanceId::new(instance_id),
        RuntimeScopeId::new(scope_id),
    )?;
    println!("extension = {}", session.extension_id());
    println!("digest = {}", session.digest());
    println!("instance = {}", session.instance_id());

    if let Some(receiver) = interrupt {
        println!("running; press Ctrl+C to stop");
        receiver.recv()?;
    }
    session.shutdown()?;
    Ok(())
}

fn watch(
    path: PathBuf,
    instance_id: String,
    scope_id: String,
    poll_interval_ms: u64,
) -> Result<()> {
    if poll_interval_ms < 50 {
        bail!("--poll-ms must be at least 50");
    }

    let interrupt = interrupt_receiver()?;
    let mut runner = ReloadingDevSession::start(
        path,
        ExtensionInstanceId::new(instance_id),
        RuntimeScopeId::new(scope_id),
    )?;
    if let Some(session) = runner.session() {
        println!("extension = {}", session.extension_id());
        println!("digest = {}", session.digest());
    }
    println!("watching; press Ctrl+C to stop");

    let interval = Duration::from_millis(poll_interval_ms);
    let mut baseline = runner.source_revision()?;
    let mut pending_revision: Option<SourceRevision> = None;

    loop {
        match interrupt.recv_timeout(interval) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }

        let revision = match runner.source_revision() {
            Ok(revision) => revision,
            Err(error) => {
                eprintln!("watch scan failed: {error}");
                pending_revision = None;
                continue;
            }
        };
        if revision == baseline {
            pending_revision = None;
            continue;
        }
        if pending_revision != Some(revision) {
            pending_revision = Some(revision);
            continue;
        }

        match runner.reload() {
            Ok(ReloadOutcome::Unchanged) => println!("source changed; RTW bytes unchanged"),
            Ok(ReloadOutcome::Reloaded { previous, current }) => {
                println!("reloaded {previous} -> {current}");
            }
            Err(error) => {
                eprintln!("reload failed: {error}");
                if runner.session().is_none() {
                    return Err(error.into());
                }
            }
        }

        baseline = match runner.source_revision() {
            Ok(revision) => revision,
            Err(error) => {
                eprintln!("post-reload scan failed: {error}");
                revision
            }
        };
        pending_revision = None;
    }

    runner.shutdown()?;
    Ok(())
}

fn interrupt_receiver() -> Result<mpsc::Receiver<()>> {
    let (sender, receiver) = mpsc::channel();
    ctrlc::set_handler(move || {
        let _ = sender.send(());
    })?;
    Ok(receiver)
}
