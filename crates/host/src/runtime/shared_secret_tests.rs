//! End-to-end managed binding through real artifact loading and scoped lifecycle.

use super::*;
use rintawa_extension_engine::{RtwComponentHost, RtwComponentHostResult, RtwComponentSource};
use rintawa_sdk::{
    context::ComponentContext,
    errors::{ExtensionError, ExtensionResult},
    manifest::{ComponentDescriptor, ExtensionManifest},
    secrets::{SecretAccessError, SecretPath, SecretValue},
    traits::Component,
    types::ComponentId,
};

const TARGET: &str = "shared-secret.test.native@1";
type Observed = Arc<Mutex<BTreeMap<String, Result<String, SecretAccessError>>>>;

struct Reader {
    id: ComponentId,
    observed: Observed,
}

impl Component for Reader {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn id(&self) -> &ComponentId {
        &self.id
    }
    fn poll_runtime(
        &mut self,
        context: &mut dyn ComponentContext,
    ) -> ExtensionResult<Option<Duration>> {
        let result = match context.read_secret(&SecretPath::parse("ai.selected").unwrap()) {
            Ok(value) => Ok(value.expose_secret().to_owned()),
            Err(ExtensionError::SecretAccess(error)) => Err(error),
            Err(error) => panic!("unexpected test read failure: {error}"),
        };
        self.observed
            .lock()
            .unwrap()
            .insert(context.extension_id().to_string(), result);
        Ok(None)
    }
}

struct Target(Observed);
struct Provider(ComponentId);
impl Component for Provider {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn id(&self) -> &ComponentId {
        &self.0
    }
}
impl RtwComponentHost for Target {
    fn target(&self) -> &str {
        TARGET
    }
    fn load_component(
        &self,
        _: &mut RtwComponentSource<'_>,
        descriptor: &ComponentDescriptor,
    ) -> RtwComponentHostResult<Box<dyn Component>> {
        Ok(Box::new(Reader {
            id: descriptor.id.clone(),
            observed: self.0.clone(),
        }))
    }
}

fn attach_target(host: &mut HostRuntime, observed: &Observed) -> anyhow::Result<()> {
    let provider = ComponentRef::new("test-target", "provider");
    host.engine.register_extension_instance(
        provider.instance_id.clone(),
        RuntimeScopeId::new(HOST_SCOPE),
        ExtensionManifest {
            id: ExtensionId::new("test-target"),
            name: "test-target".into(),
            version: "0.0.1".into(),
            sdk: "^0.0".into(),
            components: Vec::new(),
        },
        vec![Box::new(Provider(provider.component_id.clone()))],
    )?;
    host.engine
        .start_extension_instance(&provider.instance_id)?;
    host.started_instances.push(provider.instance_id.clone());
    host.engine
        .register_execution_target_host(&provider, Arc::new(Target(observed.clone())))?;
    Ok(())
}

fn install(root: &Path, home: &HostHome, world: WorldId, id: &str) -> anyhow::Result<ComponentRef> {
    let source = root.join(id);
    std::fs::create_dir(&source)?;
    std::fs::write(
        source.join("rtw.toml"),
        "format = 1\ncontent = \"rintawa.extension@1\"\nentry = \"manifest.toml\"\n",
    )?;
    std::fs::write(
        source.join("manifest.toml"),
        format!(
            r#"id = "{id}"
name = "Shared secret fixture"
version = "0.0.1"
sdk = "^0.0"
[[components]]
id = "backend"
kind = "runtime"
target = "{TARGET}"
entry = "payload.bin"
[components.permissions]
secret-read = ["ai.*"]
"#
        ),
    )?;
    std::fs::write(source.join("payload.bin"), "fixture")?;
    let output = root.join(format!("{id}.rtw"));
    rintawa_artifacts::pack_directory(source, &output, rintawa_artifacts::RtwLimits::default())?;
    let package = home.install_local_rtw(&output, Some(false))?.activation;
    let activation = home.select_world_stored_rtw(world, &package.digest, Some(true))?;
    Ok(ComponentRef::new(activation.instance_id, "backend"))
}

