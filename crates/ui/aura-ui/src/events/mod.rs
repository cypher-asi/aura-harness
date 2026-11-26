//! UI event types for communication between UI and kernel.
//!
//! The event system uses channels for bidirectional communication:
//! - `UiEvent`: Events from the UI to the kernel (user actions)
//! - `UiCommand`: Commands from the kernel to the UI (state updates)

mod command;
mod event;
mod notice;
#[cfg(test)]
mod tests;
mod views;

pub use crate::benchmarks::{
    arm_letter, cost_per_fix, cost_shares, per_fix_cells, run_order, unpriced_note, BenchArm,
    BenchBaseline, BenchCost, BenchDoing, BenchExample, BenchJudge, BenchLiveRun, BenchPhase,
    BenchProgress, BenchRequest, BenchRun, BenchSession, BenchSpec, BenchStatus, BenchTaskCell,
    BenchTaskRow, BenchTaskSetup, BenchTokens, BenchTurn, BenchVerdict, BenchView, BenchWork,
    UNPRICED_MARK,
};
pub use crate::flow::{FlowBench, FlowKind, FlowLane, FlowNode, FlowUsage};
pub use crate::run_log::{
    LogField, LogRequest, LogRunView, LogSection, LogStatus, LogStepView, LogTurnView, LogView,
};
pub use command::UiCommand;
pub use event::{RewindScope, UiEvent};
pub use notice::{ERROR_PREFIX, SUCCESS_PREFIX, WARNING_PREFIX};
pub use views::agents::{
    empty_projects, starts_project, AgentDefinitionSummary, AgentSummary, ProjectSummary,
};
pub use views::boot::{BootHistory, BootLogLine, BootPlanStep, BootStageState, BootUpdate};
pub use views::channels::{
    merge_timeline, nest_members, ChannelPostAuthor, ChannelPostView, ChannelProjectView,
    ChannelSummary, TimelineItem,
};
pub use views::hud::{
    ContextView, InstructionFileView, InstructionStatus, JobView, QueuedItemView, TodoState,
    TodoView,
};
pub use views::mail::MailView;
pub use views::messages::{
    ActivityData, FileDiffData, ImageAttachment, MessageData, MessageRole, ModelOptionView,
    ToolData,
};
pub use views::plans::{PlanView, TaskState, TaskView};
pub use views::policies::{PolicyView, ToolPolicyState, ToolPolicyView};
pub use views::processes::{ProcessKind, ProcessStatus, ProcessSummary};
pub use views::records::{PolicyDetail, RecordStatus, RecordSummary, MAX_RECORDS};
pub use views::trace::{TraceNode, TraceNodeStatus};
pub use views::usage::{CallStats, CallUsage, SessionTotals, SourceUsage};
