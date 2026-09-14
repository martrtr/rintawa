use std::collections::{HashMap, HashSet};

use rintawa_sdk::ui::{UiCapabilityId, UiError, UiNode, UiNodeId, UiSurfaceSnapshot};

pub(crate) const DEFAULT_MAX_SURFACE_NODES: usize = 4096;
pub(crate) const DEFAULT_MAX_PATCH_OPERATIONS: usize = 1024;

pub(crate) fn validate_snapshot(snapshot: &UiSurfaceSnapshot) -> Result<(), UiError> {
    if snapshot.nodes.len() > DEFAULT_MAX_SURFACE_NODES {
        return Err(UiError::SurfaceTooLarge {
            actual: snapshot.nodes.len(),
            maximum: DEFAULT_MAX_SURFACE_NODES,
        });
    }

    let nodes = node_map(snapshot)?;
    if !nodes.contains_key(&snapshot.root) {
        return Err(UiError::RootNotFound(snapshot.root.to_string()));
    }

    let mut parent_of = HashMap::new();
    for node in nodes.values() {
        for child in node.kind.children() {
            if !nodes.contains_key(child) {
                return Err(UiError::NodeNotFound(child.to_string()));
            }
            if parent_of.insert(child.clone(), node.id.clone()).is_some() {
                return Err(UiError::MultipleParents(child.to_string()));
            }
        }
    }

    if parent_of.contains_key(&snapshot.root) {
        return Err(UiError::RootHasParent(snapshot.root.to_string()));
    }

    detect_cycles(&nodes, &parent_of)?;
    validate_reachability(snapshot, &nodes)?;
    Ok(())
}

pub(crate) fn node_map(snapshot: &UiSurfaceSnapshot) -> Result<HashMap<UiNodeId, UiNode>, UiError> {
    let mut nodes = HashMap::with_capacity(snapshot.nodes.len());
    for node in &snapshot.nodes {
        if nodes.insert(node.id.clone(), node.clone()).is_some() {
            return Err(UiError::DuplicateNode(node.id.to_string()));
        }
    }
    Ok(nodes)
}

pub(crate) fn required_capabilities(snapshot: &UiSurfaceSnapshot) -> HashSet<UiCapabilityId> {
    snapshot
        .nodes
        .iter()
        .map(|node| node.kind.required_capability())
        .collect()
}

fn detect_cycles(
    nodes: &HashMap<UiNodeId, UiNode>,
    parent_of: &HashMap<UiNodeId, UiNodeId>,
) -> Result<(), UiError> {
    for start in nodes.keys() {
        let mut chain = HashSet::new();
        let mut current = start;
        while let Some(parent) = parent_of.get(current) {
            if !chain.insert(current.clone()) {
                return Err(UiError::CycleDetected(current.to_string()));
            }
            current = parent;
        }
    }
    Ok(())
}

fn validate_reachability(
    snapshot: &UiSurfaceSnapshot,
    nodes: &HashMap<UiNodeId, UiNode>,
) -> Result<(), UiError> {
    let mut visited = HashSet::new();
    let mut pending = vec![snapshot.root.clone()];
    while let Some(node_id) = pending.pop() {
        if !visited.insert(node_id.clone()) {
            continue;
        }
        if let Some(node) = nodes.get(&node_id) {
            pending.extend(node.kind.children().iter().cloned());
        }
    }

    if visited.len() != nodes.len() {
        let mut orphaned: Vec<_> = nodes.keys().filter(|id| !visited.contains(*id)).collect();
        orphaned.sort_by(|left, right| left.as_str().cmp(right.as_str()));
        if let Some(orphan) = orphaned.first() {
            return Err(UiError::OrphanNode(orphan.to_string()));
        }
    }
    Ok(())
}
