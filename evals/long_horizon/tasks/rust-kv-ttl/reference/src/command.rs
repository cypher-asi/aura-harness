use crate::clock::Clock;
use crate::store::Store;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    Get(String),
    Set(String, String),
    Del(String),
    Expire(String, u64),
    Ttl(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParseError {
    Empty,
    Unknown(String),
    /// Lowercase command name
    WrongArity(&'static str),
    NotInteger,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "ERR empty command"),
            Self::Unknown(name) => write!(f, "ERR unknown command '{name}'"),
            Self::WrongArity(name) => write!(f, "ERR wrong number of arguments for '{name}'"),
            Self::NotInteger => write!(f, "ERR value is not an integer or out of range"),
        }
    }
}

impl std::error::Error for ParseError {}

/// Parse one line such as `SET key value`. Command names are
/// case-insensitive; keys and values are single words.
pub fn parse(line: &str) -> Result<Command, ParseError> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let Some((name, args)) = words.split_first() else {
        return Err(ParseError::Empty);
    };
    match (name.to_ascii_uppercase().as_str(), args) {
        ("GET", [key]) => Ok(Command::Get((*key).to_string())),
        ("GET", _) => Err(ParseError::WrongArity("get")),
        ("SET", [key, value]) => Ok(Command::Set((*key).to_string(), (*value).to_string())),
        ("SET", _) => Err(ParseError::WrongArity("set")),
        ("DEL", [key]) => Ok(Command::Del((*key).to_string())),
        ("DEL", _) => Err(ParseError::WrongArity("del")),
        ("EXPIRE", [key, seconds]) => seconds
            .parse::<u64>()
            .map(|s| Command::Expire((*key).to_string(), s))
            .map_err(|_| ParseError::NotInteger),
        ("EXPIRE", _) => Err(ParseError::WrongArity("expire")),
        ("TTL", [key]) => Ok(Command::Ttl((*key).to_string())),
        ("TTL", _) => Err(ParseError::WrongArity("ttl")),
        _ => Err(ParseError::Unknown((*name).to_string())),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    Ok,
    Nil,
    Bulk(String),
    Integer(i64),
    Error(String),
}

impl fmt::Display for Response {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ok => write!(f, "OK"),
            Self::Nil => write!(f, "(nil)"),
            Self::Bulk(value) => write!(f, "\"{value}\""),
            Self::Integer(n) => write!(f, "(integer) {n}"),
            Self::Error(message) => write!(f, "(error) {message}"),
        }
    }
}

pub fn execute<C: Clock>(store: &mut Store<C>, command: Command) -> Response {
    match command {
        Command::Get(key) => store
            .get(&key)
            .map_or(Response::Nil, |v| Response::Bulk(v.to_string())),
        Command::Set(key, value) => {
            store.set(key, value);
            Response::Ok
        }
        Command::Del(key) => Response::Integer(i64::from(store.del(&key))),
        Command::Expire(key, seconds) => Response::Integer(i64::from(store.expire(&key, seconds))),
        Command::Ttl(key) => Response::Integer(store.ttl(&key)),
    }
}
