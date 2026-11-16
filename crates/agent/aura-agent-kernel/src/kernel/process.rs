//! Top-level transaction processing for the kernel.
//!
//! `process_direct` and `process_dequeued` bracket the `process_tx`
//! dispatcher, feeding the resulting `ProcessResult` to the matching store
//! append variant (direct / dequeued / capability-aware). Tool proposals
//! bypass `process_tx` because they append several linked entries of
//! their own (see [`super::tools`]). `process_tx`
//! itself routes each transaction to the appropriate handler based on its
//! [`TransactionType`] and owns the `System`-transaction capability-install
//! decode path.

use super::{Kernel, ProcessResult};
use crate::context::hash_tx_with_window;
use aura_core_types::{
    AgentMode, ApprovalLevel, ModeChangePayload, RecordEntry, RuntimeCapabilityInstall, SystemKind,
    Transaction, TransactionType, UserToolDefaults,
};
use aura_store_db::DequeueToken;

impl Kernel {
    /// Process a transaction from a direct (non-inbox) source.
    ///
    /// # Errors
    /// Returns error if processing or storage fails.
    pub async fn process_direct(
        &self,
        tx: Transaction,
    ) -> Result<ProcessResult, crate::KernelError> {
        Self::reject_kernel_only(&tx)?;
        if tx.tx_type == TransactionType::ToolProposal {
            return self.process_tool_proposal(tx, None).await;
        }
        let seq = self.next_seq()?;
        let result = self.process_tx(&tx, seq).await?;
        self.store
            .append_entry_direct_with_runtime_capabilities(
                self.agent_id,
                seq,
                &result.entry,
                result.runtime_capability_update.as_ref(),
                result.clear_runtime_capabilities,
            )
            .map_err(|e| {
                crate::KernelError::Store(format!(
                    "append_entry_direct_with_runtime_capabilities: {e}"
                ))
            })?;
        self.notify_entry(&result.entry);
        Ok(result)
    }

    /// Process a transaction dequeued from the inbox.
    ///
    /// # Errors
    /// Returns error if processing or storage fails.
    pub async fn process_dequeued(
        &self,
        tx: Transaction,
        token: DequeueToken,
    ) -> Result<ProcessResult, crate::KernelError> {
        Self::reject_kernel_only(&tx)?;
        if tx.tx_type == TransactionType::ToolProposal {
            return self.process_tool_proposal(tx, Some(token)).await;
        }
        let seq = self.next_seq()?;
        let result = self.process_tx(&tx, seq).await?;
        self.store
            .append_entry_dequeued_with_runtime_capabilities(
                self.agent_id,
                seq,
                &result.entry,
                token,
                result.runtime_capability_update.as_ref(),
                result.clear_runtime_capabilities,
            )
            .map_err(|e| {
                crate::KernelError::Store(format!(
                    "append_entry_dequeued_with_runtime_capabilities: {e}"
                ))
            })?;
        self.notify_entry(&result.entry);
        Ok(result)
    }

    /// Switch the session mode, recording a `mode_change` System entry
    /// before the new mode takes effect. Returns `None` when the kernel
    /// already enforces `mode`.
    ///
    /// # Errors
    /// Returns error if processing or storage fails; the mode is then
    /// unchanged.
    pub async fn switch_mode(
        &self,
        mode: AgentMode,
    ) -> Result<Option<ProcessResult>, crate::KernelError> {
        let from = self.policy.mode();
        if from == mode {
            return Ok(None);
        }
        let tx = Transaction::mode_change(self.agent_id, from, mode);
        let result = self.append_mode_change(&tx).await?;
        self.policy.set_mode(mode);
        Ok(Some(result))
    }

    /// Switch the session mode and the per-tool permission defaults
    /// together, recording one `mode_change` entry that names both
    /// permission tables before either takes effect. `from_permissions`
    /// and `to_permissions` are the caller's names for the tables. When
    /// neither the mode nor the name changes, `user_default` is applied
    /// without a record (it is the same table, perhaps over a changed
    /// tool list) and `None` is returned.
    ///
    /// # Errors
    /// Returns error if processing or storage fails; the policy is then
    /// unchanged.
    pub async fn switch_permissions(
        &self,
        mode: AgentMode,
        approval: ApprovalLevel,
        user_default: UserToolDefaults,
        from_permissions: &str,
        to_permissions: &str,
    ) -> Result<Option<ProcessResult>, crate::KernelError> {
        let from = self.policy.mode();
        let result = if from == mode && from_permissions == to_permissions {
            None
        } else {
            let tx = Transaction::mode_change_with(
                self.agent_id,
                ModeChangePayload {
                    system_kind: SystemKind::ModeChange,
                    from,
                    to: mode,
                    from_permissions: Some(from_permissions.to_string()),
                    to_permissions: Some(to_permissions.to_string()),
                },
            );
            Some(self.append_mode_change(&tx).await?)
        };
        self.policy.reconfigure(|config| {
            config.mode = mode;
            config.approval = approval;
            config.user_default = user_default;
        });
        Ok(result)
    }

