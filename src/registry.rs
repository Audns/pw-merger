use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use log::{debug, info, warn};
use pipewire::registry::GlobalObject;
use pipewire::spa::utils::dict::DictRef;
use pipewire::types::ObjectType;

/// Everything we know about a single PipeWire node that we care about.
#[derive(Debug, Clone)]
pub struct NodeInfo {
    pub id: u32,
    pub name: String,
    /// True once the loopback link into this node has been established.
    pub linked: bool,
}

/// Shared state updated by the registry listener and read by the merger.
#[derive(Debug, Default)]
pub struct NodeTracker {
    inner: Arc<Mutex<Inner>>,
}

#[derive(Debug, Default)]
struct Inner {
    /// All known nodes keyed by PipeWire global ID.
    nodes: HashMap<u32, NodeInfo>,
    /// node.name values we are interested in.
    targets: Vec<String>,
    /// IDs that disappeared and need their loopbacks recreated.
    pending_reconnect: Vec<String>,
}

impl NodeTracker {
    pub fn new(targets: Vec<String>) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                targets,
                ..Default::default()
            })),
        }
    }

    /// Called by the registry listener when a new global appears.
    pub fn on_global(&self, global: &GlobalObject<&DictRef>) {
        if global.type_ != ObjectType::Node {
            return;
        }
        let Some(props) = global.props else { return };
        let name = match props.get("node.name") {
            Some(n) => n.to_owned(),
            None => return,
        };

        let mut g = self.inner.lock().unwrap();
        if !g.targets.contains(&name) {
            return;
        }

        info!("discovered target node: '{name}' id={}", global.id);
        g.nodes.insert(
            global.id,
            NodeInfo {
                id: global.id,
                name: name.clone(),
                linked: false,
            },
        );

        // If this node was previously lost, flag it for reconnect.
        if g.pending_reconnect.contains(&name) {
            g.pending_reconnect.retain(|n| n != &name);
            info!("node '{name}' reappeared — will reconnect loopback");
        }
    }

    /// Called by the registry listener when a global is removed.
    pub fn on_global_remove(&self, id: u32) {
        let mut g = self.inner.lock().unwrap();
        if let Some(info) = g.nodes.remove(&id) {
            warn!(
                "target node '{}' (id={id}) disappeared — \
                 will reconnect when it comes back",
                info.name
            );
            g.pending_reconnect.push(info.name);
        }
    }

    /// Returns nodes that exist but have not yet been linked.
    pub fn unlinked_nodes(&self) -> Vec<NodeInfo> {
        let g = self.inner.lock().unwrap();
        g.nodes.values().filter(|n| !n.linked).cloned().collect()
    }

    /// Returns nodes that need their loopback recreated after a reconnect.
    #[allow(dead_code)]
    pub fn nodes_needing_reconnect(&self) -> Vec<NodeInfo> {
        let g = self.inner.lock().unwrap();
        g.nodes
            .values()
            .filter(|n| {
                // Appeared again (linked flag was reset on removal) but
                // the loopback for it is gone.
                !n.linked
            })
            .cloned()
            .collect()
    }

    /// Mark a node as successfully linked.
    pub fn mark_linked(&self, id: u32) {
        let mut g = self.inner.lock().unwrap();
        if let Some(n) = g.nodes.get_mut(&id) {
            debug!("marked node '{}' (id={id}) as linked", n.name);
            n.linked = true;
        }
    }

    /// Reset the linked flag for a node (e.g. when its loopback is destroyed).
    #[allow(dead_code)]
    pub fn mark_unlinked(&self, id: u32) {
        let mut g = self.inner.lock().unwrap();
        if let Some(n) = g.nodes.get_mut(&id) {
            debug!("marked node '{}' (id={id}) as unlinked", n.name);
            n.linked = false;
        }
    }

    /// How many of our target nodes have we seen so far?
    #[allow(dead_code)]
    pub fn seen_count(&self) -> usize {
        self.inner.lock().unwrap().nodes.len()
    }

    /// Total number of target nodes we are looking for.
    #[allow(dead_code)]
    pub fn target_count(&self) -> usize {
        self.inner.lock().unwrap().targets.len()
    }
}

impl Clone for NodeTracker {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}
