//! Native-only credential configuration and persisted exact component approvals.
use crate::{DEFAULT_WORLD_SCOPE, HostError, HostHome, HostResult, SecretReadGrant};
use rintawa_sdk::{
    contracts::ComponentRef,
    secrets::{SecretPath, SecretValue},
    types::RuntimeScopeId,
};

/// Applies only this exact activation's persisted local approvals before start.
pub(crate) fn apply_secret_read_grants(
    engine: &mut rintawa_extension_engine::ExtensionEngine,
    profile: &crate::CompositionProfile,
    activation: &crate::ActivationRecord,
) -> HostResult<()> {
    for grant in profile.secret_read_grants.iter().filter(|grant| {
        grant.scope_id == activation.scope_id && grant.instance_id == activation.instance_id
    }) {
        engine.grant_requested_secret_read_for_instance(
            &activation.instance_id,
            &grant.component_id,
            rintawa_sdk::secrets::SecretPathPattern::Exact(grant.path.clone()),
        )?;
        if let Some(entry_id) = grant.entry_id {
            let entry = crate::shared_secrets::entry_path(entry_id);
            engine.secret_manager().bind_read(
                ComponentRef::new(grant.instance_id.clone(), grant.component_id.clone()),
                grant.path.clone(),
                entry,
            )?;
        }
    }
    Ok(())
}

impl HostHome {
    /// Lists all persisted consumers, including disabled activations/inactive Worlds.
    /// Trusted native administration only; no global catalog is exposed to guests.
    pub fn list_secret_consumers(&self) -> HostResult<Vec<SecretReadGrant>> {
        let mut grants = self.load_profile()?.secret_read_grants;
        for world in self.list_worlds()? {
            grants.extend(self.load_world_composition(world.id)?.secret_read_grants);
        }
        Ok(grants)
    }

