//! Owner-scoped registry for reversible component runtime effects.

use std::collections::HashMap;

use rintawa_sdk::{
    contracts::ComponentRef,
    errors::{ExtensionError, ExtensionResult},
    runtime_effects::RuntimeEffect,
    types::{ExtensionInstanceId, RuntimeEffectId},
};

/// An effect together with the component that owns it.
#[derive(Debug, Clone)]
struct OwnedRuntimeEffect {
    owner: ComponentRef,
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
        owner: ComponentRef,
        effect: RuntimeEffect,
    ) -> ExtensionResult<RuntimeEffectId> {
        validate_runtime_effect(&effect)?;

        let effect_id = RuntimeEffectId::new(format!(
            "{}.{}.effect.{}",
            owner.instance_id, owner.component_id, self.next_effect_sequence
        ));
        self.next_effect_sequence = self.next_effect_sequence.checked_add(1).ok_or_else(|| {
            ExtensionError::Message(String::from(
                "runtime effect identifier sequence overflowed",
            ))
        })?;

        self.effects
            .insert(effect_id.clone(), OwnedRuntimeEffect { owner, effect });
        Ok(effect_id)
    }

    /// Revokes an effect only if it belongs to the caller.
    pub(crate) fn revoke(
        &mut self,
        effect_id: &RuntimeEffectId,
        caller: &ComponentRef,
    ) -> ExtensionResult<()> {
        let Some(owner) = self.effects.get(effect_id) else {
            return Err(ExtensionError::RuntimeEffectNotOwned(
                effect_id.as_str().to_string(),
            ));
        };

        if owner.owner != *caller {
            return Err(ExtensionError::RuntimeEffectNotOwned(
                effect_id.as_str().to_string(),
            ));
        }

        self.effects.remove(effect_id);
        Ok(())
    }

    /// Removes all effects owned by one extension instance after stop or crash.
    pub(crate) fn revoke_instance(&mut self, instance_id: &ExtensionInstanceId) {
        self.effects
            .retain(|_, effect| effect.owner.instance_id != *instance_id);
    }

    /// Removes all effects owned by one component after a component-level failure.
    pub(crate) fn revoke_component(&mut self, owner: &ComponentRef) {
        self.effects.retain(|_, effect| effect.owner != *owner);
    }

    /// Returns exact component principals subscribed to one event topic.
    ///
    /// Multiple active subscription handles owned by the same component still
    /// produce one callback. Owners are returned in deterministic principal order.
    pub(crate) fn event_subscribers(&self, topic: &str) -> Vec<ComponentRef> {
        let mut subscribers = self
            .effects
            .values()
            .filter_map(|effect| match &effect.effect {
                RuntimeEffect::EventSubscription {
                    topic: subscribed_topic,
                } if subscribed_topic == topic => Some(effect.owner.clone()),
                RuntimeEffect::EventSubscription { .. }
                | RuntimeEffect::SignalSubscription { .. } => None,
            })
            .collect::<Vec<_>>();
        subscribers.sort_by(|left, right| {
            left.instance_id
                .as_str()
                .cmp(right.instance_id.as_str())
                .then_with(|| left.component_id.as_str().cmp(right.component_id.as_str()))
        });
        subscribers.dedup();
        subscribers
    }

    /// Returns exact component principals subscribed to one ephemeral signal topic.
    pub(crate) fn signal_subscribers(&self, topic: &str) -> Vec<ComponentRef> {
        let mut subscribers = self
            .effects
            .values()
            .filter_map(|effect| match &effect.effect {
                RuntimeEffect::SignalSubscription {
                    topic: subscribed_topic,
                } if subscribed_topic == topic => Some(effect.owner.clone()),
                RuntimeEffect::SignalSubscription { .. }
                | RuntimeEffect::EventSubscription { .. } => None,
            })
            .collect::<Vec<_>>();
        subscribers.sort_by(|left, right| {
            left.instance_id
                .as_str()
                .cmp(right.instance_id.as_str())
                .then_with(|| left.component_id.as_str().cmp(right.component_id.as_str()))
        });
        subscribers.dedup();
        subscribers
    }

    /// Returns all currently installed effects with their runtime owner.
    pub(crate) fn active_effects(&self) -> Vec<(&RuntimeEffectId, &ComponentRef, &RuntimeEffect)> {
        let mut effects: Vec<_> = self
            .effects
            .iter()
            .map(|(effect_id, effect)| (effect_id, &effect.owner, &effect.effect))
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
        RuntimeEffect::SignalSubscription { topic } if topic.trim().is_empty() => {
            Err(ExtensionError::InvalidRuntimeEffect(String::from(
                "signal subscription topic must not be empty",
            )))
        }
        RuntimeEffect::EventSubscription { .. } | RuntimeEffect::SignalSubscription { .. } => {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner(instance: &str, component: &str) -> ComponentRef {
        ComponentRef::new(instance, component)
    }

    #[test]
    fn test_should_reject_revocation_by_a_different_component() {
        let mut registry = RuntimeEffectRegistry::default();
        let original_owner = owner("chat-a", "chat-runtime");
        let effect_id = registry
            .register(
                original_owner.clone(),
                RuntimeEffect::event_subscription("dialogue.message"),
            )
            .unwrap();

        let result = registry.revoke(&effect_id, &owner("chat-a", "other-component"));

        assert!(matches!(
            result,
            Err(ExtensionError::RuntimeEffectNotOwned(_))
        ));
        assert_eq!(registry.active_effects().len(), 1);

        registry.revoke(&effect_id, &original_owner).unwrap();
        assert!(registry.active_effects().is_empty());
    }

    #[test]
    fn test_should_revoke_only_the_failed_component_effects() {
        let mut registry = RuntimeEffectRegistry::default();
        let failed_owner = owner("chat-a", "failed-runtime");
        let healthy_owner = owner("chat-a", "healthy-runtime");

        registry
            .register(
                failed_owner.clone(),
                RuntimeEffect::event_subscription("dialogue.failed"),
            )
            .unwrap();
        registry
            .register(
                healthy_owner.clone(),
                RuntimeEffect::event_subscription("dialogue.healthy"),
            )
            .unwrap();

        registry.revoke_component(&failed_owner);

        let effects = registry.active_effects();
        assert_eq!(effects.len(), 1);
        assert_eq!(effects[0].1, &healthy_owner);
    }

    #[test]
    fn test_should_return_event_subscribers_once_in_principal_order() {
        let mut registry = RuntimeEffectRegistry::default();
        let owner_b = owner("b-instance", "runtime");
        let owner_a = owner("a-instance", "runtime");

        registry
            .register(
                owner_b.clone(),
                RuntimeEffect::event_subscription("dialogue.message"),
            )
            .unwrap();
        registry
            .register(
                owner_a.clone(),
                RuntimeEffect::event_subscription("dialogue.message"),
            )
            .unwrap();
        registry
            .register(
                owner_a.clone(),
                RuntimeEffect::event_subscription("dialogue.message"),
            )
            .unwrap();
        registry
            .register(
                owner_a.clone(),
                RuntimeEffect::event_subscription("dialogue.other"),
            )
            .unwrap();

        assert_eq!(
            registry.event_subscribers("dialogue.message"),
            vec![owner_a, owner_b]
        );
    }

    #[test]
    fn test_should_isolate_same_component_between_instances() {
        let mut registry = RuntimeEffectRegistry::default();
        let owner_a = owner("chat-a", "runtime");
        let owner_b = owner("chat-b", "runtime");
        registry
            .register(
                owner_a.clone(),
                RuntimeEffect::event_subscription("dialogue.a"),
            )
            .unwrap();
        registry
            .register(
                owner_b.clone(),
                RuntimeEffect::event_subscription("dialogue.b"),
            )
            .unwrap();

        registry.revoke_instance(&owner_a.instance_id);
        let effects = registry.active_effects();
        assert_eq!(effects.len(), 1);
        assert_eq!(effects[0].1, &owner_b);
    }
}
