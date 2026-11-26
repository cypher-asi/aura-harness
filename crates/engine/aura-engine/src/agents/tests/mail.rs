use super::*;

#[tokio::test]
async fn spawn_agent_mail_request_and_reply_share_a_thread() {
    let mut h = harness(AgentHostConfig::default());
    let mut mail = observe_mail(&h.host);

    let outcome = h
        .hooks
        .spawn_child(
            &h.root,
            None,
            helper_spec("helper", Some("review the parser")),
        )
        .await
        .unwrap();
    let child = outcome.child_agent_id;

    let request = next_mail(&mut mail).await;
    assert_eq!(request.kind, MessageKind::Request, "{request:?}");
    assert_eq!(request.from, h.root);
    assert_eq!(request.from_name, "root");
    assert_eq!(request.to, child);
    assert_eq!(request.to_name, "helper");
    assert_eq!(request.text, "review the parser");
    assert_eq!(request.subject, "review the parser");

    let reply = next_mail(&mut mail).await;
    assert_eq!(reply.kind, MessageKind::Reply, "{reply:?}");
    assert_eq!(reply.from, child);
    assert_eq!(reply.from_name, "helper");
    assert_eq!(reply.to, h.root);
    assert_eq!(reply.to_name, "root");
    assert_eq!(reply.text, "ok");
    assert_eq!(reply.thread, request.thread);
    assert_eq!(reply.subject, request.subject);
    assert!(reply.id > request.id, "ids increase: {request:?} {reply:?}");

    let delivered = next_message(&mut h.inbox).await;
    assert_eq!(delivered.from, child);
    assert_eq!(delivered.to, h.root);
    assert_eq!(delivered.kind, MessageKind::Reply);
    assert_eq!(delivered.thread, request.thread);
    assert_eq!(delivered.subject, request.subject);
    assert_no_mail(&mut mail);
}

#[tokio::test]
async fn mail_subject_threads_messages_sent_through_the_hook() {
    let mut h = harness(AgentHostConfig::default());
    let mut mail = observe_mail(&h.host);
    let child = h
        .hooks
        .spawn_child(&h.root, None, helper_spec("helper", None))
        .await
        .unwrap()
        .child_agent_id;
    assert_no_mail(&mut mail);
    let child_hooks = h.host.hooks_for(child);
    let root_hex = h.root.to_hex();

    let sends = [
        ("first finding", "Parser review"),
        ("second finding", "Parser review"),
        ("something else", "Other topic"),
    ];
    for (text, subject) in sends {
        child_hooks
            .deliver_message_in_thread(&root_hex, None, None, None, text, None, None, Some(subject))
            .await
            .unwrap();
    }

    let mut posted = Vec::new();
    for _ in 0..sends.len() {
        posted.push(next_mail(&mut mail).await);
    }
    for (message, (text, subject)) in posted.iter().zip(sends) {
        assert_eq!(message.kind, MessageKind::Request, "{message:?}");
        assert_eq!(message.from, child);
        assert_eq!(message.to, h.root);
        assert_eq!(message.text, text);
        assert_eq!(message.subject, subject);
    }
    assert_eq!(
        posted[0].thread, posted[1].thread,
        "same subject, same thread"
    );
    assert_ne!(
        posted[0].thread, posted[2].thread,
        "new subject, new thread"
    );
    assert_no_mail(&mut mail);

    // Each root delivery runs on its own task, so match them up by text.
    let mut delivered = Vec::new();
    for _ in 0..sends.len() {
        delivered.push(next_message(&mut h.inbox).await);
    }
    for (text, subject) in sends {
        let message = delivered
            .iter()
            .find(|m| m.text == text)
            .unwrap_or_else(|| panic!("{text:?} not delivered: {delivered:?}"));
        let expected = posted.iter().find(|m| m.text == text).unwrap();
        assert_eq!(message.from, child);
        assert_eq!(message.kind, MessageKind::Request);
        assert_eq!(message.subject, subject);
        assert_eq!(message.thread, expected.thread);
        let rendered = message.render();
        assert!(
            rendered.contains(&format!("subject \"{subject}\"")),
            "{rendered}"
        );
    }
}

#[tokio::test]
async fn only_the_mail_that_opens_a_thread_named_from_its_text_is_marked() {
    let h = harness(AgentHostConfig::default());
    let mut mail = observe_mail(&h.host);
    let child = h
        .hooks
        .spawn_child(&h.root, None, helper_spec("helper", None))
        .await
        .unwrap()
        .child_agent_id;
    assert_no_mail(&mut mail);
    let child_hooks = h.host.hooks_for(child);
    let root_hex = h.root.to_hex();

    let sends = [
        ("Found a parser bug\ndetails", None),
        ("and another one", None),
        ("bench numbers", Some("Benchmarks")),
    ];
    for (text, subject) in sends {
        child_hooks
            .deliver_message_in_thread(&root_hex, None, None, None, text, None, None, subject)
            .await
            .unwrap();
    }

    let opened = next_mail(&mut mail).await;
    assert_eq!(opened.subject, "Found a parser bug");
    assert!(opened.subject_from_text, "{opened:?}");

    let continued = next_mail(&mut mail).await;
    assert_eq!(continued.thread, opened.thread);
    assert!(!continued.subject_from_text, "{continued:?}");

    let given = next_mail(&mut mail).await;
    assert_ne!(given.thread, opened.thread);
    assert!(!given.subject_from_text, "{given:?}");
    assert_no_mail(&mut mail);
}

