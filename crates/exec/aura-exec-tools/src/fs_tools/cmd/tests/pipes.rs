use super::*;

#[test]
fn short_output_is_read_whole() {
    let kept = read_capped(&b"hello\nworld\n"[..], &LiveOutput::default());
    assert_eq!(kept, b"hello\nworld\n");
}

#[test]
fn long_output_keeps_its_start_and_end() {
    let mut output = vec![b'a'; PIPE_KEEP_BYTES];
    output.extend(vec![b'm'; 3 * PIPE_KEEP_BYTES]);
    output.extend(vec![b'z'; PIPE_KEEP_BYTES]);
    let kept = read_capped(&output[..], &LiveOutput::default());
    let marker = format!(
        "\n... ({} bytes of output dropped) ...\n",
        3 * PIPE_KEEP_BYTES
    );
    assert_eq!(kept.len(), 2 * PIPE_KEEP_BYTES + marker.len());
    assert!(kept[..PIPE_KEEP_BYTES].iter().all(|&b| b == b'a'));
    assert!(kept[kept.len() - PIPE_KEEP_BYTES..]
        .iter()
        .all(|&b| b == b'z'));
    assert!(String::from_utf8_lossy(&kept).contains(&marker));
}
