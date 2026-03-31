use thiserror::Error;

/// Errors from automaton lifecycle operations (install, tick, stop) and runtime management.
#[derive(Debug, Error)]
pub enum AutomatonError {
    #[error("automaton not found: {0}")]
    NotFound(String),

    #[error("automaton already running: {0}")]
    AlreadyRunning(String),

    #[error("automaton stopped: {0}")]
    Stopped(String),

    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    #[error("domain API error: {0}")]
    DomainApi(String),

    #[error("agent execution error: {0}")]
    AgentExecution(String),

    #[error("credits exhausted")]
    CreditsExhausted,

    #[error(transparent)]
    Internal(#[from] anyhow::Error),
}
