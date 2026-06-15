/// merger.rs — Core PipeWire integration.
///
/// Strategy
/// --------
/// 1. Connect to PipeWire and subscribe to the global registry.
/// 2. Create a virtual null-audio-sink ("Merged Output").
/// 3. For every target device, link its playback ports to the null sink's
///    monitor ports via direct port links (no loopback module needed).
/// 4. Keep the main loop running.  Holding the link proxy objects keeps
///    the links alive through media pauses and device reconnects.
/// 5. On SIGINT/SIGTERM the main loop is quit; PipeWire cleans up our objects
///    when the connection is dropped.
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use log::{debug, error, info, warn};

use pipewire as pw;
use pipewire::context::ContextBox;
use pipewire::link::Link;
use pipewire::loop_::Timeout;
use pipewire::main_loop::MainLoopRc;
use pipewire::properties::properties;
use pipewire::proxy::ProxyT;
use pipewire::registry::GlobalObject;
use pipewire::spa::utils::dict::DictRef;
use pipewire::types::ObjectType;

use crate::Args;
use crate::registry::NodeTracker;

/// A discovered port on a PipeWire node.
#[derive(Debug, Clone)]
struct PortInfo {
    id: u32,
    node_id: u32,
    name: String,
    direction: PortDirection,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PortDirection {
    In,
    Out,
}

/// Shared state for tracking ports across the registry.
#[derive(Debug, Default)]
struct PortTracker {
    inner: Arc<Mutex<PortTrackerInner>>,
}

#[derive(Debug, Default)]
struct PortTrackerInner {
    /// All known ports keyed by port global ID.
    ports: HashMap<u32, PortInfo>,
    /// Map from node name to its node ID.
    node_ids: HashMap<String, u32>,
}

impl PortTracker {
    fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(PortTrackerInner::default())),
        }
    }

    fn on_global(&self, global: &GlobalObject<&DictRef>) {
        let props = match global.props {
            Some(p) => p,
            None => return,
        };

        match global.type_ {
            ObjectType::Port => {
                let node_id: u32 = match props.get("node.id") {
                    Some(id) => id.parse().unwrap_or(0),
                    None => return,
                };
                let port_name = match props.get("port.name") {
                    Some(n) => n.to_owned(),
                    None => return,
                };
                let direction = match props.get("port.direction") {
                    Some("in") => PortDirection::In,
                    Some("out") => PortDirection::Out,
                    _ => return,
                };

                let port = PortInfo {
                    id: global.id,
                    node_id,
                    name: port_name,
                    direction,
                };
                debug!(
                    "discovered port: id={} node={} name={} dir={:?}",
                    port.id, port.node_id, port.name, port.direction
                );
                self.inner.lock().unwrap().ports.insert(global.id, port);
            }
            ObjectType::Node => {
                if let Some(name) = props.get("node.name") {
                    self.inner
                        .lock()
                        .unwrap()
                        .node_ids
                        .insert(name.to_owned(), global.id);
                }
            }
            _ => {}
        }
    }

    /// Get the monitor (output) ports of a node by name.
    fn monitor_ports(&self, node_name: &str) -> Vec<PortInfo> {
        let g = self.inner.lock().unwrap();
        let node_id = match g.node_ids.get(node_name) {
            Some(id) => *id,
            None => return vec![],
        };
        g.ports
            .values()
            .filter(|p| p.node_id == node_id && p.direction == PortDirection::Out)
            .cloned()
            .collect()
    }

    /// Get the playback (input) ports of a node by name.
    fn playback_ports(&self, node_name: &str) -> Vec<PortInfo> {
        let g = self.inner.lock().unwrap();
        let node_id = match g.node_ids.get(node_name) {
            Some(id) => *id,
            None => return vec![],
        };
        g.ports
            .values()
            .filter(|p| p.node_id == node_id && p.direction == PortDirection::In)
            .cloned()
            .collect()
    }

    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
        }
    }
}

// ── public entry point ────────────────────────────────────────────────────────

