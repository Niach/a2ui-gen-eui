use thiserror::Error;

/// What can go wrong.
#[derive(Debug, Clone, Error)]
pub enum Error {
    /// The catalog schema cannot be used.
    #[error("catalog: {0}")]
    Catalog(String),
    /// The grammar cannot be built.
    #[error("grammar: {0}")]
    Grammar(String),
    /// The model cannot be loaded.
    #[error("model: {0}")]
    Model(String),
    /// The model file is not the pinned one.
    #[error("sha256 mismatch for {path}: expected {expected}, got {actual}")]
    Sha256 {
        path: String,
        expected: String,
        actual: String,
    },
    /// Download or file error.
    #[error("{0}")]
    Io(String),
    /// Request and reply do not fit the context.
    #[error(
        "the prompt has {prompt} tokens; with {reply} for the reply it does not fit the context of {context}"
    )]
    ContextFull {
        prompt: usize,
        reply: usize,
        context: usize,
    },
    /// llama.cpp failed.
    #[error("inference: {0}")]
    Inference(String),
    /// The reply is not a valid surface.
    #[error("invalid reply: {0}")]
    Invalid(String),
    /// Every attempt was invalid.
    #[error("no valid reply in {attempts} attempts, last error: {message}")]
    NoValidReply {
        attempts: u32,
        message: String,
        /// The last reply, for diagnosis.
        reply: String,
        #[cfg(feature = "llama")]
        stats: Box<crate::Stats>,
    },
    /// The generation was cancelled.
    #[error("cancelled")]
    Cancelled,
}

/// Result with this crate's [`enum@Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}
