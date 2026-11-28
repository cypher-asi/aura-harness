//! The recorder's writer thread: steps to the bundle's steps file, then
//! in batches to the sinks.

use std::path::Path;
use std::sync::mpsc::{self, SyncSender};

use aura_core_types::trace::Step;

use super::{lock, Sinks};

pub(super) enum WriteOp {
    Step(Box<Step>),
    Flush(SyncSender<()>),
}

pub(super) fn writer_loop(path: &Path, rx: &mpsc::Receiver<WriteOp>, run_id: &str, sinks: &Sinks) {
    use std::io::Write;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path);
    let Ok(file) = file else {
        tracing::warn!(path = %path.display(), "run trace writer could not open steps file");
        return;
    };
    let mut out = std::io::BufWriter::new(file);
    let mut batch: Vec<Step> = Vec::new();
    let send = |batch: &mut Vec<Step>| {
        if batch.is_empty() {
            return;
        }
        for sink in lock(sinks).iter() {
            sink.steps(run_id, batch);
        }
        batch.clear();
    };
    while let Ok(op) = rx.recv() {
        let mut pending = Some(op);
        while let Some(op) = pending.take() {
            match op {
                WriteOp::Step(step) => {
                    if serde_json::to_writer(&mut out, &*step).is_ok() {
                        let _ = out.write_all(b"\n");
                    }
                    batch.push(*step);
                }
                WriteOp::Flush(ack) => {
                    let _ = out.flush();
                    send(&mut batch);
                    let _ = ack.send(());
                }
            }
            pending = rx.try_recv().ok();
        }
        let _ = out.flush();
        send(&mut batch);
    }
    let _ = out.flush();
    send(&mut batch);
}
