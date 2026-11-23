//! A turn's `Outcome`s on the record: writing one, reading back the
//! latest turn's, and the later signals the user gives on a turn by
//! sending its prompt again to a stronger model or rewinding it. Each
//! later signal appends another `Outcome` for the same prompt, and the
//! latest one counts.

use aura_agent_kernel::Kernel;
use aura_core_types::{
    AgentId, Hash, OutcomePayload, OutcomeSignal, PromptPayload, RecordEntry, Transaction,
    TransactionType,
};
use aura_gate_evidence::outcome::is_retry_stronger;
use aura_store_db::{ReadStore, StoreError};
use tracing::{info, warn};

/// Record entries read per scan of a whole record, oldest first.
pub(crate) const SCAN_BATCH: usize = 500;

/// Entries scanned back from the head of a record to find a turn's
/// outcome.
const SCAN_LIMIT: usize = 2_000;

/// Hand the record of `agent_id` to `take`, oldest first, in batches of
/// up to [`SCAN_BATCH`] entries.
///
/// # Errors
/// When a batch could not be read; the batches before it were taken.
pub(crate) fn for_each_batch<S: ReadStore + ?Sized>(
    store: &S,
    agent_id: AgentId,
    mut take: impl FnMut(Vec<RecordEntry>),
) -> Result<(), StoreError> {
    let mut from = 1;
    loop {
        let entries = store.scan_record(agent_id, from, SCAN_BATCH)?;
        let Some(last) = entries.last().map(|entry| entry.seq) else {
            return Ok(());
        };
        let full = entries.len() == SCAN_BATCH;
        take(entries);
        if !full {
            return Ok(());
        }
        from = last + 1;
    }
}

/// Write `outcome` to the record of `agent_id` through `kernel`. A failure
/// is logged.
pub async fn record(kernel: &Kernel, agent_id: AgentId, outcome: &OutcomePayload) -> bool {
    match kernel
        .process_direct(Transaction::outcome(agent_id, outcome))
        .await
    {
        Ok(_) => {
            info!(
                %agent_id,
                signal = %outcome.signal,
                category = %outcome.category,
                model = %outcome.model,
                "Recorded turn outcome"
            );
            true
        }
        Err(e) => {
            warn!(%agent_id, error = %e, "Failed to record turn outcome");
            false
        }
    }
}

/// The latest turn on a record: the outcome written when it ended, and its
/// prompt when the scan still reaches it.
#[derive(Debug, Clone, PartialEq)]
pub struct LastTurn {
    pub outcome: OutcomePayload,
    pub prompt: Option<PromptPayload>,
}

/// The latest turn recorded for `agent_id`, if one ended within the last
/// `SCAN_LIMIT` entries.
pub fn last_turn<S: ReadStore + ?Sized>(store: &S, agent_id: AgentId) -> Option<LastTurn> {
    let entries = newest_first(store, agent_id);
    let outcome = entries
        .iter()
        .filter_map(|entry| entry.tx.outcome_payload())
        .find(|outcome| outcome.signal.is_turn_end())?;
    let prompt = entries
        .iter()
        .find(|entry| {
            entry.tx.tx_type == TransactionType::UserPrompt && entry.tx.hash == outcome.prompt_tx
        })
        .and_then(|entry| entry.tx.prompt_payload());
    Some(LastTurn { outcome, prompt })
}

/// The outcome written when the turn of each prompt in `prompts` ended,
/// for those the scan reaches, in the order of `prompts`.
fn turn_outcomes<S: ReadStore + ?Sized>(
    store: &S,
    agent_id: AgentId,
    prompts: &[Hash],
) -> Vec<OutcomePayload> {
    let ended: Vec<OutcomePayload> = newest_first(store, agent_id)
        .iter()
        .filter_map(|entry| entry.tx.outcome_payload())
        .filter(|outcome| outcome.signal.is_turn_end())
        .collect();
    prompts
        .iter()
        .filter_map(|prompt| ended.iter().find(|o| o.prompt_tx == *prompt).cloned())
        .collect()
}

fn newest_first<S: ReadStore + ?Sized>(store: &S, agent_id: AgentId) -> Vec<RecordEntry> {
    let head = match store.get_head_seq(agent_id) {
        Ok(head) => head,
        Err(e) => {
            warn!(%agent_id, error = %e, "Failed to read the record head");
            return Vec::new();
        }
    };
    store
        .scan_record_descending(agent_id, head, SCAN_LIMIT)
        .unwrap_or_else(|e| {
            warn!(%agent_id, error = %e, "Failed to scan the record for outcomes");
            Vec::new()
        })
}

/// Before a turn of `agent_id` answers `text` on `model`: when `text`
/// repeats the previous turn's prompt on a stronger model, the previous
/// turn failed.
pub async fn note_retry(kernel: &Kernel, agent_id: AgentId, text: &str, model: &str) {
    let Some(LastTurn {
        outcome,
        prompt: Some(prompt),
    }) = last_turn(kernel.store().as_ref(), agent_id)
    else {
        return;
    };
    if is_retry_stronger(&prompt.text, &outcome.model, text, model) {
        info!(%agent_id, from = %outcome.model, to = %model, "Prompt retried on a stronger model");
        let retried = outcome.with_signal(OutcomeSignal::RetryStronger);
        record(kernel, agent_id, &retried).await;
    }
}

/// After `/rewind` undid the turns of `agent_id` whose prompts are
/// `prompts`: each of them failed.
pub async fn note_rewind(kernel: &Kernel, agent_id: AgentId, prompts: &[Hash]) {
    for outcome in turn_outcomes(kernel.store().as_ref(), agent_id, prompts) {
        record(
            kernel,
            agent_id,
            &outcome.with_signal(OutcomeSignal::Rewind),
        )
        .await;
    }
}