    /// Stores a legacy direct path in the explicitly selected backend.
    /// This is a native administrator API, unavailable to guests and browser channels.
    pub fn store_secret(&self, path: &SecretPath, value: &SecretValue) -> HostResult<()> {
        self.secret_manager()?.store(path, value)?;
        Ok(())
    }
    /// Deletes a native credential, without reading or returning its value.
    pub fn delete_secret(&self, path: &SecretPath) -> HostResult<bool> {
        Ok(self.secret_manager()?.delete(path)?)
    }
    /// Persists one manifest-requested exact path for the next activation of a local owner.
    /// Approvals cannot be stored in the default-World recipe or copied to new Worlds.
    pub fn grant_secret_read(
        &self,
        scope: RuntimeScopeId,
        owner: ComponentRef,
        path: SecretPath,
    ) -> HostResult<()> {
        if scope.as_str() == DEFAULT_WORLD_SCOPE {
            return Err(HostError::InvalidSecretPolicy(
                "secret approvals cannot be defaults".into(),
            ));
        }
        let _lock = self.lock_composition_scope(&scope)?;
        let mut profile = self.load_composition_for_scope_locked(&scope)?;
        let activation = profile
            .activations
            .iter()
            .find(|a| a.scope_id == scope && a.instance_id == owner.instance_id)
            .ok_or_else(|| {
                HostError::InvalidSecretPolicy("activation is absent from this scope".into())
            })?;
        let manifest = self.extension_manifest(&activation.artifact)?;
        let component = manifest
            .components
            .iter()
            .find(|c| c.id == owner.component_id)
            .ok_or_else(|| {
                HostError::InvalidSecretPolicy(
                    "component is absent from the installed manifest".into(),
                )
            })?;
        if !component
            .permissions
            .secret_read
            .iter()
            .any(|p| p.matches(&path))
        {
            return Err(HostError::InvalidSecretPolicy(
                "component did not request this secret path".into(),
            ));
        }
        profile.grant_secret_read(SecretReadGrant {
            scope_id: scope.clone(),
            instance_id: owner.instance_id,
            component_id: owner.component_id,
            path,
            entry_id: None,
        })?;
        self.save_composition_for_scope(&scope, &profile)
    }
    /// Removes a persisted approval, including when the original RTW is unavailable.
    /// A currently running activation retains its policy until orderly stop/restart.
    pub fn revoke_secret_read(
        &self,
        scope: &RuntimeScopeId,
        owner: &ComponentRef,
        path: &SecretPath,
    ) -> HostResult<()> {
        let _lock = self.lock_composition_scope(scope)?;
        let mut profile = self.load_composition_for_scope_locked(scope)?;
        profile.revoke_secret_read(scope, owner, path);
        self.save_composition_for_scope(scope, &profile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ActivationRecord, CompositionProfile, HOST_SCOPE, world_runtime_scope_id};
    use rintawa_artifacts::{ContentType, RtwLimits, pack_directory};
    use rintawa_extension_engine::ExtensionEngine;
    use rintawa_sdk::{
        context::RegistrationContext,
        contracts::{
            ContractDefinition, ContractGrantRequirement, ContractKey, ContractProvider,
            ContractResolutionPolicy, ContractVersion,
        },
        errors::ExtensionResult,
        prelude::{Component, ComponentId},
        secrets::SecretPathPattern,
    };

    fn artifact(root: &std::path::Path) -> anyhow::Result<std::path::PathBuf> {
        let source = root.join("package");
        std::fs::create_dir(&source)?;
        std::fs::write(
            source.join("rtw.toml"),
            "format = 1\ncontent = \"rintawa.extension@1\"\nentry = \"manifest.toml\"\n",
        )?;
        std::fs::write(
            source.join("manifest.toml"),
            r#"id = "fixture.secrets"
name = "Secret policy fixture"
version = "0.1.0"
sdk = "^0.0"
[[components]]
id = "backend"
kind = "runtime"
target = "fixture.runtime.native@1"
[components.permissions]
secret-read = ["ai.*"]
"#,
        )?;
        let output = root.join("fixture.rtw");
        pack_directory(source, &output, RtwLimits::default())?;
        Ok(output)
    }
    struct RequiresSecret {
        id: ComponentId,
        path: SecretPath,
    }
    fn contract() -> ContractKey {
        ContractKey::new("fixture.secret-policy", ContractVersion::new(1))
    }
    impl Component for RequiresSecret {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }
        fn id(&self) -> &ComponentId {
            &self.id
        }
        fn register(&mut self, context: &mut dyn RegistrationContext) -> ExtensionResult<()> {
            context.define_contract(ContractDefinition::new(
                contract(),
                ContractResolutionPolicy::Single,
            ))?;
            context.provide_contract(ContractProvider::new(contract()).requiring(
                ContractGrantRequirement::SecretRead {
                    pattern: SecretPathPattern::Exact(self.path.clone()),
                },
            ))?;
            Ok(())
        }
    }
    #[test]
    fn test_should_restore_exact_secret_approvals_and_keep_other_world_ungranted()
    -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let home = HostHome::open(root.path().join("home"))?;
        let artifact = artifact(root.path())?;
        let imported = home.install_local_rtw(&artifact, None)?.activation;
        let world_a = home.create_world()?;
        let world_b = home.create_world()?;
        let a = home.select_world_stored_rtw(world_a.id, &imported.digest, Some(true))?;
        let b = home.select_world_stored_rtw(world_b.id, &imported.digest, Some(true))?;
        let scope = world_runtime_scope_id(world_a.id);
        let path = SecretPath::parse("ai.world_a.api_key")?;
        let owner = ComponentRef::new(a.instance_id.clone(), "backend");
        home.grant_secret_read(scope.clone(), owner.clone(), path.clone())?;
        home.grant_secret_read(scope.clone(), owner.clone(), path.clone())?;
        let reopened = HostHome::open(home.root())?;
        let profile = reopened.load_world_composition(world_a.id)?;
        assert_eq!(profile.secret_read_grants.len(), 1);
        assert!(
            reopened
                .load_world_composition(world_b.id)?
                .secret_read_grants
                .is_empty()
        );
        let manifest = home.extension_manifest(&imported.digest)?;
        for _ in 0..2 {
            let mut engine = ExtensionEngine::new();
            for activation in [&a, &b] {
                engine.register_extension_instance(
                    activation.instance_id.clone(),
                    activation.scope_id.clone(),
                    manifest.clone(),
                    vec![Box::new(RequiresSecret {
                        id: ComponentId::new("backend"),
                        path: path.clone(),
                    })],
                )?;
                let record = ActivationRecord {
                    subject: "fixture.secrets".into(),
                    content: ContentType::parse("rintawa.extension@1")?,
                    artifact: imported.digest.clone(),
                    instance_id: activation.instance_id.clone(),
                    scope_id: activation.scope_id.clone(),
                    enabled: true,
                };
                // Deliberately pass A's profile for B too: the scoped filter must not grant it.
                apply_secret_read_grants(&mut engine, &profile, &record)?;
                engine.start_extension_instance(&activation.instance_id)?;
            }
            assert_eq!(
                engine
                    .resolve_active_contract_providers_in_scope(&scope, &contract())
                    .unwrap()
                    .len(),
                1
            );
            assert!(
                engine
                    .resolve_active_contract_providers_in_scope(&b.scope_id, &contract())
                    .map(|p| p.is_empty())
                    .unwrap_or(true)
            );
            engine.stop_extension_instance(&a.instance_id)?;
            // World teardown stops and unregisters instances before reactivation.
            engine.unregister_extension_instance(&a.instance_id)?;
            engine.register_extension_instance(
                a.instance_id.clone(),
                a.scope_id.clone(),
                manifest.clone(),
                vec![Box::new(RequiresSecret {
                    id: ComponentId::new("backend"),
                    path: path.clone(),
                })],
            )?;
            engine.start_extension_instance(&a.instance_id)?;
            assert!(
                engine
                    .resolve_active_contract_providers_in_scope(&scope, &contract())
                    .map(|p| p.is_empty())
                    .unwrap_or(true),
                "World teardown revokes live secret grants"
            );
        }
        assert!(
            profile
                .materialize_all(world_runtime_scope_id(world_b.id))
                .secret_read_grants
                .is_empty()
        );
        reopened.revoke_secret_read(&scope, &owner, &path)?;
        assert!(
            reopened
                .load_world_composition(world_a.id)?
                .secret_read_grants
                .is_empty()
        );
        Ok(())
    }

    #[test]
    fn managed_binding_round_trips_and_is_not_overwritten_by_legacy_grant() -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let home = HostHome::open(root.path().join("home"))?;
        let activation = home
            .install_local_rtw(artifact(root.path())?, None)?
            .activation;
        let scope = RuntimeScopeId::new(HOST_SCOPE);
        let owner = ComponentRef::new(activation.instance_id, "backend");
        let path = SecretPath::parse("ai.example.key")?;
        let entry_id = uuid::Uuid::now_v7();
        let mut profile = home.load_profile()?;
        profile.grant_secret_read(SecretReadGrant {
            scope_id: scope.clone(),
            instance_id: owner.instance_id.clone(),
            component_id: owner.component_id.clone(),
            path: path.clone(),
            entry_id: Some(entry_id),
        })?;
        profile.save(&home.profile_path)?;
        home.grant_secret_read(scope, owner, path)?;
        let reloaded = home.load_profile()?;
        assert_eq!(reloaded.secret_read_grants.len(), 1);
        assert_eq!(reloaded.secret_read_grants[0].entry_id, Some(entry_id));
        Ok(())
    }
    #[test]
    fn test_should_reject_undeclared_foreign_duplicate_and_excess_secret_approvals_atomically()
    -> anyhow::Result<()> {
        let root = tempfile::tempdir()?;
        let home = HostHome::open(root.path().join("home"))?;
        let a = home
            .install_local_rtw(artifact(root.path())?, None)?
            .activation;
        let scope = RuntimeScopeId::new(HOST_SCOPE);
        let owner = ComponentRef::new(a.instance_id.clone(), "backend");
        let path = SecretPath::parse("ai.local.api_key")?;
        for (s, o, p) in [
            (
                scope.clone(),
                ComponentRef::new("missing", "backend"),
                path.clone(),
            ),
            (
                scope.clone(),
                ComponentRef::new(a.instance_id.clone(), "missing"),
                path.clone(),
            ),
            (
                scope.clone(),
                owner.clone(),
                SecretPath::parse("other.key")?,
            ),
            (
                RuntimeScopeId::new(DEFAULT_WORLD_SCOPE),
                owner.clone(),
                path.clone(),
            ),
        ] {
            let before = home.load_profile()?;
            assert!(home.grant_secret_read(s, o, p).is_err());
            assert_eq!(home.load_profile()?, before);
        }
        home.grant_secret_read(scope.clone(), owner.clone(), path.clone())?;
        let mut profile = home.load_profile()?;
        profile
            .secret_read_grants
            .push(profile.secret_read_grants[0].clone());
        assert!(profile.save(&home.profile_path).is_err());
        for index in 1..256 {
            home.grant_secret_read(
                scope.clone(),
                owner.clone(),
                SecretPath::parse(format!("ai.key_{index}"))?,
            )?;
        }
        let before = std::fs::read(&home.profile_path)?;
        assert!(
            home.grant_secret_read(scope, owner, SecretPath::parse("ai.excess")?)
                .is_err()
        );
        assert_eq!(std::fs::read(&home.profile_path)?, before);
        let profile = CompositionProfile {
            secret_read_grants: home.load_profile()?.secret_read_grants,
            ..CompositionProfile::default()
        };
        assert!(
            profile.save(&home.profile_path).is_err(),
            "absent activations cannot acquire secret permissions"
        );
        Ok(())
    }
}
