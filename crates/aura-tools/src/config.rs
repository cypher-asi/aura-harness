//! TOML configuration parser for installed tools.
//!
//! Parses a `tools.toml` file into `Vec<InstalledToolDefinition>`,
//! interpolating environment variables (`${VAR}`) in string values.

use aura_core::InstalledToolDefinition;
use serde::Deserialize;
use std::path::Path;
use tracing::warn;

/// Error type for tool configuration loading.
#[derive(Debug, thiserror::Error)]
pub enum ToolConfigError {
    #[error("failed to read config file: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to parse TOML: {0}")]
    Parse(#[from] toml::de::Error),
}

/// Top-level structure of tools.toml.
#[derive(Debug, Deserialize)]
struct ToolsConfig {
    #[serde(default)]
    tool: Vec<InstalledToolDefinition>,
}

/// Load tool definitions from a TOML file.
///
/// Environment variables in the format `${VAR}` are interpolated in the
/// raw TOML text before parsing. Missing env vars are replaced with empty strings.
///
/// # Errors
/// Returns `ToolConfigError` if the file cannot be read or parsed.
pub fn load_tools_from_file(path: &Path) -> Result<Vec<InstalledToolDefinition>, ToolConfigError> {
    let raw = std::fs::read_to_string(path)?;
    let interpolated = interpolate_env_vars(&raw);
    let config: ToolsConfig = toml::from_str(&interpolated)?;
    Ok(config.tool)
}

/// Replace `${VAR}` patterns with their environment variable values.
/// Missing variables are replaced with empty strings and a warning is logged.
fn interpolate_env_vars(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '$' && chars.peek() == Some(&'{') {
            chars.next(); // consume '{'
            let mut var_name = String::new();
            for c in chars.by_ref() {
                if c == '}' {
                    break;
                }
                var_name.push(c);
            }
            match std::env::var(&var_name) {
                Ok(val) => result.push_str(&val),
                Err(_) => {
                    warn!(var = %var_name, "Environment variable not set, using empty string");
                }
            }
        } else {
            result.push(ch);
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_interpolate_env_vars_basic() {
        std::env::set_var("TEST_TOOLS_PORT", "9090");
        let input = "http://127.0.0.1:${TEST_TOOLS_PORT}/api";
        let result = interpolate_env_vars(input);
        assert_eq!(result, "http://127.0.0.1:9090/api");
        std::env::remove_var("TEST_TOOLS_PORT");
    }

    #[test]
    fn test_interpolate_env_vars_missing() {
        std::env::remove_var("NONEXISTENT_VAR_FOR_TEST");
        let input = "prefix_${NONEXISTENT_VAR_FOR_TEST}_suffix";
        let result = interpolate_env_vars(input);
        assert_eq!(result, "prefix__suffix");
    }

    #[test]
    fn test_interpolate_no_vars() {
        let input = "just plain text";
        let result = interpolate_env_vars(input);
        assert_eq!(result, "just plain text");
    }

    #[test]
    fn test_load_tools_from_file() {
        let toml_content = r#"
[[tool]]
name = "test_tool"
description = "A test tool"
endpoint = "http://localhost:8080/test"

[tool.input_schema]
type = "object"
required = ["query"]
[tool.input_schema.properties.query]
type = "string"
description = "Search query"

[[tool]]
name = "another_tool"
description = "Another tool"
endpoint = "http://localhost:8080/another"
timeout_ms = 60000
namespace = "test"

[tool.input_schema]
type = "object"
"#;

        let mut file = NamedTempFile::new().unwrap();
        file.write_all(toml_content.as_bytes()).unwrap();

        let tools = load_tools_from_file(file.path()).unwrap();
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "test_tool");
        assert_eq!(tools[0].endpoint, "http://localhost:8080/test");
        assert_eq!(tools[1].name, "another_tool");
        assert_eq!(tools[1].timeout_ms, Some(60000));
        assert_eq!(tools[1].namespace.as_deref(), Some("test"));
    }

    #[test]
    fn test_load_tools_with_env_interpolation() {
        std::env::set_var("TEST_BIND_PORT_CFG", "3333");

        let toml_content = r#"
[[tool]]
name = "env_tool"
description = "Uses env var"
endpoint = "http://127.0.0.1:${TEST_BIND_PORT_CFG}/api/test"

[tool.input_schema]
type = "object"
"#;

        let mut file = NamedTempFile::new().unwrap();
        file.write_all(toml_content.as_bytes()).unwrap();

        let tools = load_tools_from_file(file.path()).unwrap();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].endpoint, "http://127.0.0.1:3333/api/test");

        std::env::remove_var("TEST_BIND_PORT_CFG");
    }

    #[test]
    fn test_load_empty_file() {
        let mut file = NamedTempFile::new().unwrap();
        file.write_all(b"").unwrap();

        let tools = load_tools_from_file(file.path()).unwrap();
        assert!(tools.is_empty());
    }

    #[test]
    fn test_load_nonexistent_file() {
        let result = load_tools_from_file(Path::new("/nonexistent/tools.toml"));
        assert!(result.is_err());
    }
}
