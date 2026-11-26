//! Process definitions: create, read, update, delete, and the trigger
//! metadata export.

use super::schedule::{next_run_after, validate_cron, validate_name, validate_prompt};
use super::{
    NewProcess, ProcessError, ProcessRecord, ProcessStore, ProcessTriggerMeta, ProcessUpdate,
};
use chrono::Utc;
use rocksdb::IteratorMode;

impl ProcessStore {
    pub(super) fn put_record(&self, record: &ProcessRecord) -> Result<(), ProcessError> {
        let plain = serde_json::to_vec(record).map_err(|e| ProcessError::Serde(e.to_string()))?;
        let sealed = self.seal_value(plain)?;
        let cf = self.processes_cf()?;
        self.db
            .put_cf(&cf, record.id.as_bytes(), sealed)
            .map_err(|e| ProcessError::Store(e.to_string()))
    }

    /// Create a new process. Validates name, prompt, and cron; computes
    /// the initial `next_run_at` from the cron expression (UTC).
    ///
    /// # Errors
    /// Rejects invalid input; surfaces store / sealing failures.
    pub fn create(&self, input: NewProcess) -> Result<ProcessRecord, ProcessError> {
        validate_name(&input.name)?;
        validate_prompt(&input.prompt)?;
        let now = Utc::now();
        let next_run_at = next_run_after(&input.cron, now)?;

        let record = ProcessRecord {
            id: uuid::Uuid::new_v4().to_string(),
            name: input.name,
            description: input.description,
            cron: input.cron,
            prompt: input.prompt,
            config: input.config,
            enabled: input.enabled,
            created_at: now,
            updated_at: now,
            last_run_at: None,
            next_run_at,
        };
        self.put_record(&record)?;
        Ok(record)
    }

    /// Fetch a process by id.
    ///
    /// # Errors
    /// Surfaces store / unsealing / decoding failures.
    pub fn get(&self, id: &str) -> Result<Option<ProcessRecord>, ProcessError> {
        let cf = self.processes_cf()?;
        match self
            .db
            .get_cf(&cf, id.as_bytes())
            .map_err(|e| ProcessError::Store(e.to_string()))?
        {
            Some(bytes) => {
                let bytes = self.open_value(&bytes)?;
                let record: ProcessRecord = serde_json::from_slice(&bytes)
                    .map_err(|e| ProcessError::Serde(e.to_string()))?;
                Ok(Some(record))
            }
            None => Ok(None),
        }
    }

    /// List all processes (full definitions — in-VM consumers only).
    ///
    /// # Errors
    /// Surfaces store / unsealing / decoding failures.
    pub fn list(&self) -> Result<Vec<ProcessRecord>, ProcessError> {
        let cf = self.processes_cf()?;
        let mut out = Vec::new();
        for item in self.db.iterator_cf(&cf, IteratorMode::Start) {
            let (_, v) = item.map_err(|e| ProcessError::Store(e.to_string()))?;
            let bytes = self.open_value(&v)?;
            let record: ProcessRecord =
                serde_json::from_slice(&bytes).map_err(|e| ProcessError::Serde(e.to_string()))?;
            out.push(record);
        }
        Ok(out)
    }

    /// Apply a partial update. `created_at` and run bookkeeping are
    /// preserved; `updated_at` is refreshed and `next_run_at` is
    /// recomputed from the (possibly new) cron expression.
    ///
    /// # Errors
    /// [`ProcessError::NotFound`] when the id is unknown; rejects
    /// invalid replacement values.
    pub fn update(&self, id: &str, update: ProcessUpdate) -> Result<ProcessRecord, ProcessError> {
        let mut record = self
            .get(id)?
            .ok_or_else(|| ProcessError::NotFound(id.to_string()))?;

        if let Some(name) = update.name {
            validate_name(&name)?;
            record.name = name;
        }
        if let Some(description) = update.description {
            record.description = description;
        }
        if let Some(cron_expr) = update.cron {
            validate_cron(&cron_expr)?;
            record.cron = cron_expr;
        }
        if let Some(prompt) = update.prompt {
            validate_prompt(&prompt)?;
            record.prompt = prompt;
        }
        if let Some(config) = update.config {
            record.config = config;
        }
        if let Some(enabled) = update.enabled {
            record.enabled = enabled;
        }

        let now = Utc::now();
        record.updated_at = now;
        record.next_run_at = next_run_after(&record.cron, now)?;
        self.put_record(&record)?;
        Ok(record)
    }

    /// Delete a process and its entire run history. Returns `true`
    /// when a record existed.
    ///
    /// # Errors
    /// Surfaces store failures.
    pub fn delete(&self, id: &str) -> Result<bool, ProcessError> {
        let existed = self.get(id)?.is_some();
        if existed {
            let cf = self.processes_cf()?;
            self.db
                .delete_cf(&cf, id.as_bytes())
                .map_err(|e| ProcessError::Store(e.to_string()))?;
            // Drop run history.
            let runs_cf = self.runs_cf()?;
            for key in self.run_keys(id)? {
                self.db
                    .delete_cf(&runs_cf, key)
                    .map_err(|e| ProcessError::Store(e.to_string()))?;
            }
        }
        Ok(existed)
    }

    /// Exportable trigger metadata for every process.
    ///
    /// **This is the only process data allowed off-VM.** It carries
    /// `(process_id, cron, enabled, next_run_at)` and structurally
    /// cannot carry the prompt, config, or run history — see
    /// [`ProcessTriggerMeta`]. The next phase registers this with the
    /// swarm gateway's `process_triggers` CF so the external
    /// `ProcessCronService` can fire content-free triggers.
    ///
    /// # Errors
    /// Surfaces store / unsealing / decoding failures.
    pub fn trigger_metadata(&self) -> Result<Vec<ProcessTriggerMeta>, ProcessError> {
        Ok(self
            .list()?
            .into_iter()
            .map(|p| ProcessTriggerMeta {
                process_id: p.id,
                cron: p.cron,
                enabled: p.enabled,
                next_run_at: p.next_run_at,
            })
            .collect())
    }
}
