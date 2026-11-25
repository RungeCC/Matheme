//! Structured errors shared by the link wrapper and kernel sessions.

use std::fmt;

/// Structured error returned by [`crate::WstpLink`] operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WstpError {
    /// Low-level WSTP transport or encoding error (preserving WSTP error code
    /// when available).
    Transport { code: Option<i32>, message: String },
    /// Unexpected WSTP packet or malformed envelope.
    Protocol(String),
    /// Exceeded the configured maximum packet count before receiving a return packet.
    PacketLimit { max_packets: usize },
    /// Timed out waiting for data from the remote kernel or link peer.
    Timeout(String),
    /// Link/session was left desynchronized and must be reconnected before reuse.
    Poisoned(String),
}

impl WstpError {
    pub fn transport(message: impl Into<String>) -> Self {
        Self::Transport {
            code: None,
            message: message.into(),
        }
    }

    pub fn protocol(message: impl Into<String>) -> Self {
        Self::Protocol(message.into())
    }

    /// Returns `true` when the link cannot be safely reused without reconnecting.
    pub fn is_fatal_to_link(&self) -> bool {
        matches!(self, Self::Poisoned(_) | Self::Transport { .. })
    }
}

impl From<wstp::Error> for WstpError {
    fn from(err: wstp::Error) -> Self {
        Self::Transport {
            code: err.code(),
            message: err.to_string(),
        }
    }
}

impl fmt::Display for WstpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport {
                code: Some(code),
                message,
            } => write!(f, "WSTP transport error ({code}): {message}"),
            Self::Transport {
                code: None,
                message,
            } => write!(f, "WSTP transport error: {message}"),
            Self::Protocol(msg) => write!(f, "WSTP protocol error: {msg}"),
            Self::PacketLimit { max_packets } => {
                write!(
                    f,
                    "exceeded WSTP packet limit ({max_packets}) before ReturnPacket"
                )
            }
            Self::Timeout(msg) => write!(f, "WSTP timeout: {msg}"),
            Self::Poisoned(msg) => write!(f, "WSTP session poisoned: {msg}"),
        }
    }
}

impl std::error::Error for WstpError {}
