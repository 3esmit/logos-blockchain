//! Errors and results specific to Steelsafe.

use std::{
    error::Error as StdError,
    fmt::{self, Debug, Display, Formatter},
    io::Error as IoError,
    str::Utf8Error,
};

use arboard::Error as ClipboardError;
use argon2::Error as Argon2Error;
use block_padding::UnpadError;
use chacha20poly1305::Error as XChaCha20Poly1305Error;
use crypto_common::InvalidLength;
use logos_blockchain_zone_sdk::indexer::Error as ZoneIndexerError;
use nanosql::{Error as SqlError, rusqlite::Error as RusqliteError};
use serde_json::Error as JsonError;
use thiserror::Error;

use crate::error::Error::Context;

#[derive(Error)]
pub enum Error {
    #[error("Can't re-open screen guard while one is already open")]
    ScreenAlreadyOpen,

    #[error("Can't find database directory")]
    MissingDatabaseDir,

    #[error("Label is required and must be a single line")]
    LabelRequired,

    #[error("Secret is required")]
    SecretRequired,

    #[error("Encryption (master) password is required and must be a single line")]
    EncryptionPasswordRequired,

    #[error("Passwords do not match")]
    ConfirmPasswordMismatch,

    #[error("Account name must be a single line if specified")]
    AccountNameSingleLine,

    #[error("No item is currently selected")]
    SelectionRequired,

    #[error("I/O error: {0}")]
    Io(#[from] IoError),

    #[error("Secret is not valid UTF-8: {0}")]
    Utf8(#[from] Utf8Error),

    #[error("JSON error: {0}")]
    Json(#[from] JsonError),

    #[error("Database error: {0}")]
    Db(#[from] SqlError),

    #[error("Rusqlite Database error: {0}")]
    Sqlite(#[from] RusqliteError),

    #[error("Database schema version too high: need <= {expected}, got {actual}")]
    SchemaVersionMismatch { expected: i64, actual: i64 },

    #[error("Password hashing error: {0}")]
    Argon2(#[from] Argon2Error),

    #[error("Encryption, decryption, or authentication error")]
    XChaCha20Poly1305(#[from] XChaCha20Poly1305Error),

    #[error("Invalid padding in decrypted secret")]
    Unpad(#[from] UnpadError),

    #[error(transparent)]
    InvalidLength(#[from] InvalidLength),

    #[error(transparent)]
    Clipboard(#[from] ClipboardError),

    #[error("{0}")]
    InvalidChannelId(String),

    #[error("URL parse error: {0}")]
    Url(String),

    #[error(transparent)]
    ZoneIndexer(#[from] ZoneIndexerError),

    #[error("{message}: {source}")]
    Context {
        message: String,
        #[source]
        source: Box<dyn StdError + Send + Sync + 'static>,
    },
}

impl Error {
    pub fn context<E, M>(source: E, message: M) -> Self
    where
        E: StdError + Send + Sync + 'static,
        M: Into<String>,
    {
        Context {
            message: message.into(),
            source: Box::new(source),
        }
    }
}

impl Debug for Error {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        Display::fmt(self, formatter)
    }
}

pub type Result<T, E = Error> = core::result::Result<T, E>;

pub trait ResultExt<T> {
    fn context<M>(self, message: M) -> Result<T>
    where
        M: Into<String>;
}

impl<T, E> ResultExt<T> for Result<T, E>
where
    E: StdError + Send + Sync + 'static,
{
    fn context<M>(self, message: M) -> Result<T>
    where
        M: Into<String>,
    {
        self.map_err(|error| Error::context(error, message))
    }
}
