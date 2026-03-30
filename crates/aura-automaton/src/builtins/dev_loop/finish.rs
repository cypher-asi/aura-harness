use super::*;

impl DevLoopAutomaton {
    pub(super) async fn finish(
        &self,
        ctx: &mut TickContext,
    ) -> Result<TickOutcome, AutomatonError> {
        let completed: u32 = ctx.state.get(STATE_COMPLETED_COUNT).unwrap_or(0);
        let failed: u32 = ctx.state.get(STATE_FAILED_COUNT).unwrap_or(0);

        let outcome = if failed > 0 {
            "all_tasks_blocked"
        } else {
            "all_tasks_complete"
        };

        info!(outcome, completed, failed, "Dev loop finished");

        ctx.emit(AutomatonEvent::LoopFinished {
            outcome: outcome.into(),
            completed_count: completed,
            failed_count: failed,
        });
        ctx.state.set(STATE_LOOP_FINISHED, &true);

        Ok(TickOutcome::Done)
    }
}
