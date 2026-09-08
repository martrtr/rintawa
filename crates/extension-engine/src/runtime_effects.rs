//! Owner-scoped registry for reversible component runtime effects.

use std::collections::HashMap;

use rintawa_sdk::{
    errors::{ExtensionError, ExtensionResult},
    runtime_effects::RuntimeEffect,
    types::{ComponentId, ExtensionId, RuntimeEffectId},
};

/// An effect together with the component that owns it.
#[derive(Debug, Clone)]
struct OwnedRuntimeEffect {
    extension_id: ExtensionId,
    component_id: ComponentId,
    effect: RuntimeEffect,
}

/// Stores runtime effects while their owners are active.
#[derive(Debug, Default)]
pub(crate) struct RuntimeEffectRegistry {
    next_effect_sequence: u64,
    effects: HashMap<RuntimeEffectId, OwnedRuntimeEffect>,
}

impl RuntimeEffectRegistry {
    /// Registers one validated effect for an active component.
    pub(crate) fn register(
        &mut self,
        extension_id: ExtensionId,
        component_id: ComponentId,
        effect: RuntimeEffect,
    ) -> ExtensionResult<RuntimeEffectId> {
        validate_runtime_effect(&effect)?;

        let effect_id = RuntimeEffectId::new(format!(
            "{}.{}.effect.{}",
            extension_id, component_id, self.next_effect_sequence
        ));
        self.next_effect_sequence = self.next_effect_sequence.checked_add(1).ok_or_else(|| {
            ExtensionError::Message(String::from(
                "runtime effect identifier sequence overflowed",
            ))
        })?;

        self.effects.insert(
            effect_id.clone(),
            OwnedRuntimeEffect {
                extension_id,
                component_id,
                effect,
            },
        );
        Ok(effect_id)
    }

    /// Revokes an effect only if it belongs to the caller.
    pub(crate) fn revoke(
        &mut self,
        effect_id: &RuntimeEffectId,
        extension_id: &ExtensionId,
        component_id: &ComponentId,
    ) -> ExtensionResult<()> {
        let Some(owner) = self.effects.get(effect_id) else {
            return Err(ExtensionError::RuntimeEffectNotOwned(
                effect_id.as_str().to_string(),
            ));
        };

        if owner.extension_id != *extension_id || owner.component_id != *component_id {
            return Err(ExtensionError::RuntimeEffectNotOwned(
                effect_id.as_str().to_string(),
            ));
        }

        self.effects.remove(effect_id);
        Ok(())
    }

    /// Removes all effects owned by an extension after failed start, stop, or crash.
    pub(crate) fn revoke_extension(&mut self, extension_id: &ExtensionId) {
        self.effects
            .retain(|_, owner| owner.extension_id != *extension_id);
    }

    /// Removes all effects owned by one component after a component-level failure.
    pub(crate) fn revoke_component(
        &mut self,
        extension_id: &ExtensionId,
        component_id: &ComponentId,
    ) {
        self.effects.retain(|_, owner| {
            owner.extension_id != *extension_id || owner.component_id != *component_id
        });
    }

    /// Returns all currently installed effects with their owner.
    pub(crate) fn active_effects(
        &self,
    ) -> Vec<(&RuntimeEffectId, &ExtensionId, &ComponentId, &RuntimeEffect)> {
        let mut effects: Vec<_> = self
            .effects
            .iter()
            .map(|(effect_id, owner)| {
                (
                    effect_id,
                    &owner.extension_id,
                    &owner.component_id,
                    &owner.effect,
                )
            })
            .collect();
        effects.sort_by(|left, right| left.0.as_str().cmp(right.0.as_str()));
        effects
    }
}

fn validate_runtime_effect(effect: &RuntimeEffect) -> ExtensionResult<()> {
    match effect {
        RuntimeEffect::EventSubscription { topic } if topic.trim().is_empty() => {
            Err(ExtensionError::InvalidRuntimeEffect(String::from(
                "event subscription topic must not be empty",
            )))
        }
        RuntimeEffect::EventSubscription { .. } => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_reject_revocation_by_a_different_component() {
        let mut registry = RuntimeEffectRegistry::default();
        let extension_id = ExtensionId::new("rintawa.chat");
        let component_id = ComponentId::new("chat-runtime");
        let effect_id = registry
            .register(
                extension_id.clone(),
                component_id.clone(),
                RuntimeEffect::event_subscription("dialogue.message"),
            )
            .unwrap();

        let result = registry.revoke(
            &effect_id,
            &extension_id,
            &ComponentId::new("other-component"),
        );

        assert!(matches!(
            result,
            Err(ExtensionError::RuntimeEffectNotOwned(_))
        ));
        assert_eq!(registry.active_effects().len(), 1);

        registry
            .revoke(&effect_id, &extension_id, &component_id)
            .unwrap();
        assert!(registry.active_effects().is_empty());
    }

    #[test]
    fn test_should_revoke_only_the_failed_component_effects() {
        let mut registry = RuntimeEffectRegistry::default();
        let extension_id = ExtensionId::new("rintawa.chat");
        let failed_component_id = ComponentId::new("failed-runtime");
        let healthy_component_id = ComponentId::new("healthy-runtime");

        registry
            .register(
                extension_id.clone(),
                failed_component_id.clone(),
                RuntimeEffect::event_subscription("dialogue.failed"),
            )
            .unwrap();
        registry
            .register(
                extension_id.clone(),
                healthy_component_id.clone(),
                RuntimeEffect::event_subscription("dialogue.healthy"),
            )
            .unwrap();

        registry.revoke_component(&extension_id, &failed_component_id);

        let effects = registry.active_effects();
        assert_eq!(effects.len(), 1);
        assert_eq!(effects[0].2, &healthy_component_id);
    }
}
