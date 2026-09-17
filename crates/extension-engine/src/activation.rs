//! Deterministic activation planning over registered contract topology.

use std::{
    cmp::Reverse,
    collections::{BinaryHeap, HashMap, HashSet},
    fmt,
};

use crate::composition::{CompositionSnapshot, UnresolvedContractReason};
use rintawa_sdk::{
    contracts::ContractKey,
    types::{ComponentId, ExtensionInstanceId},
};

/// Deterministic extension-instance order for one activation batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivationPlan {
    ordered_instances: Vec<ExtensionInstanceId>,
}

impl ActivationPlan {
    /// Returns instances in provider-before-consumer activation order.
    pub fn ordered_instances(&self) -> &[ExtensionInstanceId] {
        &self.ordered_instances
    }

    /// Consumes the plan and returns its provider-before-consumer order.
    pub fn into_ordered_instances(self) -> Vec<ExtensionInstanceId> {
        self.ordered_instances
    }
}

/// Errors produced while validating or ordering one activation batch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivationPlanError {
    /// One runtime instance appears more than once in the requested activation batch.
    DuplicateInstance {
        /// Repeated runtime instance.
        instance_id: ExtensionInstanceId,
    },

    /// A required consumer could not resolve a compatible provider binding.
    RequiredContractUnresolved {
        /// Consumer runtime instance.
        instance_id: ExtensionInstanceId,
        /// Consumer component.
        component_id: ComponentId,
        /// Required contract.
        contract: ContractKey,
        /// Resolution failure reported by the composition resolver.
        reason: UnresolvedContractReason,
    },

    /// Required provider edges contain a cycle and therefore cannot be started safely.
    DependencyCycle {
        /// Deterministic cycle path with the first instance repeated at the end.
        instances: Vec<ExtensionInstanceId>,
    },
}

impl fmt::Display for ActivationPlanError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateInstance { instance_id } => write!(
                formatter,
                "extension instance `{instance_id}` appears more than once in the activation plan"
            ),
            Self::RequiredContractUnresolved {
                instance_id,
                component_id,
                contract,
                reason,
            } => write!(
                formatter,
                "required contract `{contract}` for component `{component_id}` in extension \
                 instance `{instance_id}` is unresolved: {reason}"
            ),
            Self::DependencyCycle { instances } => {
                formatter.write_str("activation dependency cycle detected: ")?;
                for (index, instance_id) in instances.iter().enumerate() {
                    if index > 0 {
                        formatter.write_str(" -> ")?;
                    }
                    write!(formatter, "{instance_id}")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ActivationPlanError {}

pub(crate) fn build_activation_plan(
    snapshot: &CompositionSnapshot,
    scheduled_instances: &[ExtensionInstanceId],
    active_instances: &HashSet<ExtensionInstanceId>,
) -> Result<ActivationPlan, ActivationPlanError> {
    let mut scheduled_indices = HashMap::new();
    for (index, instance_id) in scheduled_instances.iter().enumerate() {
        if scheduled_indices
            .insert(instance_id.clone(), index)
            .is_some()
        {
            return Err(ActivationPlanError::DuplicateInstance {
                instance_id: instance_id.clone(),
            });
        }
    }

    if let Some(unresolved) = snapshot
        .unresolved
        .iter()
        .find(|entry| entry.required && scheduled_indices.contains_key(&entry.consumer.instance_id))
    {
        return Err(ActivationPlanError::RequiredContractUnresolved {
            instance_id: unresolved.consumer.instance_id.clone(),
            component_id: unresolved.consumer.component_id.clone(),
            contract: unresolved.contract.clone(),
            reason: unresolved.reason,
        });
    }

    let instance_count = scheduled_instances.len();
    let mut outgoing = vec![Vec::new(); instance_count];
    let mut incoming = vec![Vec::new(); instance_count];
    let mut indegrees = vec![0_usize; instance_count];
    let mut edges = HashSet::new();

    for binding in snapshot.bindings.iter().filter(|entry| entry.required) {
        let Some(&consumer_index) = scheduled_indices.get(&binding.consumer.instance_id) else {
            continue;
        };

        for provider in &binding.providers {
            if provider.instance_id == binding.consumer.instance_id {
                continue;
            }
            if let Some(&provider_index) = scheduled_indices.get(&provider.instance_id) {
                if edges.insert((provider_index, consumer_index)) {
                    outgoing[provider_index].push(consumer_index);
                    incoming[consumer_index].push(provider_index);
                    indegrees[consumer_index] += 1;
                }
                continue;
            }
            debug_assert!(
                active_instances.contains(&provider.instance_id),
                "activation composition included a provider outside the active or scheduled set"
            );
        }
    }

    for dependents in &mut outgoing {
        dependents.sort_unstable();
    }
    for dependencies in &mut incoming {
        dependencies.sort_unstable();
    }

    let mut ready = BinaryHeap::new();
    for (index, indegree) in indegrees.iter().enumerate() {
        if *indegree == 0 {
            ready.push(Reverse(index));
        }
    }

    let mut ordered_indices = Vec::with_capacity(instance_count);
    while let Some(Reverse(index)) = ready.pop() {
        ordered_indices.push(index);
        for &dependent in &outgoing[index] {
            indegrees[dependent] -= 1;
            if indegrees[dependent] == 0 {
                ready.push(Reverse(dependent));
            }
        }
    }

    if ordered_indices.len() != instance_count {
        return Err(ActivationPlanError::DependencyCycle {
            instances: find_dependency_cycle(scheduled_instances, &incoming, &indegrees),
        });
    }

    Ok(ActivationPlan {
        ordered_instances: ordered_indices
            .into_iter()
            .map(|index| scheduled_instances[index].clone())
            .collect(),
    })
}

fn find_dependency_cycle(
    scheduled_instances: &[ExtensionInstanceId],
    incoming: &[Vec<usize>],
    residual_indegrees: &[usize],
) -> Vec<ExtensionInstanceId> {
    let Some(mut current) = residual_indegrees.iter().position(|degree| *degree > 0) else {
        return Vec::new();
    };
    let mut path: Vec<usize> = Vec::new();
    let mut positions: HashMap<usize, usize> = HashMap::new();

    loop {
        if let Some(&cycle_start) = positions.get(&current) {
            let mut cycle: Vec<_> = path[cycle_start..]
                .iter()
                .map(|&index| scheduled_instances[index].clone())
                .collect();
            cycle.push(scheduled_instances[current].clone());
            return cycle;
        }
        positions.insert(current, path.len());
        path.push(current);

        let next = incoming[current]
            .iter()
            .copied()
            .filter(|&index| residual_indegrees[index] > 0)
            .min();
        let Some(next) = next else {
            return residual_indegrees
                .iter()
                .enumerate()
                .filter(|(_, degree)| **degree > 0)
                .map(|(index, _)| scheduled_instances[index].clone())
                .collect();
        };
        current = next;
    }
}
