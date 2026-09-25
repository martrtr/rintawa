//! Command-line entry point for running and managing a local Rintawa host.

use std::{
    env,
    path::PathBuf,
    sync::mpsc::{self, RecvTimeoutError},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use rintawa_host::{HOST_SCOPE, HostHome, HostRuntime, UserContentId};
use rintawa_sdk::{
    contracts::ComponentRef, runtime_permissions::RuntimePermission, types::RuntimeScopeId,
    world::WorldId,
};

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
    /// Approve one manifest-requested runtime capability for an exact component.
    GrantRuntime {
        /// Concrete baseline runtime instance.
        instance: String,
        /// Component within the runtime instance.
        component: String,
        /// Exact runtime capability to approve.
        permission: RuntimePermission,
        /// Runtime scope containing the component.
        #[arg(long, default_value = HOST_SCOPE)]
        scope: String,
    },
    /// Remove one persisted runtime capability approval.
    RevokeRuntime {
        /// Concrete baseline runtime instance.
        instance: String,
        /// Component within the runtime instance.
        component: String,
        /// Exact runtime capability to revoke.
        permission: RuntimePermission,
        /// Runtime scope containing the component.
        #[arg(long, default_value = HOST_SCOPE)]
        scope: String,
    },
    /// Import a validated RTW into the generic persistent user-content library.
    ContentImport {
        /// Local RTW content file.
        path: PathBuf,
    },
    /// List persistent generic user-content entries.
    ContentList,
    /// Replace one logical user-content item with a validated RTW revision.
    ContentReplace {
        /// Stable logical user-content identity.
        id: UserContentId,
        /// Replacement local RTW content file.
        path: PathBuf,
    },
    /// Remove one logical user-content item from the library index.
    ContentRemove {
        /// Stable logical user-content identity.
        id: UserContentId,
    },
    /// Create an empty persistent authoritative world.
    WorldCreate,
    /// List persistent worlds in the local Rintawa home.
    WorldList,
    /// Show durable metadata for one persistent world.
    WorldInfo {
        /// Stable world identifier.
        id: WorldId,
    },
    /// Start the pre-world baseline composition from persisted exact digests.
    Run {
        /// Start the composition once, then shut down.
        #[arg(long)]
        once: bool,
    },
}

fn main() -> Result<()> {
    init_tracing();
    let cli = Cli::parse();
    let home = HostHome::open(resolve_home(cli.home)?)?;
    match cli.command {
        Commands::Install { path, disabled } => install(&home, path, disabled),
        Commands::List => list(&home),
        Commands::Enable { id } => set_enabled(&home, id, true),
        Commands::Disable { id } => set_enabled(&home, id, false),
        Commands::GrantRuntime {
            instance,
            component,
            permission,
            scope,
        } => grant_runtime_permission(&home, instance, component, permission, scope),
        Commands::RevokeRuntime {
            instance,
            component,
            permission,
            scope,
        } => revoke_runtime_permission(&home, instance, component, permission, scope),
        Commands::ContentImport { path } => content_import(&home, path),
        Commands::ContentList => content_list(&home),
        Commands::ContentReplace { id, path } => content_replace(&home, id, path),
        Commands::ContentRemove { id } => content_remove(&home, id),
        Commands::WorldCreate => world_create(&home),
        Commands::WorldList => world_list(&home),
        Commands::WorldInfo { id } => world_info(&home, id),
        Commands::Run { once } => run(&home, once),
    }
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
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

fn grant_runtime_permission(
    home: &HostHome,
    instance: String,
    component: String,
    permission: RuntimePermission,
    scope: String,
) -> Result<()> {
    let scope_id = RuntimeScopeId::new(scope);
    let owner = ComponentRef::new(instance, component);
    home.grant_runtime_permission(scope_id, owner.clone(), permission)?;
    println!(
        "{} {} {}: granted",
        owner.instance_id, owner.component_id, permission
    );
    Ok(())
}

fn revoke_runtime_permission(
    home: &HostHome,
    instance: String,
    component: String,
    permission: RuntimePermission,
    scope: String,
) -> Result<()> {
    let scope_id = RuntimeScopeId::new(scope);
    let owner = ComponentRef::new(instance, component);
    home.revoke_runtime_permission(&scope_id, &owner, permission)?;
    println!(
        "{} {} {}: revoked",
        owner.instance_id, owner.component_id, permission
    );
    Ok(())
}

fn content_import(home: &HostHome, path: PathBuf) -> Result<()> {
    let mut runtime = HostRuntime::start(home)?;
    let operation = (|| {
        let entry = runtime.import_user_content_rtw(home, path)?;
        print_user_content_entry(&entry);
        Ok(())
    })();
    finish_runtime_operation(runtime, operation)
}

fn content_list(home: &HostHome) -> Result<()> {
    let entries = home.list_user_content()?;
    if entries.is_empty() {
        println!("No user content imported.");
        return Ok(());
    }
    for entry in entries {
        println!("{} {} {}", entry.id, entry.content, entry.revision);
    }
    Ok(())
}

fn content_replace(home: &HostHome, id: UserContentId, path: PathBuf) -> Result<()> {
    let mut runtime = HostRuntime::start(home)?;
    let operation = (|| {
        let entry = runtime.replace_user_content_rtw(home, id, path)?;
        print_user_content_entry(&entry);
        Ok(())
    })();
    finish_runtime_operation(runtime, operation)
}

fn content_remove(home: &HostHome, id: UserContentId) -> Result<()> {
    let entry = home.remove_user_content(id)?;
    print_user_content_entry(&entry);
    Ok(())
}

fn print_user_content_entry(entry: &rintawa_host::UserContentEntry) {
    println!("id = {}", entry.id);
    println!("content = {}", entry.content);
    println!("revision = {}", entry.revision);
}

fn finish_runtime_operation(runtime: HostRuntime, operation: Result<()>) -> Result<()> {
    let shutdown = runtime.shutdown().map_err(anyhow::Error::from);
    match (operation, shutdown) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(operation_error), Err(shutdown_error)) => {
            Err(operation_error.context(format!("runtime shutdown also failed: {shutdown_error}")))
        }
    }
}

