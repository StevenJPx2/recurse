use std::fmt;

use recurse_protocol::{ErrorCode, RpcError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    /// The daemon answered with a protocol error.
    Rpc(RpcError),
    /// The daemon could not be reached or the connection failed.
    Transport(String),
    /// The daemon answered with something this client does not understand.
    Protocol(String),
}

impl ClientError {
    pub fn code(&self) -> Option<ErrorCode> {
        match self {
            Self::Rpc(error) => Some(error.code),
            Self::Transport(_) | Self::Protocol(_) => None,
        }
    }
}

impl fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Rpc(error) => write!(formatter, "{error}"),
            Self::Transport(message) | Self::Protocol(message) => formatter.write_str(message),
        }
    }
}

impl std::error::Error for ClientError {}
