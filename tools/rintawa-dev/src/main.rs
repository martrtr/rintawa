use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, Subcommand};
use rintawa_dev::DevProject;

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
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Commands::Check { path } => check(path),
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
