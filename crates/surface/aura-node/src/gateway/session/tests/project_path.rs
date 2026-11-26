//! `project_path` sandbox checks on chat runtime requests.

use super::support::{absolute_path, chat_request_with_project_path, test_session};

#[test]
fn project_path_allowed_when_no_base() {
    let project_path = absolute_path(&["any", "absolute", "path"]);
    let mut session = test_session(None);
    let req = chat_request_with_project_path(&project_path);
    assert!(session.apply_chat_runtime_request(req).is_ok());
    assert_eq!(session.project_path.unwrap(), project_path);
}

#[test]
fn project_path_allowed_under_base() {
    let project_base = absolute_path(&["home", "aura"]);
    let project_path = project_base.join("myproject");
    let mut session = test_session(Some(project_base));
    let req = chat_request_with_project_path(&project_path);
    assert!(session.apply_chat_runtime_request(req).is_ok());
}

#[test]
fn project_path_blocked_outside_base() {
    let project_base = absolute_path(&["home", "aura"]);
    let project_path = absolute_path(&["etc", "passwd"]);
    let mut session = test_session(Some(project_base));
    let req = chat_request_with_project_path(&project_path);
    let result = session.apply_chat_runtime_request(req);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("must be under"));
}

#[test]
fn safe_workspace_root_is_not_an_accepted_project_path() {
    let project_base = absolute_path(&["home", "aura"]);
    let mut session = test_session(Some(project_base));
    let safe_root = session
        .workspace_base
        .parent()
        .expect("workspace base has a data-dir parent")
        .join("safe-workspaces");
    let req = chat_request_with_project_path(&safe_root);

    let result = session.apply_chat_runtime_request(req);

    assert!(result.is_err());
    assert!(result.unwrap_err().contains("must be under"));
}

#[test]
fn project_path_blocked_with_traversal() {
    let project_base = absolute_path(&["home", "aura"]);
    let project_path = project_base.join("..").join("etc").join("passwd");
    let mut session = test_session(Some(project_base));
    let req = chat_request_with_project_path(&project_path);
    let result = session.apply_chat_runtime_request(req);
    assert!(result.is_err());
}

#[test]
fn project_path_rejects_relative() {
    let mut session = test_session(None);
    let req = chat_request_with_project_path(std::path::Path::new("relative/path"));
    let result = session.apply_chat_runtime_request(req);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("absolute"));
}
