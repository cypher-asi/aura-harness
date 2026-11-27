use super::is_openable;

#[test]
fn only_web_and_mail_links_open() {
    for url in [
        "https://example.com",
        "http://localhost:8080/a?b=1",
        "HTTPS://EXAMPLE.COM/",
        "mailto:dev@example.com",
    ] {
        assert!(is_openable(url, false), "{url}");
    }
    for url in [
        "file:///etc/passwd",
        "/etc/passwd",
        "src/lib.rs",
        "javascript:alert(1)",
        "ssh://host",
        "vscode://file/x",
        "https://",
        "mailto:",
        "https://x.dev/a b",
        "https://x.dev/\n",
        "",
    ] {
        assert!(!is_openable(url, false), "{url:?}");
    }
}

#[test]
fn a_link_that_cmd_would_read_does_not_open_through_it() {
    for url in ["https://x.dev/?a=1&b=2", "https://x.dev/%20"] {
        assert!(is_openable(url, false), "{url}");
        assert!(!is_openable(url, true), "{url}");
    }
}
