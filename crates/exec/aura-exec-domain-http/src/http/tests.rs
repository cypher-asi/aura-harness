use super::*;
use aura_exec_traits::domain::DomainApi;

#[tokio::test]
async fn project_connection_failure_preserves_reason_in_display() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let base = format!("http://{addr}");
    let api = HttpDomainApi::new(&base, &base, Some(base.clone())).unwrap();
    let error = api
        .get_project("project-test", Some("test-token"))
        .await
        .unwrap_err();
    let displayed = error.to_string();
    assert!(displayed.contains("connection failed"), "{displayed}");
    assert!(
        displayed.contains("/api/projects/project-test"),
        "{displayed}"
    );
    assert!(!displayed.contains("test-token"));
}

#[tokio::test]
async fn specs_timeout_preserves_reason_in_display() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.unwrap();
        std::future::pending::<()>().await;
    });
    let mut api = HttpDomainApi::new(&base, &base, Some(base.clone())).unwrap();
    api.http = Client::builder()
        .timeout(std::time::Duration::from_millis(50))
        .build()
        .unwrap();
    let error = api
        .list_specs("project-test", Some("test-token"))
        .await
        .unwrap_err();
    server.abort();
    let displayed = error.to_string();
    assert!(displayed.contains("request timed out"), "{displayed}");
    assert!(
        displayed.contains("/api/projects/project-test/specs"),
        "{displayed}"
    );
    assert!(!displayed.contains("test-token"));
}

/// `specs_tasks_base_url()` must prefer the aura-os-server override
/// when set. This is the routing hook that lets an operator flip
/// `AURA_OS_SERVER_URL` and redirect spec / task / project / log
/// writes through aura-os-server so its disk-mirror + SSE + JWT
/// billing side effects fire.
#[test]
fn specs_tasks_base_url_prefers_os_server_when_set() {
    let api = HttpDomainApi::new(
        "https://storage.example.com",
        "https://network.example.com",
        Some("http://os".to_string()),
    )
    .expect("build HttpDomainApi");

    assert_eq!(api.specs_tasks_base_url(), "http://os");

    // Sanity-check URL composition for the highest-value route
    // (spec create) to lock in the exact path shape the rest of
    // the code assumes.
    let url = format!(
        "{}/api/projects/{pid}/specs",
        api.specs_tasks_base_url(),
        pid = "proj-1",
    );
    assert_eq!(url, "http://os/api/projects/proj-1/specs");
}

/// With no override, routing must fall back to `aura_storage_url`
/// verbatim so existing deployments that haven't set
/// `AURA_OS_SERVER_URL` see no behavior change.
#[test]
fn specs_tasks_base_url_falls_back_to_storage_when_unset() {
    let api = HttpDomainApi::new(
        "https://storage.example.com",
        "https://network.example.com",
        None,
    )
    .expect("build HttpDomainApi");

    assert_eq!(api.specs_tasks_base_url(), "https://storage.example.com");

    let url = format!(
        "{}/api/projects/{pid}/specs",
        api.specs_tasks_base_url(),
        pid = "proj-1",
    );
    assert_eq!(url, "https://storage.example.com/api/projects/proj-1/specs");
}

/// Trailing slashes on either base URL must be normalised away so
/// `format!("{base}/api/...")` never produces a `//api/...` path
/// that some reverse proxies reject. Regression gate for the
/// override — if we forget to trim we'd silently break every route
/// that runs through the helper.
#[test]
fn os_server_url_trims_trailing_slash() {
    let api = HttpDomainApi::new(
        "https://storage.example.com/",
        "https://network.example.com",
        Some("http://os/".to_string()),
    )
    .expect("build HttpDomainApi");

    assert_eq!(api.specs_tasks_base_url(), "http://os");
}

/// `project_base_url()` prefers the aura-os-server override when
/// set so project GET / PUT writes fire the server's SSE / billing
/// side effects alongside specs and tasks.
#[test]
fn project_base_url_prefers_os_server_when_set() {
    let api = HttpDomainApi::new(
        "https://storage.example.com",
        "https://network.example.com",
        Some("http://os".to_string()),
    )
    .expect("build HttpDomainApi");

    assert_eq!(api.project_base_url(), "http://os");
}

/// Without the override, project routes must fall back to
/// `aura_network_url` (not `storage_url`). aura-storage has no
/// `/api/projects/:id` GET — only `/api/projects/:id/{specs,tasks,
/// artifacts,logs}` sub-routes — so a storage fallback 404s every
/// `get_project` the dev-loop issues and no task ever runs. This
/// test pins the pre-`feat(domain): route DomainApi writes through
/// aura-os-server` behavior so a future base-URL refactor can't
/// silently reintroduce that regression.
#[test]
fn project_base_url_falls_back_to_network_when_unset() {
    let api = HttpDomainApi::new(
        "https://storage.example.com",
        "https://network.example.com",
        None,
    )
    .expect("build HttpDomainApi");

    assert_eq!(api.project_base_url(), "https://network.example.com");

    let url = format!(
        "{}/api/projects/{pid}",
        api.project_base_url(),
        pid = "proj-1",
    );
    assert_eq!(url, "https://network.example.com/api/projects/proj-1");
}