#[tokio::test]
async fn user_messages_are_not_mail() {
    let mut h = harness(AgentHostConfig::default());
    let mut mail = observe_mail(&h.host);
    let child = h
        .hooks
        .spawn_child(&h.root, None, helper_spec("helper", None))
        .await
        .unwrap()
        .child_agent_id;

    // Mail is posted inline in `enqueue`, so its absence is visible as
    // soon as `send` returns.
    h.host
        .send(None, child, "hello from the user".into())
        .unwrap();
    assert_no_mail(&mut mail);
    assert!(h.host.send(None, h.root, "hello root".into()).is_err());
    assert_no_mail(&mut mail);

    // The child handles its queue in order, so once the answer to this
    // agent message is mailed, the user's turn has finished too.
    h.host.send(Some(h.root), child, "ping".into()).unwrap();
    let request = next_mail(&mut mail).await;
    assert_eq!(request.kind, MessageKind::Request, "{request:?}");
    assert_eq!(request.from, h.root);
    assert_eq!(request.text, "ping");
    let reply = next_mail(&mut mail).await;
    assert_eq!(reply.kind, MessageKind::Reply, "{reply:?}");
    assert_eq!(reply.from, child);
    assert_eq!(reply.to, h.root);
    assert_eq!(reply.thread, request.thread);
    let delivered = next_message(&mut h.inbox).await;
    assert_eq!(delivered.thread, request.thread);
    assert_no_mail(&mut mail);
    assert_eq!(h.host.info(child).unwrap().turns, 2);
}

#[tokio::test]
async fn mail_forwarded_from_a_childs_reply_goes_in_the_parents_thread() {
    let mut h = harness(AgentHostConfig::default());
    let mut mail = observe_mail(&h.host);
    let middle = h
        .hooks
        .spawn_child(
            &h.root,
            None,
            helper_spec("middle", Some("review the parser")),
        )
        .await
        .unwrap()
        .child_agent_id;
    let request = next_mail(&mut mail).await;
    assert_eq!((request.from, request.to), (h.root, middle));
    let answer = next_mail(&mut mail).await;
    assert_eq!((answer.from, answer.to), (middle, h.root));
    assert_eq!(answer.thread, request.thread);
    next_message(&mut h.inbox).await;

    // The middle agent starts a child of its own, whose reply wakes it.
    let leaf = h
        .host
        .hooks_for(middle)
        .spawn_child(&middle, None, helper_spec("leaf", Some("check the lexer")))
        .await
        .unwrap()
        .child_agent_id;
    let down = next_mail(&mut mail).await;
    assert_eq!((down.from, down.to), (middle, leaf));
    assert_ne!(
        down.thread, request.thread,
        "a spawn prompt starts a thread"
    );
    let up = next_mail(&mut mail).await;
    assert_eq!(
        (up.from, up.to, up.kind),
        (leaf, middle, MessageKind::Reply)
    );
    assert_eq!(up.thread, down.thread);

    let forwarded = next_mail(&mut mail).await;
    assert_eq!(
        (forwarded.from, forwarded.to, forwarded.kind),
        (middle, h.root, MessageKind::Reply)
    );
    assert_eq!(forwarded.thread, request.thread, "{forwarded:?}");
    assert_eq!(forwarded.subject, request.subject);
    let delivered = next_message(&mut h.inbox).await;
    assert_eq!(delivered.thread, request.thread);
    assert_no_mail(&mut mail);
}

#[tokio::test]
async fn mail_from_a_delegated_task_starts_a_new_thread() {
    let mut h = harness(AgentHostConfig::default());
    let mut mail = observe_mail(&h.host);
    h.hooks
        .spawn_child(
            &h.root,
            None,
            helper_spec("helper", Some("review the parser")),
        )
        .await
        .unwrap();
    let request = next_mail(&mut mail).await;
    let answer = next_mail(&mut mail).await;
    assert_eq!(answer.thread, request.thread);
    next_message(&mut h.inbox).await;

    h.hooks
        .delegate_task("helper", None, None, "Benchmark the lexer", None, None)
        .await
        .unwrap();
    let delegated = next_mail(&mut mail).await;
    assert_eq!(delegated.kind, MessageKind::Request);
    assert_eq!(delegated.text, "Benchmark the lexer");
    assert_ne!(delegated.thread, request.thread, "{delegated:?}");
    assert_eq!(delegated.subject, "Benchmark the lexer");
}

#[tokio::test]
async fn mail_answers_a_message_dropped_by_a_stop_with_a_cancelled_reply() {
    let h = harness(AgentHostConfig::default());
    let mut mail = observe_mail(&h.host);
    let child = h
        .hooks
        .spawn_child(&h.root, None, helper_spec("sleeper", Some("slow")))
        .await
        .unwrap()
        .child_agent_id;
    let first = next_mail(&mut mail).await;
    assert_eq!(first.text, "slow");
    for _ in 0..200 {
        if h.host.info(child).unwrap().state == AgentState::Running {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    h.host.send(Some(h.root), child, "ping".into()).unwrap();
    let queued = next_mail(&mut mail).await;
    assert_eq!(queued.text, "ping");
    assert_eq!(h.host.info(child).unwrap().queued, 1);

    h.hooks
        .lifecycle(&child.to_hex(), None, None, "stop")
        .await
        .unwrap();

    let cancelled = next_mail(&mut mail).await;
    assert_eq!(cancelled.kind, MessageKind::Reply, "{cancelled:?}");
    assert_eq!((cancelled.from, cancelled.to), (child, h.root));
    assert_eq!(
        (cancelled.from_name.as_str(), cancelled.to_name.as_str()),
        ("sleeper", "root")
    );
    assert_eq!(cancelled.text, "was cancelled");
    assert_eq!(cancelled.thread, queued.thread);
    assert_eq!(cancelled.subject, queued.subject);
    assert!(cancelled.id > queued.id);
    assert_no_mail(&mut mail);
}
