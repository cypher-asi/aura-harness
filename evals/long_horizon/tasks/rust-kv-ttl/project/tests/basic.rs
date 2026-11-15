use kv::{execute, parse, Command, ParseError, Response, Store};

#[test]
fn set_get_del() {
    let mut store = Store::new();
    assert_eq!(execute(&mut store, parse("SET a 1").unwrap()), Response::Ok);
    assert_eq!(
        execute(&mut store, parse("get a").unwrap()).to_string(),
        "\"1\""
    );
    assert_eq!(
        execute(&mut store, parse("DEL a").unwrap()),
        Response::Integer(1)
    );
    assert_eq!(execute(&mut store, parse("GET a").unwrap()), Response::Nil);
    assert!(store.is_empty());
}

#[test]
fn parse_errors() {
    assert_eq!(parse("  "), Err(ParseError::Empty));
    assert_eq!(
        parse("SET a").unwrap_err().to_string(),
        "ERR wrong number of arguments for 'set'"
    );
    assert_eq!(
        parse("FOO x").unwrap_err().to_string(),
        "ERR unknown command 'FOO'"
    );
    assert_eq!(parse("del k"), Ok(Command::Del("k".to_string())));
}
