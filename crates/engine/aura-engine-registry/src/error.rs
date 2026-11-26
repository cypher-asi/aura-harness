//! Errors from the [`crate::registry`] and the [`crate::definitions`].

use aura_core_types::AgentId;
use aura_store_db::{AgentStoreError, StoreError};
use std::path::PathBuf;

/// Errors from [`AgentRegistry`](crate::AgentRegistry) changes and loads.
#[derive(Debug, thiserror::Error)]
pub enum AgentRegistryError {
    /// `agent_id` is not in the registry.
    #[error("no such agent")]
    NoSuchAgent { agent_id: AgentId },

    /// Archiving `agent_id` would leave no agent listed.
    #[error("it is the last agent in the list; create another project first")]
    LastAgent { agent_id: AgentId },

    /// `project_id` is not in the registry.
    #[error("no such project")]
    NoSuchProject { project_id: String },

    /// Removing `project_id` would leave no agent listed.
    #[error("it is the last project in the list; create another one first")]
    LastProject { project_id: String },

    /// An agent to place or promote is not in the registry.
    #[error("{} is not registered", agent_id.to_hex())]
    NotRegistered { agent_id: AgentId },

    /// `folder`, opened as a project, does not resolve.
    #[error("opening {}", folder.display())]
    OpenFolder {
        folder: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// `folder`, opened as a project, is not a directory.
    #[error("{} is not a folder", folder.display())]
    NotAFolder { folder: PathBuf },

    /// The process's working directory, the default launch directory,
    /// could not be read.
    #[error("resolving working directory")]
    WorkingDir {
        #[source]
        source: std::io::Error,
    },

    /// The agent-state database at `path` could not be opened.
    #[error("opening agent registry database {}", path.display())]
    OpenDatabase {
        path: PathBuf,
        #[source]
        source: StoreError,
    },

    /// Checking whether the store holds a registry failed.
    #[error("reading agent registry in {}", data_dir.display())]
    Read {
        data_dir: PathBuf,
        #[source]
        source: AgentStoreError,
    },

    /// Loading the stored registry failed.
    #[error("loading agent registry in {}", data_dir.display())]
    Load {
        data_dir: PathBuf,
        #[source]
        source: AgentStoreError,
    },

    /// Writing the changed rows failed; `agents` and `projects` count
    /// the rows the batch put or deleted.
    #[error(
        "saving agent registry in {} ({agents} agents, {projects} projects changed)",
        data_dir.display()
    )]
    Save {
        data_dir: PathBuf,
        agents: usize,
        projects: usize,
        #[source]
        source: AgentStoreError,
    },

    /// A legacy registry or identity file could not be read.
    #[error("reading {}", path.display())]
    ReadFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The legacy registry file is not valid JSON.
    #[error("parsing {}", path.display())]
    ParseFile {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    /// A new identity file could not be written.
    #[error("writing {}", path.display())]
    WriteFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// Errors from parsing, loading, creating and deleting permanent agent
/// definitions ([`AgentDefinitions`](crate::AgentDefinitions)).
#[derive(Debug, thiserror::Error)]
pub enum AgentDefinitionError {
    /// The file does not start with a `---` frontmatter block.
    #[error("missing `---` frontmatter at the top of the file")]
    MissingFrontmatter,

    /// The frontmatter block has no closing `---`.
    #[error("frontmatter is not closed with `---`")]
    UnclosedFrontmatter,

    /// The frontmatter is not the expected YAML.
    #[error("invalid frontmatter")]
    InvalidFrontmatter {
        #[source]
        source: serde_yaml::Error,
    },

    /// The name is empty.
    #[error("name must not be empty")]
    EmptyName,

    /// `name` is longer than `max` characters.
    #[error("name must be at most {max} characters")]
    NameTooLong { name: String, max: usize },

    /// `name` has characters a definition name cannot have.
    #[error("name `{name}` must use lowercase letters, digits, `-` or `_`")]
    InvalidName { name: String },

    /// A definition named `name` exists already.
    #[error("a permanent agent named `{name}` already exists")]
    AlreadyExists { name: String },

    /// No definition is named `name`.
    #[error("no permanent agent named `{name}`")]
    NotFound { name: String },

    /// A definition file is larger than `max` bytes.
    #[error("file is larger than {max} bytes")]
    TooLarge { max: u64 },

    /// A definition file could not be read.
    #[error(transparent)]
    Read(#[from] std::io::Error),

    /// The definitions directory `dir` could not be created.
    #[error("creating {}", dir.display())]
    CreateDir {
        dir: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The file at `path` could not be written.
    #[error("writing {}", path.display())]
    WriteFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// The file at `path` could not be removed.
    #[error("removing {}", path.display())]
    RemoveFile {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

/// `error` followed by each of its sources, `": "`-separated, as
/// `anyhow` prints an error with `{:#}`.
pub(crate) fn chain(error: &dyn std::error::Error) -> String {
    let mut text = error.to_string();
    let mut source = error.source();
    while let Some(cause) = source {
        text.push_str(": ");
        text.push_str(&cause.to_string());
        source = cause.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_lists_each_source_like_anyhow_alternate() {
        let error = AgentRegistryError::ReadFile {
            path: PathBuf::from("/data/agents.json"),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "gone"),
        };
        assert_eq!(error.to_string(), "reading /data/agents.json");
        assert_eq!(chain(&error), "reading /data/agents.json: gone");
    }

    #[test]
    fn name_messages_keep_their_limits() {
        let too_long = AgentDefinitionError::NameTooLong {
            name: "x".repeat(41),
            max: 40,
        };
        assert_eq!(too_long.to_string(), "name must be at most 40 characters");
        let too_large = AgentDefinitionError::TooLarge { max: 65_536 };
        assert_eq!(too_large.to_string(), "file is larger than 65536 bytes");
    }
}