    async fn append_mode_change(
        &self,
        tx: &Transaction,
    ) -> Result<ProcessResult, crate::KernelError> {
        let seq = self.next_seq()?;
        let result = self.process_tx(tx, seq).await?;
        self.store
            .append_entry_direct(self.agent_id, seq, &result.entry)
            .map_err(|e| crate::KernelError::Store(format!("append_entry_direct: {e}")))?;
        self.notify_entry(&result.entry);
        Ok(result)
    }

    /// `PolicyDecision` and `ToolExecution` entries are written only by the
    /// kernel's tool pipeline, and mode changes only by [`Self::switch_mode`];
    /// accepting them from callers would let a submitter forge the audit
    /// trail of what policy allowed a call.
    fn reject_kernel_only(tx: &Transaction) -> Result<(), crate::KernelError> {
        if matches!(
            tx.tx_type,
            TransactionType::PolicyDecision | TransactionType::ToolExecution
        ) {
            return Err(crate::KernelError::Internal(format!(
                "{:?} transactions are written only by the kernel tool pipeline",
                tx.tx_type
            )));
        }
        if tx.mode_change_payload().is_some() {
            return Err(crate::KernelError::Internal(
                "mode_change transactions are written only by Kernel::switch_mode".to_string(),
            ));
        }
        Ok(())
    }

    pub(super) async fn process_tx(
        &self,
        tx: &Transaction,
        seq: u64,
    ) -> Result<ProcessResult, crate::KernelError> {
        let window = self.load_window(seq)?;
        let context_hash = hash_tx_with_window(tx, &window)?;

        match tx.tx_type {
            TransactionType::SessionStart => {
                self.policy.clear_session_approvals();
                let entry = RecordEntry::builder(seq, tx.clone())
                    .context_hash(context_hash)
                    .build();
                Ok(ProcessResult {
                    entry,
                    tool_output: None,
                    had_failures: false,
                    runtime_capability_update: None,
                    clear_runtime_capabilities: true,
                    tool_decision: None,
                    policy_decision: None,
                })
            }
            TransactionType::System => {
                let runtime_capability_update = Self::runtime_capability_update_from_tx(tx)
                    .map_err(|e| {
                        crate::KernelError::Serialization(format!(
                            "deserialize capability install: {e}"
                        ))
                    })?;
                let entry = RecordEntry::builder(seq, tx.clone())
                    .context_hash(context_hash)
                    .build();
                Ok(ProcessResult {
                    entry,
                    tool_output: None,
                    had_failures: false,
                    runtime_capability_update,
                    clear_runtime_capabilities: false,
                    tool_decision: None,
                    policy_decision: None,
                })
            }
            TransactionType::SubagentSpawn => {
                // Phase 10 schema-v2: the SubagentSpawn audit row is
                // a typed transaction-header variant. The kernel
                // path is identical to the generic catch-all (build
                // a context-hashed record entry); we surface an
                // explicit arm so the closed-enum match remains
                // exhaustive for documentation.
                let entry = RecordEntry::builder(seq, tx.clone())
                    .context_hash(context_hash)
                    .build();
                Ok(ProcessResult {
                    entry,
                    tool_output: None,
                    had_failures: false,
                    runtime_capability_update: None,
                    clear_runtime_capabilities: false,
                    tool_decision: None,
                    policy_decision: None,
                })
            }
            _ => {
                let entry = RecordEntry::builder(seq, tx.clone())
                    .context_hash(context_hash)
                    .build();
                Ok(ProcessResult {
                    entry,
                    tool_output: None,
                    had_failures: false,
                    runtime_capability_update: None,
                    clear_runtime_capabilities: false,
                    tool_decision: None,
                    policy_decision: None,
                })
            }
        }
    }

    pub(super) fn runtime_capability_update_from_tx(
        tx: &Transaction,
    ) -> Result<Option<RuntimeCapabilityInstall>, serde_json::Error> {
        if tx.tx_type != TransactionType::System {
            return Ok(None);
        }

        let Ok(payload) = serde_json::from_slice::<serde_json::Value>(&tx.payload) else {
            return Ok(None);
        };
        let is_capability_install = payload
            .get("system_kind")
            .and_then(serde_json::Value::as_str)
            == Some("capability_install");

        if is_capability_install {
            serde_json::from_value(payload).map(Some)
        } else {
            Ok(None)
        }
    }
}
