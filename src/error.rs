use std::{error::Error as ErrorTrait, fmt, io};

use kuska_ssb::{api, crypto, discovery, feed, handshake, rpc};
use serde_json;
use toml::{de, ser};

use crate::storage::kv;

/// Possible solar errors.
#[derive(Debug)]
pub enum Error {
    /// xdg::BaseDirectoriesError.
    BaseDirectories(xdg::BaseDirectoriesError),
    /// Failed to deserialization TOML.
    DeserializeToml(de::Error),
    /// io::Error.
    IO(io::Error),
    /// SSB API error.
    KuskaApi(api::Error),
    /// SSB cryptograpy error.
    KuskaCrypto(crypto::Error),
    /// LAN UDP discovery error.
    KuskaDiscovery(discovery::Error),
    /// SSB feed error.
    KuskaFeed(feed::Error),
    /// Secret handshake error.
    KuskaHandshake(handshake::async_std::Error),
    /// SSB RPC error.
    KuskaRpc(rpc::Error),
    /// Key-value database error.
    KV(kv::Error),
    /// Serde JSON error.
    SerdeJson(serde_json::Error),
    /// Failed to serialization TOML.
    SerializeToml(ser::Error),
    /// Unknown error.
    Other(String),
}

impl ErrorTrait for Error {
    fn source(&self) -> Option<&(dyn ErrorTrait + 'static)> {
        match self {
            Error::IO(source) => Some(source),
            _ => None,
        }
    }
}

impl From<xdg::BaseDirectoriesError> for Error {
    fn from(err: xdg::BaseDirectoriesError) -> Error {
        Error::BaseDirectories(err)
    }
}

impl From<de::Error> for Error {
    fn from(err: de::Error) -> Error {
        Error::DeserializeToml(err)
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Error {
        Error::IO(err)
    }
}

impl From<api::Error> for Error {
    fn from(err: api::Error) -> Error {
        Error::KuskaApi(err)
    }
}

impl From<crypto::Error> for Error {
    fn from(err: crypto::Error) -> Error {
        Error::KuskaCrypto(err)
    }
}

impl From<discovery::Error> for Error {
    fn from(err: discovery::Error) -> Error {
        Error::KuskaDiscovery(err)
    }
}

impl From<feed::Error> for Error {
    fn from(err: feed::Error) -> Error {
        Error::KuskaFeed(err)
    }
}

impl From<handshake::async_std::Error> for Error {
    fn from(err: handshake::async_std::Error) -> Error {
        Error::KuskaHandshake(err)
    }
}

impl From<rpc::Error> for Error {
    fn from(err: rpc::Error) -> Error {
        Error::KuskaRpc(err)
    }
}

impl From<kv::Error> for Error {
    fn from(err: kv::Error) -> Error {
        Error::KV(err)
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Error {
        Error::SerdeJson(err)
    }
}

impl From<ser::Error> for Error {
    fn from(err: ser::Error) -> Error {
        Error::SerializeToml(err)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Error::BaseDirectories(err) => format!("{}", err),
                Error::DeserializeToml(err) => format!("{}", err),
                Error::IO(err) => format!("{}", err),
                Error::KuskaApi(err) => format!("{}", err),
                Error::KuskaCrypto(err) => format!("{}", err),
                Error::KuskaDiscovery(err) => format!("{}", err),
                Error::KuskaFeed(err) => format!("{}", err),
                Error::KuskaHandshake(err) => format!("{}", err),
                Error::KuskaRpc(err) => format!("{}", err),
                Error::KV(err) => format!("{}", err),
                Error::SerdeJson(err) => format!("{}", err),
                Error::SerializeToml(err) => format!("{}", err),
                Error::Other(reason) => reason.to_string(),
            }
        )
    }
}

//impl PartialEq for Error {
