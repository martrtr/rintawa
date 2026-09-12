//! Contract provider/consumer resolution.

use std::collections::HashMap;

use rintawa_sdk::contracts::{
    ComponentRef, ContractConsumer, ContractDefinition, ContractGrantRequirement, ContractKey,
    ContractProvider, ContractResolutionPolicy,
};

use crate::secrets::SecretManager;

#[derive(Debug, Clone)]
pub(crate) struct OwnedContractDefinition {
    pub(crate) owner: ComponentRef,
    pub(crate) definition: ContractDefinition,
}

#[derive(Debug, Clone)]
pub(crate) struct OwnedContractProvider {
    pub(crate) owner: ComponentRef,
    pub(crate) provider: ContractProvider,
}

#[derive(Debug, Clone)]
pub(crate) struct OwnedContractConsumer {
    pub(crate) owner: ComponentRef,
    pub(crate) consumer: ContractConsumer,
}

/// One resolved consumer-to-provider contract binding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractBinding {
    /// Consumer endpoint receiving the binding.
    pub consumer: ComponentRef,
    /// Contract being resolved.
    pub contract: ContractKey,
    /// Providers selected by the contract resolution policy.
    pub providers: Vec<ComponentRef>,
}

/// Reason a consumer currently has no active binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnresolvedContractReason {
    /// No active definition exists for the contract.
    UndefinedContract,
    /// The consumer lacks one or more host grants required by its endpoint.
    ConsumerIneligible,
    /// No active provider is registered for the contract.
    NoProvider,
    /// Providers exist, but none currently satisfy their host-grant requirements.
    NoEligibleProvider,
    /// The preferred provider is not currently eligible for this contract.
    PreferredProviderUnavailable,
}

/// One unresolved consumer endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnresolvedContract {
    /// Consumer endpoint that could not be bound.
    pub consumer: ComponentRef,
    /// Contract requested by the consumer.
    pub contract: ContractKey,
    /// Whether the consumer declared the contract as required.
    pub required: bool,
    /// Current reason resolution failed.
    pub reason: UnresolvedContractReason,
}

/// Current resolved and unresolved contract state.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CompositionSnapshot {
    /// Active bindings.
    pub bindings: Vec<ContractBinding>,
    /// Active consumer endpoints without a binding.
    pub unresolved: Vec<UnresolvedContract>,
}

pub(crate) fn resolve_contracts(
    definitions: &[OwnedContractDefinition],
    providers: &[OwnedContractProvider],
    consumers: &[OwnedContractConsumer],
    preferred_providers: &HashMap<ContractKey, ComponentRef>,
    secrets: &SecretManager,
) -> CompositionSnapshot {
    let mut snapshot = CompositionSnapshot::default();

    for owned_consumer in consumers {
        let contract = &owned_consumer.consumer.contract;
        let Some(definition) = definitions
            .iter()
            .find(|candidate| candidate.definition.contract == *contract)
        else {
            snapshot.unresolved.push(UnresolvedContract {
                consumer: owned_consumer.owner.clone(),
                contract: contract.clone(),
                required: owned_consumer.consumer.required,
                reason: UnresolvedContractReason::UndefinedContract,
            });
            continue;
        };

        if !requirements_satisfied(
            secrets,
            &owned_consumer.owner,
            &owned_consumer.consumer.required_grants,
        ) {
            snapshot.unresolved.push(UnresolvedContract {
                consumer: owned_consumer.owner.clone(),
                contract: contract.clone(),
                required: owned_consumer.consumer.required,
                reason: UnresolvedContractReason::ConsumerIneligible,
            });
            continue;
        }

        let mut matching: Vec<_> = providers
            .iter()
            .filter(|candidate| candidate.provider.contract == *contract)
            .collect();

        if matching.is_empty() {
            snapshot.unresolved.push(UnresolvedContract {
                consumer: owned_consumer.owner.clone(),
                contract: contract.clone(),
                required: owned_consumer.consumer.required,
                reason: UnresolvedContractReason::NoProvider,
            });
            continue;
        }

        matching.retain(|candidate| {
            requirements_satisfied(
                secrets,
                &candidate.owner,
                &candidate.provider.required_grants,
            )
        });
        matching.sort_by(|left, right| {
            component_ref_key(&left.owner).cmp(&component_ref_key(&right.owner))
        });

        if matching.is_empty() {
            snapshot.unresolved.push(UnresolvedContract {
                consumer: owned_consumer.owner.clone(),
                contract: contract.clone(),
                required: owned_consumer.consumer.required,
                reason: UnresolvedContractReason::NoEligibleProvider,
            });
            continue;
        }

        let selected = match definition.definition.resolution {
            ContractResolutionPolicy::Single => {
                if let Some(preferred) = preferred_providers.get(contract) {
                    let Some(provider) = matching
                        .iter()
                        .find(|candidate| candidate.owner == *preferred)
                    else {
                        snapshot.unresolved.push(UnresolvedContract {
                            consumer: owned_consumer.owner.clone(),
                            contract: contract.clone(),
                            required: owned_consumer.consumer.required,
                            reason: UnresolvedContractReason::PreferredProviderUnavailable,
                        });
                        continue;
                    };
                    vec![provider.owner.clone()]
                } else {
                    vec![matching[0].owner.clone()]
                }
            }
            ContractResolutionPolicy::Multiple => matching
                .iter()
                .map(|provider| provider.owner.clone())
                .collect(),
        };

        snapshot.bindings.push(ContractBinding {
            consumer: owned_consumer.owner.clone(),
            contract: contract.clone(),
            providers: selected,
        });
    }

    snapshot.bindings.sort_by(|left, right| {
        component_ref_key(&left.consumer)
            .cmp(&component_ref_key(&right.consumer))
            .then_with(|| left.contract.to_string().cmp(&right.contract.to_string()))
    });
    snapshot.unresolved.sort_by(|left, right| {
        component_ref_key(&left.consumer)
            .cmp(&component_ref_key(&right.consumer))
            .then_with(|| left.contract.to_string().cmp(&right.contract.to_string()))
    });
    snapshot
}

fn requirements_satisfied(
    secrets: &SecretManager,
    owner: &ComponentRef,
    requirements: &[ContractGrantRequirement],
) -> bool {
    requirements.iter().all(|requirement| match requirement {
        ContractGrantRequirement::SecretRead { pattern } => {
            secrets.has_read_grant(&owner.extension_id, &owner.component_id, pattern)
        }
    })
}

fn component_ref_key(component: &ComponentRef) -> (&str, &str) {
    (
        component.extension_id.as_str(),
        component.component_id.as_str(),
    )
}
