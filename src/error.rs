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
    /// The Discord client returned an error.
    ///
    /// Boxed because `serenity::Error` is large; keeping it inline would bloat
    /// every `Result<_, HoneyPotError>` (see clippy's `result_large_err`).
    #[error("Discord client error: {0}")]
    Client(#[source] Box<serenity::Error>),
}

impl From<serenity::Error> for HoneyPotError {
    fn from(err: serenity::Error) -> Self {
        HoneyPotError::Client(Box::new(err))
    }
}
