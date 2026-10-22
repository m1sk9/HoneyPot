//! Error types for HoneyPot.
//!
//! All fallible operations in the bot funnel through [`HoneyPotError`], which is
//! also the return type of `main`.

/// Errors that can occur while running HoneyPot.
#[derive(thiserror::Error, Debug)]
pub enum HoneyPotError {
    /// Failed to read the configuration file from disk.
    #[error("Failed to read configuration file: {0}")]
    ConfigRead(#[from] std::io::Error),
    /// Failed to parse the configuration file contents.
    #[error("Failed to parse configuration file: {0}")]
    ConfigParse(#[from] toml::de::Error),
    /// The global configuration has already been initialized.
    #[error("Configuration has already been initialized.")]
    AlreadyInitialized,
    /// A required environment variable is missing.
    #[error("Missing environment variable: {0}")]
    MissingEnv(String),
    /// An environment variable held a value that could not be parsed.
    ///
    /// Only produced by the preview mode (see `discord::preview`), so it is
    /// gated on that feature to stay out of the production build.
    #[cfg(feature = "preview")]
    #[error("Invalid value for environment variable: {0}")]
    InvalidEnv(String),
    /// Failed to load the `.env` file (parse or I/O error; a missing file is
    /// not treated as an error).
    #[error("Failed to load .env file: {0}")]
    Dotenv(#[from] dotenvy::Error),
    /// The Discord HTTP client returned an error.
    ///
    /// Boxed because `twilight_http::Error` is large; keeping it inline would
    /// bloat every `Result<_, HoneyPotError>` (see clippy's `result_large_err`).
    #[error("Discord client error: {0}")]
    Client(#[source] Box<twilight_http::Error>),
    /// A Discord HTTP response body could not be deserialized into its model.
    #[error("Failed to decode Discord response: {0}")]
    Decode(#[from] twilight_http::response::DeserializeBodyError),
    /// The gateway closed the connection with a non-resumable close code (e.g.
    /// an invalid token or disallowed privileged intents), so the shard stopped.
    #[error("Gateway connection closed fatally; check the bot token and privileged intents")]
    GatewayClosed,
}

impl From<twilight_http::Error> for HoneyPotError {
    fn from(err: twilight_http::Error) -> Self {
        HoneyPotError::Client(Box::new(err))
    }
}