#[test]
fn two_extensions_share_rotation_restart_rebind_and_disconnect_without_foreign_access()
-> anyhow::Result<()> {
    let root = tempfile::tempdir()?;
    let home = HostHome::open(root.path().join("home"))?;
    home.select_secret_backend(crate::SecretBackend::LocalFile)?;
    let world = home.create_world()?.id;
    let scope = world_runtime_scope_id(world);
    let a = install(root.path(), &home, world, "fixture.first")?;
    let b = install(root.path(), &home, world, "fixture.second")?;
    let _c = install(root.path(), &home, world, "fixture.unapproved")?;
    let secret = home.create_shared_secret("shared", &SecretValue::new("shared sentinel"))?;
    let other = home.create_shared_secret("other", &SecretValue::new("other sentinel"))?;
    let observed = Observed::default();
    let mut host = HostRuntime::start(&home)?;
    attach_target(&mut host, &observed)?;
    host.activate_world(&home, world)?;
    let slot = SecretPath::parse("ai.selected")?;
    let target_a = host.secret_consumer_target(&scope, &a).unwrap();
    let target_b = host.secret_consumer_target(&scope, &b).unwrap();
    host.poll_runtime()?;
    assert!(
        observed
            .lock()
            .unwrap()
            .values()
            .all(|v| *v == Err(SecretAccessError::AccessDenied))
    );
    for target in [&target_a, &target_b] {
        host.bind_shared_secret(target, slot.clone(), secret.id)
            .unwrap();
    }
    host.poll_runtime()?;
    for id in ["fixture.first", "fixture.second"] {
        assert_eq!(
            observed.lock().unwrap()[id],
            Ok("shared sentinel".to_owned())
        );
    }
    assert_eq!(
        observed.lock().unwrap()["fixture.unapproved"],
        Err(SecretAccessError::AccessDenied)
    );
    assert!(
        host.bind_shared_secret(&target_a, SecretPath::parse("foreign.slot")?, other.id)
            .is_err()
    );
    assert!(
        host.bind_shared_secret(&target_a, slot.clone(), Uuid::new_v4())
            .is_err()
    );
    assert_eq!(home.list_secret_consumers()?.len(), 2);
    let replacement = home.replace_shared_secret(
        secret.id,
        secret.revision,
        &SecretValue::new("rotated sentinel"),
    )?;
    host.poll_runtime()?;
    for id in ["fixture.first", "fixture.second"] {
        assert_eq!(
            observed.lock().unwrap()[id],
            Ok("rotated sentinel".to_owned())
        );
    }
    host.bind_shared_secret(&target_a, slot.clone(), other.id)
        .unwrap();
    assert_eq!(
        home.load_world_composition(world)?
            .secret_read_grants
            .iter()
            .find(|g| g.instance_id == a.instance_id)
            .unwrap()
            .entry_id,
        Some(other.id)
    );
    host.deactivate_world(world)?;
    assert!(
        host.bind_shared_secret(&target_a, slot.clone(), secret.id)
            .is_err()
    );
    host.shutdown()?;
    drop(host);
    let reopened = HostHome::open(home.root())?;
    let mut host = HostRuntime::start(&reopened)?;
    attach_target(&mut host, &observed)?;
    host.activate_world(&reopened, world)?;
    host.poll_runtime()?;
    assert_eq!(
        observed.lock().unwrap()["fixture.first"],
        Ok("other sentinel".to_owned())
    );
    assert_eq!(
        observed.lock().unwrap()["fixture.second"],
        Ok("rotated sentinel".to_owned())
    );
    let target_a = host.secret_consumer_target(&scope, &a).unwrap();
    host.disconnect_shared_secret(target_a, slot).unwrap();
    host.poll_runtime()?;
    assert_eq!(
        observed.lock().unwrap()["fixture.first"],
        Err(SecretAccessError::AccessDenied)
    );
    assert_eq!(home.list_shared_secrets()?.len(), 2);
    home.delete_shared_secret(secret.id, replacement.revision)?;
    home.create_shared_secret("shared", &SecretValue::new("fresh sentinel"))?;
    host.poll_runtime()?;
    assert_eq!(
        observed.lock().unwrap()["fixture.second"],
        Err(SecretAccessError::NotFound)
    );
    assert_eq!(home.list_secret_consumers()?.len(), 1);
    host.deactivate_world(world)?;
    home.delete_world(world)?;
    assert!(home.list_secret_consumers()?.is_empty());
    assert_eq!(home.list_shared_secrets()?.len(), 2);
    host.shutdown()?;
    Ok(())
}