fn world_create(home: &HostHome) -> Result<()> {
    let world = home.create_world()?;
    println!("world = {}", world.id);
    println!("commit_position = {}", world.commit_position);
    Ok(())
}

fn world_list(home: &HostHome) -> Result<()> {
    let worlds = home.list_worlds()?;
    if worlds.is_empty() {
        println!("No worlds created.");
        return Ok(());
    }
    for world in worlds {
        println!("{} {}", world.id, world.commit_position);
    }
    Ok(())
}

fn world_info(home: &HostHome, world_id: WorldId) -> Result<()> {
    let world = home.load_world_state(world_id)?;
    println!("world = {}", world.id());
    println!("commit_position = {}", world.commit_position());
    println!("schemas = {}", world.schemas().len());
    Ok(())
}

fn run(home: &HostHome, once: bool) -> Result<()> {
    let mut runtime = HostRuntime::start(home)?;
    let operation_result = if once {
        Ok(())
    } else {
        run_until_interrupt(&mut runtime)
    };
    let shutdown_result = runtime.shutdown().map_err(anyhow::Error::from);

    match (operation_result, shutdown_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(operation_error), Err(shutdown_error)) => {
            Err(operation_error.context(format!("runtime shutdown also failed: {shutdown_error}")))
        }
    }
}

fn run_until_interrupt(runtime: &mut HostRuntime) -> Result<()> {
    println!("running; press Ctrl+C to stop");
    let receiver = interrupt_receiver()?;
    let maximum_idle_wait = Duration::from_millis(250);
    loop {
        let wait = runtime
            .poll_runtime()?
            .map_or(maximum_idle_wait, |delay| delay.min(maximum_idle_wait));
        match receiver.recv_timeout(wait) {
            Ok(()) | Err(RecvTimeoutError::Disconnected) => return Ok(()),
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

fn resolve_home(override_home: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = override_home {
        return Ok(path);
    }
    if let Some(path) = env::var_os("RINTAWA_HOME") {
        return Ok(PathBuf::from(path));
    }
    #[cfg(windows)]
    if let Some(path) = env::var_os("LOCALAPPDATA") {
        return Ok(PathBuf::from(path).join("Rintawa"));
    }
    #[cfg(not(windows))]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_parse_runtime_permission_commands() {
        let grant = Cli::try_parse_from([
            "rintawa",
            "grant-runtime",
            "example.runtime",
            "runtime",
            "background-task",
        ])
        .expect("grant-runtime command should parse");
        assert!(matches!(
            grant.command,
            Commands::GrantRuntime {
                instance,
                component,
                permission: RuntimePermission::BackgroundTask,
                scope,
            } if instance == "example.runtime" && component == "runtime" && scope == HOST_SCOPE
        ));

        let content_import = Cli::try_parse_from(["rintawa", "content-import", "/tmp/content.rtw"])
            .expect("content-import command should parse");
        assert!(matches!(
            content_import.command,
            Commands::ContentImport { path } if path == std::path::Path::new("/tmp/content.rtw")
        ));

        let revoke = Cli::try_parse_from([
            "rintawa",
            "revoke-runtime",
            "example.runtime",
            "runtime",
            "loopback-listen",
            "--scope",
            "world:test",
        ])
        .expect("revoke-runtime command should parse");
        assert!(matches!(
            revoke.command,
            Commands::RevokeRuntime {
                permission: RuntimePermission::LoopbackListen,
                scope,
                ..
            } if scope == "world:test"
        ));
    }

    #[test]
    fn test_should_reject_unknown_runtime_permission() {
        assert!(
            Cli::try_parse_from([
                "rintawa",
                "grant-runtime",
                "example.runtime",
                "runtime",
                "raw-network",
            ])
            .is_err()
        );
    }
}
