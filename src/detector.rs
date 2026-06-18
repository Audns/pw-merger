/// detector.rs — Discover available PipeWire audio sinks.
///
/// Scans the PipeWire registry for nodes with `media.class = "Audio/Sink"`
/// and returns them sorted by name.  Used by the `--list` flag and `--auto`
/// mode.
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use log::debug;

use pipewire::context::ContextBox;
use pipewire::loop_::Timeout;
use pipewire::main_loop::MainLoopRc;
use pipewire::registry::GlobalObject;
use pipewire::spa::utils::dict::DictRef;
use pipewire::types::ObjectType;

/// A discovered audio sink device.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SinkDevice {
    /// PipeWire global ID.
    pub id: u32,
    /// `node.name` — unique identifier used in `--device-a` / `--device-b`.
    pub name: String,
    /// Human-readable description from `node.description`.
    pub description: String,
    /// The audio card / driver name if available (e.g. "alsa_card.pci-...").
    pub driver: String,
}

impl SinkDevice {
    /// Format for display in a table.
    #[allow(dead_code)]
    pub fn display_name(&self) -> String {
        if self.description.is_empty() || self.description == self.name {
            self.name.clone()
        } else {
            format!("{} ({})", self.description, self.name)
        }
    }
}

/// Scan PipeWire for all audio sink nodes.
///
/// Connects to PipeWire, waits briefly for the registry to populate, then
/// returns the discovered sinks sorted by name.
pub fn discover_sinks() -> Result<Vec<SinkDevice>> {
    let main_loop = MainLoopRc::new(None).context("failed to create PipeWire main loop")?;
    let context =
        ContextBox::new(main_loop.loop_(), None).context("failed to create PipeWire context")?;
    let core = context
        .connect(None)
        .context("failed to connect to PipeWire daemon — is it running?")?;
    let registry = core
        .get_registry()
        .context("failed to get PipeWire registry")?;

    let sinks: Rc<RefCell<HashMap<u32, SinkDevice>>> = Rc::new(RefCell::new(HashMap::new()));

    let sinks_clone = Rc::clone(&sinks);
    let _listener = registry
        .add_listener_local()
        .global(move |global: &GlobalObject<&DictRef>| {
            if let Some(sink) = parse_sink(global) {
                debug!("discovered sink: '{}' (id={})", sink.name, sink.id);
                sinks_clone.borrow_mut().insert(sink.id, sink);
            }
        })
        .register();

    // Sync and wait for the initial burst of registry events.
    core.sync(0).context("core sync failed")?;

    // Give PipeWire a moment to deliver all globals (typically < 50ms).
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        main_loop
            .loop_()
            .iterate(Timeout::Finite(Duration::from_millis(50)));
    }

    let mut result: Vec<SinkDevice> = sinks.borrow().values().cloned().collect();
    result.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(result)
}

/// Try to extract a `SinkDevice` from a PipeWire global object.
///
/// Returns `Some` only for audio sink nodes (excludes monitors/sources).
fn parse_sink(global: &GlobalObject<&DictRef>) -> Option<SinkDevice> {
    if global.type_ != ObjectType::Node {
        return None;
    }
    let props = global.props?;
    let media_class = props.get("media.class")?;
    if media_class != "Audio/Sink" {
        return None;
    }
    let name = props.get("node.name")?.to_owned();
    // Skip our own virtual sinks to avoid loops.
    if name.starts_with("pw-merger") {
        return None;
    }

    let description = props.get("node.description").unwrap_or("").to_owned();
    let driver = props
        .get("device.bus-path")
        .or_else(|| props.get("device.name"))
        .unwrap_or("")
        .to_owned();

    Some(SinkDevice {
        id: global.id,
        name,
        description,
        driver,
    })
}

/// Print the list of sinks to stdout in a human-readable table.
pub fn print_sink_list(sinks: &[SinkDevice]) {
    if sinks.is_empty() {
        println!("No audio sinks found.  Is PipeWire running?");
        return;
    }

    println!("Available audio sinks:");
    println!();
    println!("  {:<4}  {:<40}  DESCRIPTION", "ID", "NAME");
    println!("  {}", "-".repeat(86));
    for sink in sinks {
        println!(
            "  {:<4}  {:<40}  {}",
            sink.id,
            sink.name,
            if sink.description.is_empty() {
                "-"
            } else {
                &sink.description
            }
        );
    }
    println!();
    println!("Usage:");
    println!("  pw-merger <ID_A> <ID_B>                  # merge two sinks");
    println!("  pw-merger <ID_A> <ID_B> <ID_C>           # merge three sinks");
    println!("  pw-merger -o \"My Speakers\" 55 61        # with custom name");
}