pub fn run(args: Args, device_a: String, device_b: String) -> Result<()> {
    let main_loop = MainLoopRc::new(None).context("failed to create PipeWire main loop")?;
    let context =
        ContextBox::new(main_loop.loop_(), None).context("failed to create PipeWire context")?;
    let core = context
        .connect(None)
        .context("failed to connect to PipeWire daemon — is it running?")?;
    let registry = core
        .get_registry()
        .context("failed to get PipeWire registry")?;

    // ── shutdown flag ──────────────────────────────────────────────────────
    let quit = Arc::new(AtomicBool::new(false));
    let quit_ctrlc = quit.clone();
    ctrlc::set_handler(move || {
        info!("received shutdown signal");
        quit_ctrlc.store(true, Ordering::SeqCst);
    })
    .context("failed to install Ctrl-C handler")?;

    // ── trackers ───────────────────────────────────────────────────────────
    let tracker = NodeTracker::new(vec![device_a.clone(), device_b.clone()]);
    let ports = PortTracker::new();

    // We store created link proxies here so they stay alive.
    let links: Rc<RefCell<Vec<Link>>> = Rc::new(RefCell::new(Vec::new()));

    // ── registry listener ────────────────────────────────────────────────────
    let tracker_reg = tracker.clone();
    let ports_reg = ports.clone();

    let _registry_listener = registry
        .add_listener_local()
        .global({
            let tracker = tracker_reg.clone();
            let ports = ports_reg.clone();
            move |global| {
                tracker.on_global(global);
                ports.on_global(global);
            }
        })
        .global_remove({
            let tracker = tracker_reg.clone();
            move |id| {
                tracker.on_global_remove(id);
            }
        })
        .register();

    // ── create the virtual null sink ─────────────────────────────────────────
    info!("creating virtual null sink: '{}'", args.sink_name);
    let _null_sink = create_null_sink(&core, &args.sink_name, &args.media_role)
        .context("failed to create virtual null sink")?;

    // Sync so the null sink appears in the registry before we start the loop.
    let pending_sync = Rc::new(RefCell::new(true));
    let pending_sync_clone = Rc::clone(&pending_sync);
    let _sync_listener = core
        .add_listener_local()
        .done(move |_id, _seq| {
            *pending_sync_clone.borrow_mut() = false;
        })
        .error(|id, seq, res, msg| {
            error!("PipeWire core error: id={id} seq={seq} res={res} msg={msg}");
        })
        .register();

    // Drain initial sync and let registry events (ports, nodes) arrive.
    core.sync(0).context("core sync failed")?;
    while *pending_sync.borrow() {
        main_loop
            .loop_()
            .iterate(Timeout::Finite(Duration::from_millis(100)));
    }
    // Extra iterations to ensure all port/node globals are delivered.
    for _ in 0..5 {
        main_loop
            .loop_()
            .iterate(Timeout::Finite(Duration::from_millis(50)));
    }
    info!("initial sync complete");

    // ── link initially discovered nodes ─────────────────────────────────────
    link_pending(&tracker, &ports, &links, &core, &args.sink_name);

    // ── main loop ─────────────────────────────────────────────────────────────
    info!(
        "running — set '{}' as your audio output in your player / pavucontrol",
        args.sink_name
    );
    info!("press Ctrl-C to stop");

    // Run the main loop, polling for the quit flag periodically.
    while !quit.load(Ordering::SeqCst) {
        main_loop
            .loop_()
            .iterate(Timeout::Finite(Duration::from_millis(100)));
    }

    info!("pw-merger stopped — PipeWire objects released");
    Ok(())
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Create a persistent null audio sink (virtual output device).
fn create_null_sink(core: &pw::core::Core, name: &str, media_role: &str) -> Result<pw::node::Node> {
    let props = properties! {
        "factory.name"       => "support.null-audio-sink",
        "node.name"          => name,
        "node.description"   => name,
        "media.class"        => "Audio/Sink",
        "media.role"         => media_role,
        "monitor.channel-volumes" => "true",
        "audio.position"     => "FL,FR",
    };

    let node = core
        .create_object::<pw::node::Node>("adapter", &props)
        .context("create_object failed for null sink")?;

    debug!("null sink proxy id = {}", node.upcast_ref().id());
    Ok(node)
}

/// For every target node that is known but not yet linked, create port links.
fn link_pending(
    tracker: &NodeTracker,
    ports: &PortTracker,
    links: &Rc<RefCell<Vec<Link>>>,
    core: &pw::core::Core,
    null_sink_name: &str,
) {
    for node in tracker.unlinked_nodes() {
        info!(
            "linking null sink monitor → '{}' (id={})",
            node.name, node.id
        );
        match link_device(ports, links, core, null_sink_name, &node.name) {
            Ok(()) => {
                tracker.mark_linked(node.id);
                info!("links established for '{}'", node.name);
            }
            Err(e) => {
                warn!("failed to link '{}': {e}", node.name);
            }
        }
    }
}

/// Create direct port links from the null sink's monitor ports to a target
/// device's playback ports.
fn link_device(
    ports: &PortTracker,
    links: &Rc<RefCell<Vec<Link>>>,
    core: &pw::core::Core,
    null_sink_name: &str,
    target_name: &str,
) -> Result<()> {
    let monitors = ports.monitor_ports(null_sink_name);
    let playbacks = ports.playback_ports(target_name);

    if monitors.is_empty() {
        anyhow::bail!("no monitor ports found on '{null_sink_name}'");
    }
    if playbacks.is_empty() {
        anyhow::bail!("no playback ports found on '{target_name}'");
    }

    // Link each monitor port to the corresponding playback port by channel
    // position (FL→FL, FR→FR, etc.).
    for mon in &monitors {
        // Extract channel suffix from port name (e.g. "monitor_FL" → "FL").
        let mon_suffix = mon.name.split('_').next_back().unwrap_or("");
        // Find matching playback port.
        let playback = playbacks.iter().find(|p| {
            let pb_suffix = p.name.split('_').next_back().unwrap_or("");
            pb_suffix == mon_suffix
        });

        if let Some(pb) = playback {
            info!("  linking port {} → {} ({})", mon.id, pb.id, mon_suffix);
            let link = create_link(core, mon.id, pb.id)?;
            links.borrow_mut().push(link);
        } else {
            warn!(
                "  no matching playback port for monitor channel '{mon_suffix}' on '{target_name}'"
            );
        }
    }

    Ok(())
}

/// Create a single link between two ports using the link-factory.
fn create_link(core: &pw::core::Core, output_port: u32, input_port: u32) -> Result<Link> {
    let props = properties! {
        "link.output.port" => output_port.to_string(),
        "link.input.port"  => input_port.to_string(),
        "object.linger"    => "true",
    };

    let link = core
        .create_object::<Link>("link-factory", &props)
        .context("create_object failed for link")?;

    debug!("link created: id={}", link.upcast_ref().id());
    Ok(link)
}
