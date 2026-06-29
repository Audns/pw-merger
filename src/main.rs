mod detector;
mod error;
mod merger;
mod registry;

use anyhow::{Result, bail};
use clap::Parser;
use log::info;

/// Merge two `PipeWire` audio sinks into one persistent virtual output.
///
/// Creates a virtual sink that plays to both target devices simultaneously.
///
/// Examples
/// --------
///   pw-merger --list                                         # show available sinks
///   pw-merger 55 61                                          # merge two by ID
///   pw-merger -o "Speakers" 55 61                            # custom name
///   pw-merger 55 61 73                                       # merge three sinks
///   pw-merger alsa_output.pci-0000_08_00.1.hdmi-stereo \
///             alsa_output.pci-0000_0a_00.4.iec958-stereo    # merge by name
#[derive(Parser, Debug)]
#[command(author, version, about, verbatim_doc_comment)]
pub struct Args {
    /// List available audio sinks and exit.
    #[arg(short, long)]
    pub list: bool,

    /// Name for the merged sink (shown in pavucontrol / audio settings).
    #[arg(short = 'o', long = "output", default_value = "Merged Output")]
    pub sink_name: String,

    /// Sink IDs or node names to merge (2 or more required).
    /// Run `pw-merger --list` to see available IDs.
    #[arg(required_unless_present = "list", num_args = 2..)]
    pub devices: Vec<String>,

    /// Media role applied to the virtual sink (Music, Movie, Game …).
    #[arg(long, default_value = "Music")]
    pub media_role: String,

    /// Verbose logging (set `RUST_LOG=debug` for even more detail).
    #[arg(short, long)]
    pub verbose: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Initialise logging.  RUST_LOG overrides --verbose.
    if std::env::var("RUST_LOG").is_err() {
        let level = if args.verbose { "debug" } else { "info" };
        // SAFETY: called before any threads are spawned
        unsafe { std::env::set_var("RUST_LOG", format!("pw_merger={level}")) };
    }
    env_logger::init();

    // ── --list: discover and display sinks, then exit ──────────────────────
    if args.list {
        let sinks = detector::discover_sinks()?;
        detector::print_sink_list(&sinks);
        return Ok(());
    }

    // ── Resolve device names ───────────────────────────────────────────────
    if args.devices.len() < 2 {
        bail!(
            "need at least 2 sink IDs, got {}.\n\
             Run `pw-merger --list` to see available sinks.",
            args.devices.len()
        );
    }
    let sinks = detector::discover_sinks()?;
    let devices: &[String] = &args
        .devices
        .iter()
        .map(|d| resolve_device(d, &sinks).unwrap_or_default())
        .collect::<Vec<_>>();

    info!("pw-merger starting");
    info!("  sink name : {}", args.sink_name);
    for (i, name) in devices.iter().enumerate() {
        info!(
            "  device {}  : {}",
            (b'A' + u8::try_from(i).unwrap_or_default()) as char,
            name
        );
    }

    merger::run(&args, devices)
}

/// Resolve a device identifier to a node.name.
///
/// If `id_str` is purely numeric, treat it as a `PipeWire` global ID and
/// look up the corresponding sink.  Otherwise treat it as a node.name
/// directly.
fn resolve_device(id_str: &str, sinks: &[detector::SinkDevice]) -> Result<String> {
    if let Ok(id) = id_str.parse::<u32>() {
        sinks
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.name.clone())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "no audio sink with ID {id}.\n\
                     Run `pw-merger --list` to see available sinks."
                )
            })
    } else {
        Ok(id_str.to_owned())
    }
}
