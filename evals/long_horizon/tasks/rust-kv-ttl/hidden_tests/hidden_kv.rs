use kv::{execute, parse, Clock, ManualClock, Response, Session, Store, SystemClock};

fn store_at(ms: u64) -> (Store<ManualClock>, ManualClock) {
    let clock = ManualClock::new(ms);
    (Store::with_clock(clock.clone()), clock)
}

#[test]
fn manual_clock_clones_share_time() {
    let clock = ManualClock::new(5);
    let other = clock.clone();
    clock.advance(10);
    assert_eq!(other.now_ms(), 15);
    other.set(100);
    assert_eq!(clock.now_ms(), 100);
    assert!(SystemClock.now_ms() > 1_600_000_000_000);
}

#[test]
fn keys_expire() {
    let (mut store, clock) = store_at(1_000);
    store.set("a", "1");
    store.set("b", "2");
    assert!(!store.expire("missing", 5));
    assert!(store.expire("a", 10));
    assert_eq!(store.ttl("a"), 10);
    assert_eq!(store.ttl("b"), -1);
    assert_eq!(store.ttl("missing"), -2);

    clock.advance(1);
    assert_eq!(store.ttl("a"), 10, "rounded up");
    clock.advance(8_999);
    assert_eq!(store.ttl("a"), 1);
    assert_eq!(store.get("a"), Some("1"));
    clock.advance(1_000);
    assert_eq!(store.get("a"), None);
    assert_eq!(store.ttl("a"), -2);
    assert_eq!(store.len(), 1);
    assert!(!store.del("a"));
    assert!(!store.expire("a", 5));
}

#[test]
fn set_clears_expiry_and_expired_keys_can_be_set_again() {
    let (mut store, clock) = store_at(0);
    store.set("a", "1");
    store.expire("a", 1);
    store.set("a", "2");
    assert_eq!(store.ttl("a"), -1);
    store.expire("a", 1);
    clock.advance(1_000);
    assert!(store.is_empty());
    store.set("a", "3");
    assert_eq!(store.get("a"), Some("3"));
    assert_eq!(store.ttl("a"), -1);
}

#[test]
fn expire_and_ttl_commands() {
    let (mut store, clock) = store_at(0);
    let mut run = |line: &str| execute(&mut store, parse(line).unwrap()).to_string();
    assert_eq!(run("SET k v"), "OK");
    assert_eq!(run("EXPIRE k 3"), "(integer) 1");
    assert_eq!(run("expire nope 3"), "(integer) 0");
    assert_eq!(run("TTL k"), "(integer) 3");
    clock.advance(3_000);
    assert_eq!(run("TTL k"), "(integer) -2");
    assert_eq!(run("GET k"), "(nil)");

    assert_eq!(
        parse("EXPIRE k soon").unwrap_err().to_string(),
        "ERR value is not an integer or out of range"
    );
    assert_eq!(
        parse("EXPIRE k -1").unwrap_err().to_string(),
        "ERR value is not an integer or out of range"
    );
    assert_eq!(
        parse("TTL").unwrap_err().to_string(),
        "ERR wrong number of arguments for 'ttl'"
    );
    assert_eq!(
        parse("EXPIRE k").unwrap_err().to_string(),
        "ERR wrong number of arguments for 'expire'"
    );
}

#[test]
fn execute_still_works_on_the_system_clock() {
    let mut store = Store::new();
    assert_eq!(execute(&mut store, parse("SET a 1").unwrap()), Response::Ok);
    assert_eq!(
        execute(&mut store, parse("TTL a").unwrap()),
        Response::Integer(-1)
    );
}

#[test]
fn session_runs_plain_commands() {
    let mut session = Session::new(Store::new());
    assert_eq!(session.execute_line("SET a 1"), "OK");
    assert_eq!(session.execute_line("GET a"), "\"1\"");
    assert_eq!(
        session.execute_line("BOGUS"),
        "(error) ERR unknown command 'BOGUS'"
    );
    assert_eq!(session.store().get("a"), Some("1"));
}

#[test]
fn transactions_queue_then_run() {
    let mut session = Session::new(Store::new());
    assert_eq!(session.execute_line("MULTI"), "OK");
    assert_eq!(session.execute_line("SET a 1"), "QUEUED");
    assert_eq!(session.execute_line("GET a"), "QUEUED");
    assert_eq!(session.execute_line("DEL zz"), "QUEUED");
    assert_eq!(session.store().get("a"), None, "nothing ran yet");
    assert_eq!(
        session.execute_line("exec"),
        "1) OK\n2) \"1\"\n3) (integer) 0"
    );
    assert_eq!(session.store().get("a"), Some("1"));
    assert_eq!(session.execute_line("GET a"), "\"1\"", "transaction ended");

    assert_eq!(session.execute_line("MULTI"), "OK");
    assert_eq!(session.execute_line("EXEC"), "(empty array)");
}

#[test]
fn transaction_errors() {
    let mut session = Session::new(Store::new());
    assert_eq!(
        session.execute_line("EXEC"),
        "(error) ERR EXEC without MULTI"
    );
    assert_eq!(
        session.execute_line("DISCARD"),
        "(error) ERR DISCARD without MULTI"
    );

    assert_eq!(session.execute_line("MULTI"), "OK");
    assert_eq!(
        session.execute_line("multi"),
        "(error) ERR MULTI calls can not be nested"
    );
    assert_eq!(session.execute_line("SET a 1"), "QUEUED");
    assert_eq!(session.execute_line("DISCARD"), "OK");
    assert_eq!(session.store().get("a"), None);

    assert_eq!(session.execute_line("MULTI"), "OK");
    assert_eq!(session.execute_line("SET b 1"), "QUEUED");
    assert_eq!(
        session.execute_line("SET b"),
        "(error) ERR wrong number of arguments for 'set'"
    );
    assert_eq!(
        session.execute_line("EXEC"),
        "(error) EXECABORT Transaction discarded because of previous errors."
    );
    assert_eq!(session.store().get("b"), None);
    assert_eq!(session.execute_line("SET b 2"), "OK");
}

#[test]
fn sessions_work_on_a_manual_clock() {
    let clock = ManualClock::new(0);
    let mut session = Session::new(Store::with_clock(clock.clone()));
    session.execute_line("SET a 1");
    session.execute_line("EXPIRE a 1");
    clock.advance(1_000);
    assert_eq!(session.execute_line("GET a"), "(nil)");
}
