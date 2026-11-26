//! Recording prompts on an agent and clearing its inbox.

use super::*;

/// Record `text` on `session` as a prompt from the user, not classified:
/// a message sent into a running turn, which is part of that turn. The
/// kernels' record observer shows the entry in the Record panel.
pub(in crate::event_loop) async fn record_user_prompt(
    session: &AgentSession,
    text: String,
) -> Result<(), String> {
    record_prompt(session, &PromptPayload::new(text))
        .await
        .map(|_| ())
}

/// Record `prompt` on `session` as a prompt from the user, returning the
/// transaction and the record entry it became.
pub(crate) async fn record_prompt(
    session: &AgentSession,
    prompt: &PromptPayload,
) -> Result<(Transaction, RecordEntry), String> {
    let prompt_tx = Transaction::user_prompt_with(session.agent_id(), prompt);
    let (tx, token) = enqueue_and_dequeue(session, prompt_tx)?;
    let result = session
        .kernel
        .process_dequeued(tx.clone(), token)
        .await
        .map_err(|e| {
            error!(error = %e, "Failed to persist prompt record via kernel");
            format!("Kernel error: {e}")
        })?;
    Ok((tx, result.entry))
}

/// Record `prompt` on `session` as the prompt of the turn it is running,
/// which the turn's file checkpoints then name; returns its transaction
/// hash.
pub(in crate::event_loop) async fn record_turn_prompt(
    session: &AgentSession,
    prompt: &PromptPayload,
) -> Result<Hash, String> {
    let (tx, _) = record_prompt(session, prompt).await?;
    if let Some(files) = &session.file_checkpoints {
        files.note_prompt_tx(session.agent_id(), tx.hash);
    }
    Ok(tx.hash)
}

/// Clear transactions left in the agent's inbox by an earlier run.
pub(crate) async fn drain_stale_inbox(session: &AgentSession) {
    let store = session.kernel.store();
    let mut stale_count = 0;
    while let Ok(Some((token, tx))) = store.dequeue_tx(session.agent_id()) {
        warn!(
            stale_inbox_seq = token.inbox_seq(),
            stale_tx_type = ?tx.tx_type,
            "Discarding stale inbox transaction"
        );
        match session.kernel.process_dequeued(tx, token).await {
            Ok(_result) => {
                stale_count += 1;
            }
            Err(e) => {
                error!(error = %e, "Failed to clear stale transaction via kernel");
                break;
            }
        }
        if stale_count > 10 {
            error!("Too many stale transactions, aborting drain");
            break;
        }
    }
}

/// Put `tx` through the agent's inbox, returning it with its dequeue token.
fn enqueue_and_dequeue(
    session: &AgentSession,
    tx: Transaction,
) -> Result<(Transaction, aura_store_record::DequeueToken), String> {
    let store = session.kernel.store();
    if let Err(e) = store.enqueue_tx(&tx) {
        error!(error = %e, "Failed to enqueue transaction");
        return Err(format!("Storage error: {e}"));
    }

    let (token, dequeued_tx) = match store.dequeue_tx(session.agent_id()) {
        Ok(Some(item)) => item,
        Ok(None) => {
            error!("Transaction was enqueued but not found in inbox");
            return Err("Storage error: the message was not queued".to_string());
        }
        Err(e) => {
            error!(error = %e, "Failed to dequeue transaction");
            return Err(format!("Storage error: {e}"));
        }
    };

    if dequeued_tx.hash != tx.hash {
        error!("Transaction mismatch after draining stale entries");
    }

    Ok((tx, token))
}
