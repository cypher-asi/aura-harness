//! Node limits and benchmarks.

use super::*;

#[test]
fn the_oldest_nodes_go_once_there_are_too_many() {
    let lead = agent(1);
    let mut records = Records::default();
    let mut flow = FlowBuilder::new(lead, "Lead");
    for n in 0..MAX_NODES + 5 {
        flow.push(&records.reply(lead, &format!("reply {n}")));
    }
    let (_, nodes) = flow.snapshot();
    assert_eq!(nodes.len(), MAX_NODES);
    assert_eq!(nodes[0].label, "reply 5");
    assert_eq!(flow.by_tx.len(), MAX_NODES);
}

#[test]
fn a_benchmark_says_what_each_run_worked_on_and_how_it_did() {
    let (lead, host, arm_a, arm_b, a_1, b_1, coder) = (
        agent(1),
        agent(2),
        agent(3),
        agent(4),
        agent(5),
        agent(6),
        agent(7),
    );
    let mut flow = FlowBuilder::new(lead, "Lead");
    flow.add_lane(host, "Compare #1", Some(lead));
    flow.add_lane(arm_a, "A · gate", Some(host));
    flow.add_lane(arm_b, "B · opus", Some(host));
    flow.add_lane(a_1, "a-1", Some(arm_a));
    let _ = flow.snapshot();

    let payload = bench_payload(
        lead,
        vec![
            bench_run(CompareArm::A, a_1, true, "coding/write-file"),
            bench_run(CompareArm::B, b_1, false, "coding/write-file"),
        ],
    );
    let mut records = Records::default();
    flow.push(&records.entry(host, TransactionType::System, payload.to_bytes()));

    // The lanes known so far are reported again with what they are.
    let (lanes, _) = flow.drain().expect("the lanes changed");
    let a = lane_of(&lanes, a_1)
        .and_then(|lane| lane.bench)
        .expect("a-1 is a run");
    assert_eq!(a.group, "coding/write-file");
    assert_eq!((a.run.as_str(), a.arm), ("a-1", 0));
    assert_eq!(a.arm_label, "gate");
    assert_eq!(a.check_passed, Some(true));
    assert!(lane_of(&lanes, arm_a).is_some_and(|lane| lane.bench_arm));
    assert!(lane_of(&lanes, host).is_none(), "the host did not change");

    // A run that joins later, and an agent under a run, learn theirs.
    flow.add_lane(b_1, "b-1", Some(arm_b));
    flow.add_lane(coder, "coder #1", Some(a_1));
    let (lanes, _) = flow.snapshot();
    let b = lane_of(&lanes, b_1)
        .and_then(|lane| lane.bench)
        .expect("b-1 is a run");
    assert_eq!((b.arm, b.arm_label.as_str()), (1, "opus"));
    assert_eq!(b.check_passed, Some(false));
    let coder = lane_of(&lanes, coder).expect("a lane");
    assert!(coder.bench.is_none() && !coder.bench_arm);
}
