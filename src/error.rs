use std::fmt;

/// Errors specific to pw-merger that give the user actionable messages.
#[derive(Debug)]
#[allow(dead_code)]
pub enum MergerError {
    /// A device name was given but never appeared in the registry.
    DeviceNotFound(String),
    /// `PipeWire` reported an error code while creating an object.
    PipeWire(i32, &'static str),
}

impl fmt::Display for MergerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MergerError::DeviceNotFound(name) => write!(
                f,
                "device not found: '{name}'\n\
                 Hint: list available devices with:\n\
                 \x20 pw-cli list-objects Node | grep node.name"
            ),
            MergerError::PipeWire(code, ctx) => {
                write!(f, "PipeWire error {code} while {ctx}")
            }
        }
    }
}

impl std::error::Error for MergerError {}
