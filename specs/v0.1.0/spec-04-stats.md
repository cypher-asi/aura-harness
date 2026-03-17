# AURA Stats — Spec 04

**Status**: Design-ready  
**Builds on**: spec-01-aura.md, spec-02-interactive-runtime.md  
**Goal**: Comprehensive usage statistics tracking for agents and users

---

## 1) Purpose

Build a statistics collection and storage system that tracks agent and user activity across the AURA platform:

* **Token Accounting** — Track input/output tokens per agent and per user
* **Message Metrics** — Count messages by type and direction
* **Tool Analytics** — Monitor tool usage patterns and success rates
* **Code Impact** — Measure lines of code added, removed, and modified
* **Historical Trends** — Maintain time-series data for analysis

### Why This Matters

1. **Cost Management** — Token usage directly maps to API costs
2. **Usage Insights** — Understand how agents are being used
3. **Performance Monitoring** — Track tool success rates and patterns
4. **Capacity Planning** — Project future resource needs
5. **Billing Foundation** — Per-user/per-agent metering for future monetization

---

## 2) Architecture

### 2.1 Updated Crate Layout

```
aura/
├─ aura-core              # IDs, schemas, hashing (unchanged)
├─ aura-store             # RocksDB storage (unchanged, stats use this)
├─ aura-kernel            # Deterministic kernel (emits stat events)
├─ aura-swarm             # Router, scheduler, workers (unchanged)
├─ aura-reasoner          # Provider interface (emits token usage)
├─ aura-executor          # Executor trait (unchanged)
├─ aura-tools             # ToolExecutor (emits tool stats)
├─ aura-stats             # NEW: Stats collection and aggregation
├─ aura-terminal          # Terminal UI (displays stats)
├─ aura-cli               # CLI (stats commands)
└─ aura-gateway-ts        # DEPRECATED
```

### 2.2 Component Diagram

```
┌─────────────────────────────────────────────────────────────────────────┐
│                            Stat Event Sources                           │
├─────────────────────────────────────────────────────────────────────────┤
│                                                                         │
│  ┌──────────────┐    ┌──────────────┐    ┌──────────────────────────┐  │
│  │ aura-kernel  │    │aura-reasoner │    │     aura-tools           │  │
│  │              │    │              │    │                          │  │
│  │ • Messages   │    │ • Tokens     │    │ • Tool calls             │  │
│  │ • Turns      │    │ • Latency    │    │ • Lines changed          │  │
│  │ • Decisions  │    │ • Model info │    │ • File operations        │  │
│  └──────┬───────┘    └──────┬───────┘    └────────────┬─────────────┘  │
│         │                   │                         │                 │
│         └───────────────────┼─────────────────────────┘                 │
│                             ▼                                           │
│  ┌──────────────────────────────────────────────────────────────────┐  │
│  │                        aura-stats                                 │  │
│  │  ┌─────────────┐   ┌─────────────┐   ┌──────────────────────┐   │  │
│  │  │  Collector  │──►│ Aggregator  │──►│     StatsStore       │   │  │
│  │  │  (async)    │   │ (rollups)   │   │  (RocksDB CFs)       │   │  │
│  │  └─────────────┘   └─────────────┘   └──────────────────────┘   │  │
│  │                                                                   │  │
│  │  ┌─────────────┐   ┌─────────────────────────────────────────┐   │  │
│  │  │   Query     │   │                  API                     │   │  │
│  │  │   Engine    │◄──│  get_agent_stats / get_user_stats / ... │   │  │
│  │  └─────────────┘   └─────────────────────────────────────────┘   │  │
│  └──────────────────────────────────────────────────────────────────┘  │
│                             │                                           │
│                             ▼                                           │
│  ┌──────────────────────────────────────────────────────────────────┐  │
│  │                        Consumers                                  │  │
│  │  • aura-cli (/stats command)                                     │  │
│  │  • aura-terminal (stats panel)                                   │  │
│  │  • HTTP API (/agents/{id}/stats)                                 │  │
│  └──────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## 3) Data Model

### 3.1 Core Types

```rust
// aura-stats/src/types.rs

use aura_core::{AgentId, UserId};
use serde::{Deserialize, Serialize};

/// Unique identifier for a stats event
pub type EventId = [u8; 16];

/// Time bucket granularity for aggregations
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeBucket {
    Minute,
    Hour,
    Day,
    Week,
    Month,
}

/// A raw stat event from any source
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatEvent {
    pub event_id: EventId,
    pub timestamp_ms: u64,
    pub agent_id: AgentId,
    pub user_id: Option<UserId>,
    pub session_id: Option<[u8; 16]>,
    pub kind: StatEventKind,
}

/// Types of stat events
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StatEventKind {
    /// Token usage from model calls
    TokenUsage {
        model: String,
        input_tokens: u32,
        output_tokens: u32,
        cache_read_tokens: Option<u32>,
        cache_write_tokens: Option<u32>,
    },
    
    /// Message sent or received
    Message {
        direction: MessageDirection,
        message_type: MessageType,
        content_bytes: u32,
    },
    
    /// Tool execution
    ToolExecution {
        tool_name: String,
        success: bool,
        duration_ms: u32,
        error_type: Option<String>,
    },
    
    /// Code changes from file operations
    CodeChange {
        operation: CodeOperation,
        file_path: String,
        lines_added: u32,
        lines_removed: u32,
        bytes_changed: u32,
    },
    
    /// Turn completion
    TurnComplete {
        steps: u32,
        total_duration_ms: u64,
        tool_calls: u32,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageDirection {
    Inbound,   // User → Agent
    Outbound,  // Agent → User
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
    UserPrompt,
    AssistantResponse,
    ToolResult,
    System,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeOperation {
    Create,
    Modify,
    Delete,
    Rename,
}
```

### 3.2 Aggregated Stats

```rust
/// Aggregated statistics for a time period
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AggregatedStats {
    /// Time period start (Unix ms)
    pub period_start_ms: u64,
    /// Time period end (Unix ms)
    pub period_end_ms: u64,
    /// Granularity of this aggregation
    pub bucket: TimeBucket,
    
    /// Token statistics
    pub tokens: TokenStats,
    /// Message statistics
    pub messages: MessageStats,
    /// Tool statistics
    pub tools: ToolStats,
    /// Code change statistics
    pub code: CodeStats,
    /// Turn statistics
    pub turns: TurnStats,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenStats {
    /// Total input tokens consumed
    pub input_tokens: u64,
    /// Total output tokens generated
    pub output_tokens: u64,
    /// Total tokens (input + output)
    pub total_tokens: u64,
    /// Cache read tokens (if supported)
    pub cache_read_tokens: u64,
    /// Cache write tokens (if supported)
    pub cache_write_tokens: u64,
    /// Number of model calls
    pub model_calls: u32,
    /// Breakdown by model
    pub by_model: HashMap<String, ModelTokenStats>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelTokenStats {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub calls: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MessageStats {
    /// Total messages
    pub total: u32,
    /// Inbound messages (user → agent)
    pub inbound: u32,
    /// Outbound messages (agent → user)
    pub outbound: u32,
    /// Breakdown by message type
    pub by_type: HashMap<MessageType, u32>,
    /// Total content bytes
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolStats {
    /// Total tool executions
    pub total_calls: u32,
    /// Successful executions
    pub successful: u32,
    /// Failed executions
    pub failed: u32,
    /// Total execution time (ms)
    pub total_duration_ms: u64,
    /// Average execution time (ms)
    pub avg_duration_ms: f64,
    /// Breakdown by tool name
    pub by_tool: HashMap<String, ToolUsageStats>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolUsageStats {
    pub calls: u32,
    pub successful: u32,
    pub failed: u32,
    pub total_duration_ms: u64,
    /// Most common error types
    pub error_counts: HashMap<String, u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CodeStats {
    /// Total lines added
    pub lines_added: u64,
    /// Total lines removed
    pub lines_removed: u64,
    /// Net lines changed (added - removed)
    pub lines_net: i64,
    /// Total bytes changed
    pub bytes_changed: u64,
    /// Files created
    pub files_created: u32,
    /// Files modified
    pub files_modified: u32,
    /// Files deleted
    pub files_deleted: u32,
    /// Breakdown by file extension
    pub by_extension: HashMap<String, ExtensionCodeStats>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExtensionCodeStats {
    pub lines_added: u64,
    pub lines_removed: u64,
    pub files_touched: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TurnStats {
    /// Total turns processed
    pub total_turns: u32,
    /// Total steps across all turns
    pub total_steps: u64,
    /// Average steps per turn
    pub avg_steps_per_turn: f64,
    /// Total processing time (ms)
    pub total_duration_ms: u64,
    /// Average turn duration (ms)
    pub avg_duration_ms: f64,
}
```

### 3.3 Query Types

```rust
/// Query for retrieving stats
#[derive(Debug, Clone)]
pub struct StatsQuery {
    /// Filter by agent (None = all agents)
    pub agent_id: Option<AgentId>,
    /// Filter by user (None = all users)
    pub user_id: Option<UserId>,
    /// Start time (inclusive)
    pub from_ms: Option<u64>,
    /// End time (exclusive)
    pub to_ms: Option<u64>,
    /// Aggregation granularity
    pub bucket: TimeBucket,
    /// Include breakdown details
    pub include_breakdowns: bool,
}

/// Response containing queried stats
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatsResponse {
    pub query: StatsQueryMeta,
    pub stats: Vec<AggregatedStats>,
    pub summary: AggregatedStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatsQueryMeta {
    pub agent_id: Option<String>,
    pub user_id: Option<String>,
    pub from_ms: u64,
    pub to_ms: u64,
    pub bucket: TimeBucket,
}
```

---

## 4) Storage Schema

### 4.1 Column Families

Add to `aura-store` (or use dedicated stats DB):

```rust
// New column families for stats
const CF_STAT_EVENTS: &str = "stat_events";      // Raw events (time-ordered)
const CF_STATS_AGENT: &str = "stats_agent";       // Per-agent aggregations
const CF_STATS_USER: &str = "stats_user";         // Per-user aggregations
const CF_STATS_GLOBAL: &str = "stats_global";     // Global aggregations
```

### 4.2 Key Schemas

```
stat_events (raw events, for replay/debugging):
  Key:   E | timestamp_ms(u64be) | event_id(16)
  Value: StatEvent (CBOR/protobuf)

stats_agent (per-agent time-bucketed aggregations):
  Key:   A | agent_id(32) | bucket(u8) | period_start_ms(u64be)
  Value: AggregatedStats (CBOR/protobuf)

stats_user (per-user time-bucketed aggregations):
  Key:   U | user_id(32) | bucket(u8) | period_start_ms(u64be)
  Value: AggregatedStats (CBOR/protobuf)

stats_global (system-wide aggregations):
  Key:   G | bucket(u8) | period_start_ms(u64be)
  Value: AggregatedStats (CBOR/protobuf)
```

### 4.3 Time Bucket Encoding

```rust
impl TimeBucket {
    pub fn as_u8(&self) -> u8 {
        match self {
            TimeBucket::Minute => 0,
            TimeBucket::Hour => 1,
            TimeBucket::Day => 2,
            TimeBucket::Week => 3,
            TimeBucket::Month => 4,
        }
    }
    
    pub fn bucket_start_ms(&self, timestamp_ms: u64) -> u64 {
        let secs = timestamp_ms / 1000;
        match self {
            TimeBucket::Minute => (secs / 60) * 60 * 1000,
            TimeBucket::Hour => (secs / 3600) * 3600 * 1000,
            TimeBucket::Day => (secs / 86400) * 86400 * 1000,
            TimeBucket::Week => {
                // Start from Unix epoch Monday
                let days = secs / 86400;
                let week_start = (days / 7) * 7;
                week_start * 86400 * 1000
            }
            TimeBucket::Month => {
                // Approximate: 30-day months
                let days = secs / 86400;
                let month_start = (days / 30) * 30;
                month_start * 86400 * 1000
            }
        }
    }
}
```

### 4.4 Retention Policy

```rust
/// Stats retention configuration
#[derive(Debug, Clone)]
pub struct RetentionPolicy {
    /// Keep minute-level data for this duration
    pub minute_retention_hours: u32,   // default: 24
    /// Keep hour-level data for this duration
    pub hour_retention_days: u32,      // default: 7
    /// Keep day-level data for this duration
    pub day_retention_days: u32,       // default: 90
    /// Keep week-level data for this duration
    pub week_retention_days: u32,      // default: 365
    /// Keep month-level data for this duration
    pub month_retention_days: u32,     // default: unlimited (u32::MAX)
    /// Keep raw events for this duration
    pub raw_event_retention_hours: u32, // default: 48
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            minute_retention_hours: 24,
            hour_retention_days: 7,
            day_retention_days: 90,
            week_retention_days: 365,
            month_retention_days: u32::MAX,
            raw_event_retention_hours: 48,
        }
    }
}
```

---

## 5) Collector Implementation

### 5.1 Stat Collector Trait

```rust
// aura-stats/src/collector.rs

use crate::types::StatEvent;
use tokio::sync::mpsc;

/// Interface for components that emit stat events
pub trait StatEmitter: Send + Sync {
    /// Get a sender for emitting stat events
    fn stat_sender(&self) -> mpsc::Sender<StatEvent>;
}

/// The main stats collector that receives events from all sources
pub struct StatsCollector {
    receiver: mpsc::Receiver<StatEvent>,
    store: Arc<dyn StatsStore>,
    aggregator: Aggregator,
    config: CollectorConfig,
}

#[derive(Debug, Clone)]
pub struct CollectorConfig {
    /// Buffer size for incoming events
    pub buffer_size: usize,
    /// Flush interval for batched writes
    pub flush_interval_ms: u64,
    /// Maximum batch size before forced flush
    pub max_batch_size: usize,
    /// Retention policy
    pub retention: RetentionPolicy,
}

impl Default for CollectorConfig {
    fn default() -> Self {
        Self {
            buffer_size: 10_000,
            flush_interval_ms: 1000,
            max_batch_size: 500,
            retention: RetentionPolicy::default(),
        }
    }
}

impl StatsCollector {
    pub fn new(store: Arc<dyn StatsStore>, config: CollectorConfig) -> (Self, mpsc::Sender<StatEvent>) {
        let (sender, receiver) = mpsc::channel(config.buffer_size);
        let aggregator = Aggregator::new();
        
        (Self {
            receiver,
            store,
            aggregator,
            config,
        }, sender)
    }
    
    /// Run the collector loop (spawn as a task)
    pub async fn run(mut self) -> anyhow::Result<()> {
        let mut batch = Vec::with_capacity(self.config.max_batch_size);
        let mut flush_interval = tokio::time::interval(
            Duration::from_millis(self.config.flush_interval_ms)
        );
        
        loop {
            tokio::select! {
                Some(event) = self.receiver.recv() => {
                    batch.push(event);
                    
                    if batch.len() >= self.config.max_batch_size {
                        self.process_batch(&mut batch).await?;
                    }
                }
                _ = flush_interval.tick() => {
                    if !batch.is_empty() {
                        self.process_batch(&mut batch).await?;
                    }
                    
                    // Run periodic cleanup
                    self.cleanup_old_data().await?;
                }
            }
        }
    }
    
    async fn process_batch(&self, batch: &mut Vec<StatEvent>) -> anyhow::Result<()> {
        // 1. Store raw events
        self.store.store_events(batch).await?;
        
        // 2. Update aggregations
        for event in batch.iter() {
            self.aggregator.process_event(event, &self.store).await?;
        }
        
        batch.clear();
        Ok(())
    }
    
    async fn cleanup_old_data(&self) -> anyhow::Result<()> {
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis() as u64;
        
        self.store.cleanup(&self.config.retention, now_ms).await
    }
}
```

### 5.2 Aggregator

```rust
// aura-stats/src/aggregator.rs

pub struct Aggregator {
    // In-memory buffers for current time buckets
    agent_buffers: DashMap<(AgentId, TimeBucket, u64), AggregatedStats>,
    user_buffers: DashMap<(UserId, TimeBucket, u64), AggregatedStats>,
    global_buffers: DashMap<(TimeBucket, u64), AggregatedStats>,
}

impl Aggregator {
    pub fn new() -> Self {
        Self {
            agent_buffers: DashMap::new(),
            user_buffers: DashMap::new(),
            global_buffers: DashMap::new(),
        }
    }
    
    pub async fn process_event(
        &self,
        event: &StatEvent,
        store: &dyn StatsStore,
    ) -> anyhow::Result<()> {
        let ts = event.timestamp_ms;
        
        // Update all time bucket granularities
        for bucket in [TimeBucket::Minute, TimeBucket::Hour, TimeBucket::Day] {
            let period_start = bucket.bucket_start_ms(ts);
            
            // Update agent stats
            self.update_agent_bucket(event.agent_id, bucket, period_start, event);
            
            // Update user stats if present
            if let Some(user_id) = event.user_id {
                self.update_user_bucket(user_id, bucket, period_start, event);
            }
            
            // Update global stats
            self.update_global_bucket(bucket, period_start, event);
        }
        
        Ok(())
    }
    
    fn update_agent_bucket(
        &self,
        agent_id: AgentId,
        bucket: TimeBucket,
        period_start: u64,
        event: &StatEvent,
    ) {
        let key = (agent_id, bucket, period_start);
        let mut stats = self.agent_buffers
            .entry(key)
            .or_insert_with(|| AggregatedStats::new(period_start, bucket));
        
        stats.apply_event(event);
    }
    
    // Similar for user and global...
}

impl AggregatedStats {
    pub fn new(period_start_ms: u64, bucket: TimeBucket) -> Self {
        let period_end_ms = match bucket {
            TimeBucket::Minute => period_start_ms + 60_000,
            TimeBucket::Hour => period_start_ms + 3_600_000,
            TimeBucket::Day => period_start_ms + 86_400_000,
            TimeBucket::Week => period_start_ms + 604_800_000,
            TimeBucket::Month => period_start_ms + 2_592_000_000,
        };
        
        Self {
            period_start_ms,
            period_end_ms,
            bucket,
            ..Default::default()
        }
    }
    
    pub fn apply_event(&mut self, event: &StatEvent) {
        match &event.kind {
            StatEventKind::TokenUsage { model, input_tokens, output_tokens, .. } => {
                self.tokens.input_tokens += *input_tokens as u64;
                self.tokens.output_tokens += *output_tokens as u64;
                self.tokens.total_tokens += (*input_tokens + *output_tokens) as u64;
                self.tokens.model_calls += 1;
                
                let model_stats = self.tokens.by_model
                    .entry(model.clone())
                    .or_default();
                model_stats.input_tokens += *input_tokens as u64;
                model_stats.output_tokens += *output_tokens as u64;
                model_stats.calls += 1;
            }
            
            StatEventKind::Message { direction, message_type, content_bytes } => {
                self.messages.total += 1;
                self.messages.total_bytes += *content_bytes as u64;
                
                match direction {
                    MessageDirection::Inbound => self.messages.inbound += 1,
                    MessageDirection::Outbound => self.messages.outbound += 1,
                }
                
                *self.messages.by_type.entry(*message_type).or_default() += 1;
            }
            
            StatEventKind::ToolExecution { tool_name, success, duration_ms, error_type } => {
                self.tools.total_calls += 1;
                self.tools.total_duration_ms += *duration_ms as u64;
                
                if *success {
                    self.tools.successful += 1;
                } else {
                    self.tools.failed += 1;
                }
                
                let tool_stats = self.tools.by_tool
                    .entry(tool_name.clone())
                    .or_default();
                tool_stats.calls += 1;
                tool_stats.total_duration_ms += *duration_ms as u64;
                
                if *success {
                    tool_stats.successful += 1;
                } else {
                    tool_stats.failed += 1;
                    if let Some(err) = error_type {
                        *tool_stats.error_counts.entry(err.clone()).or_default() += 1;
                    }
                }
                
                // Recalculate average
                self.tools.avg_duration_ms = 
                    self.tools.total_duration_ms as f64 / self.tools.total_calls as f64;
            }
            
            StatEventKind::CodeChange { operation, file_path, lines_added, lines_removed, bytes_changed } => {
                self.code.lines_added += *lines_added as u64;
                self.code.lines_removed += *lines_removed as u64;
                self.code.lines_net += *lines_added as i64 - *lines_removed as i64;
                self.code.bytes_changed += *bytes_changed as u64;
                
                match operation {
                    CodeOperation::Create => self.code.files_created += 1,
                    CodeOperation::Modify => self.code.files_modified += 1,
                    CodeOperation::Delete => self.code.files_deleted += 1,
                    CodeOperation::Rename => self.code.files_modified += 1,
                }
                
                // Extract extension
                if let Some(ext) = std::path::Path::new(file_path)
                    .extension()
                    .and_then(|e| e.to_str())
                {
                    let ext_stats = self.code.by_extension
                        .entry(ext.to_string())
                        .or_default();
                    ext_stats.lines_added += *lines_added as u64;
                    ext_stats.lines_removed += *lines_removed as u64;
                    ext_stats.files_touched += 1;
                }
            }
            
            StatEventKind::TurnComplete { steps, total_duration_ms, tool_calls } => {
                self.turns.total_turns += 1;
                self.turns.total_steps += *steps as u64;
                self.turns.total_duration_ms += *total_duration_ms;
                
                // Recalculate averages
                self.turns.avg_steps_per_turn = 
                    self.turns.total_steps as f64 / self.turns.total_turns as f64;
                self.turns.avg_duration_ms = 
                    self.turns.total_duration_ms as f64 / self.turns.total_turns as f64;
            }
        }
    }
    
    /// Merge another AggregatedStats into this one
    pub fn merge(&mut self, other: &AggregatedStats) {
        // Tokens
        self.tokens.input_tokens += other.tokens.input_tokens;
        self.tokens.output_tokens += other.tokens.output_tokens;
        self.tokens.total_tokens += other.tokens.total_tokens;
        self.tokens.cache_read_tokens += other.tokens.cache_read_tokens;
        self.tokens.cache_write_tokens += other.tokens.cache_write_tokens;
        self.tokens.model_calls += other.tokens.model_calls;
        
        for (model, stats) in &other.tokens.by_model {
            let entry = self.tokens.by_model.entry(model.clone()).or_default();
            entry.input_tokens += stats.input_tokens;
            entry.output_tokens += stats.output_tokens;
            entry.calls += stats.calls;
        }
        
        // Messages
        self.messages.total += other.messages.total;
        self.messages.inbound += other.messages.inbound;
        self.messages.outbound += other.messages.outbound;
        self.messages.total_bytes += other.messages.total_bytes;
        
        for (msg_type, count) in &other.messages.by_type {
            *self.messages.by_type.entry(*msg_type).or_default() += count;
        }
        
        // Tools
        self.tools.total_calls += other.tools.total_calls;
        self.tools.successful += other.tools.successful;
        self.tools.failed += other.tools.failed;
        self.tools.total_duration_ms += other.tools.total_duration_ms;
        
        if self.tools.total_calls > 0 {
            self.tools.avg_duration_ms = 
                self.tools.total_duration_ms as f64 / self.tools.total_calls as f64;
        }
        
        for (tool, stats) in &other.tools.by_tool {
            let entry = self.tools.by_tool.entry(tool.clone()).or_default();
            entry.calls += stats.calls;
            entry.successful += stats.successful;
            entry.failed += stats.failed;
            entry.total_duration_ms += stats.total_duration_ms;
            
            for (err, count) in &stats.error_counts {
                *entry.error_counts.entry(err.clone()).or_default() += count;
            }
        }
        
        // Code
        self.code.lines_added += other.code.lines_added;
        self.code.lines_removed += other.code.lines_removed;
        self.code.lines_net += other.code.lines_net;
        self.code.bytes_changed += other.code.bytes_changed;
        self.code.files_created += other.code.files_created;
        self.code.files_modified += other.code.files_modified;
        self.code.files_deleted += other.code.files_deleted;
        
        for (ext, stats) in &other.code.by_extension {
            let entry = self.code.by_extension.entry(ext.clone()).or_default();
            entry.lines_added += stats.lines_added;
            entry.lines_removed += stats.lines_removed;
            entry.files_touched += stats.files_touched;
        }
        
        // Turns
        self.turns.total_turns += other.turns.total_turns;
        self.turns.total_steps += other.turns.total_steps;
        self.turns.total_duration_ms += other.turns.total_duration_ms;
        
        if self.turns.total_turns > 0 {
            self.turns.avg_steps_per_turn = 
                self.turns.total_steps as f64 / self.turns.total_turns as f64;
            self.turns.avg_duration_ms = 
                self.turns.total_duration_ms as f64 / self.turns.total_turns as f64;
        }
    }
}
```

---

## 6) Stats Store Interface

```rust
// aura-stats/src/store.rs

use async_trait::async_trait;
use crate::types::*;

#[async_trait]
pub trait StatsStore: Send + Sync {
    /// Store raw stat events
    async fn store_events(&self, events: &[StatEvent]) -> anyhow::Result<()>;
    
    /// Store/update aggregated stats for an agent
    async fn store_agent_stats(
        &self,
        agent_id: AgentId,
        bucket: TimeBucket,
        period_start: u64,
        stats: &AggregatedStats,
    ) -> anyhow::Result<()>;
    
    /// Store/update aggregated stats for a user
    async fn store_user_stats(
        &self,
        user_id: UserId,
        bucket: TimeBucket,
        period_start: u64,
        stats: &AggregatedStats,
    ) -> anyhow::Result<()>;
    
    /// Store/update global aggregated stats
    async fn store_global_stats(
        &self,
        bucket: TimeBucket,
        period_start: u64,
        stats: &AggregatedStats,
    ) -> anyhow::Result<()>;
    
    /// Query stats for an agent
    async fn get_agent_stats(
        &self,
        agent_id: AgentId,
        bucket: TimeBucket,
        from_ms: u64,
        to_ms: u64,
    ) -> anyhow::Result<Vec<AggregatedStats>>;
    
    /// Query stats for a user
    async fn get_user_stats(
        &self,
        user_id: UserId,
        bucket: TimeBucket,
        from_ms: u64,
        to_ms: u64,
    ) -> anyhow::Result<Vec<AggregatedStats>>;
    
    /// Query global stats
    async fn get_global_stats(
        &self,
        bucket: TimeBucket,
        from_ms: u64,
        to_ms: u64,
    ) -> anyhow::Result<Vec<AggregatedStats>>;
    
    /// Get current/live stats for an agent (from in-memory aggregator)
    async fn get_agent_current(&self, agent_id: AgentId) -> anyhow::Result<AggregatedStats>;
    
    /// Clean up old data according to retention policy
    async fn cleanup(
        &self,
        policy: &RetentionPolicy,
        current_time_ms: u64,
    ) -> anyhow::Result<()>;
}
```

---

## 7) Integration Points

### 7.1 Kernel Integration

```rust
// In aura-kernel/src/turn_processor.rs

impl<P: ModelProvider, S: Store> TurnProcessor<P, S> {
    pub async fn process_turn(
        &self,
        agent_id: AgentId,
        tx: Transaction,
        stat_tx: &mpsc::Sender<StatEvent>,
    ) -> anyhow::Result<Vec<RecordEntry>> {
        let turn_start = Instant::now();
        let mut step_count = 0u32;
        let mut tool_call_count = 0u32;
        
        // ... existing turn processing logic ...
        
        for step in 0..self.config.max_steps {
            step_count += 1;
            
            // After model call, emit token stats
            let response = self.provider.complete(request).await?;
            
            stat_tx.send(StatEvent {
                event_id: generate_event_id(),
                timestamp_ms: now_ms(),
                agent_id,
                user_id: self.current_user_id,
                session_id: self.session_id,
                kind: StatEventKind::TokenUsage {
                    model: response.trace.model.clone(),
                    input_tokens: response.usage.input_tokens,
                    output_tokens: response.usage.output_tokens,
                    cache_read_tokens: response.usage.cache_read_tokens,
                    cache_write_tokens: response.usage.cache_write_tokens,
                },
            }).await?;
            
            // After tool execution, emit tool stats
            if response.stop_reason == StopReason::ToolUse {
                for result in &tool_results {
                    tool_call_count += 1;
                    
                    stat_tx.send(StatEvent {
                        event_id: generate_event_id(),
                        timestamp_ms: now_ms(),
                        agent_id,
                        user_id: self.current_user_id,
                        session_id: self.session_id,
                        kind: StatEventKind::ToolExecution {
                            tool_name: result.tool.clone(),
                            success: result.ok,
                            duration_ms: result.duration_ms,
                            error_type: result.error.clone(),
                        },
                    }).await?;
                }
            }
        }
        
        // Emit turn completion stats
        stat_tx.send(StatEvent {
            event_id: generate_event_id(),
            timestamp_ms: now_ms(),
            agent_id,
            user_id: self.current_user_id,
            session_id: self.session_id,
            kind: StatEventKind::TurnComplete {
                steps: step_count,
                total_duration_ms: turn_start.elapsed().as_millis() as u64,
                tool_calls: tool_call_count,
            },
        }).await?;
        
        Ok(entries)
    }
}
```

### 7.2 Tool Integration (Code Changes)

```rust
// In aura-tools/src/fs_tools.rs

impl FsTools {
    pub async fn write_file(
        &self,
        path: &Path,
        content: &[u8],
        stat_tx: &mpsc::Sender<StatEvent>,
        agent_id: AgentId,
    ) -> anyhow::Result<()> {
        // Calculate diff
        let (lines_added, lines_removed) = if path.exists() {
            let old_content = fs::read_to_string(path)?;
            calculate_line_diff(&old_content, &String::from_utf8_lossy(content))
        } else {
            (content.lines().count() as u32, 0)
        };
        
        let operation = if path.exists() {
            CodeOperation::Modify
        } else {
            CodeOperation::Create
        };
        
        // Perform write
        fs::write(path, content)?;
        
        // Emit code change stat
        stat_tx.send(StatEvent {
            event_id: generate_event_id(),
            timestamp_ms: now_ms(),
            agent_id,
            user_id: None,
            session_id: None,
            kind: StatEventKind::CodeChange {
                operation,
                file_path: path.to_string_lossy().to_string(),
                lines_added,
                lines_removed,
                bytes_changed: content.len() as u32,
            },
        }).await?;
        
        Ok(())
    }
}

fn calculate_line_diff(old: &str, new: &str) -> (u32, u32) {
    let old_lines: HashSet<_> = old.lines().collect();
    let new_lines: HashSet<_> = new.lines().collect();
    
    let added = new_lines.difference(&old_lines).count() as u32;
    let removed = old_lines.difference(&new_lines).count() as u32;
    
    (added, removed)
}
```

### 7.3 Message Integration

```rust
// In aura-kernel when processing transactions

fn emit_message_stat(
    tx: &Transaction,
    stat_tx: &mpsc::Sender<StatEvent>,
    agent_id: AgentId,
) {
    let (direction, message_type) = match tx.kind {
        TransactionKind::UserPrompt => (MessageDirection::Inbound, MessageType::UserPrompt),
        TransactionKind::AgentMsg => (MessageDirection::Outbound, MessageType::AssistantResponse),
        TransactionKind::ActionResult => (MessageDirection::Inbound, MessageType::ToolResult),
        TransactionKind::System => (MessageDirection::Inbound, MessageType::System),
        _ => return,
    };
    
    let _ = stat_tx.try_send(StatEvent {
        event_id: generate_event_id(),
        timestamp_ms: now_ms(),
        agent_id,
        user_id: extract_user_id(&tx),
        session_id: None,
        kind: StatEventKind::Message {
            direction,
            message_type,
            content_bytes: tx.payload.len() as u32,
        },
    });
}
```

---

## 8) Query API

### 8.1 Stats Query Engine

```rust
// aura-stats/src/query.rs

pub struct StatsQueryEngine {
    store: Arc<dyn StatsStore>,
}

impl StatsQueryEngine {
    pub fn new(store: Arc<dyn StatsStore>) -> Self {
        Self { store }
    }
    
    /// Execute a stats query
    pub async fn query(&self, query: StatsQuery) -> anyhow::Result<StatsResponse> {
        let now_ms = now_ms();
        let from_ms = query.from_ms.unwrap_or(0);
        let to_ms = query.to_ms.unwrap_or(now_ms);
        
        let stats = match (query.agent_id, query.user_id) {
            (Some(agent_id), _) => {
                self.store.get_agent_stats(agent_id, query.bucket, from_ms, to_ms).await?
            }
            (None, Some(user_id)) => {
                self.store.get_user_stats(user_id, query.bucket, from_ms, to_ms).await?
            }
            (None, None) => {
                self.store.get_global_stats(query.bucket, from_ms, to_ms).await?
            }
        };
        
        // Calculate summary by merging all periods
        let mut summary = AggregatedStats::new(from_ms, query.bucket);
        summary.period_end_ms = to_ms;
        for s in &stats {
            summary.merge(s);
        }
        
        // Optionally strip breakdowns for lighter response
        let stats = if query.include_breakdowns {
            stats
        } else {
            stats.into_iter().map(|mut s| {
                s.tokens.by_model.clear();
                s.tools.by_tool.clear();
                s.code.by_extension.clear();
                s.messages.by_type.clear();
                s
            }).collect()
        };
        
        Ok(StatsResponse {
            query: StatsQueryMeta {
                agent_id: query.agent_id.map(|id| hex::encode(id)),
                user_id: query.user_id.map(|id| hex::encode(id)),
                from_ms,
                to_ms,
                bucket: query.bucket,
            },
            stats,
            summary,
        })
    }
    
    /// Get real-time stats for an agent
    pub async fn get_realtime_stats(&self, agent_id: AgentId) -> anyhow::Result<AggregatedStats> {
        self.store.get_agent_current(agent_id).await
    }
    
    /// Get top N agents by token usage
    pub async fn top_agents_by_tokens(
        &self,
        n: usize,
        from_ms: u64,
        to_ms: u64,
    ) -> anyhow::Result<Vec<(AgentId, u64)>> {
        // Implementation would scan stats_agent CF
        todo!()
    }
    
    /// Get top N tools by usage
    pub async fn top_tools(
        &self,
        n: usize,
        from_ms: u64,
        to_ms: u64,
    ) -> anyhow::Result<Vec<(String, u32)>> {
        // Implementation would aggregate from stats
        todo!()
    }
}
```

### 8.2 HTTP API Endpoints

```rust
// In aura-swarm/src/router.rs (add to existing router)

/// Stats endpoints
pub fn stats_routes() -> Router {
    Router::new()
        .route("/stats/global", get(get_global_stats))
        .route("/agents/:agent_id/stats", get(get_agent_stats))
        .route("/users/:user_id/stats", get(get_user_stats))
        .route("/stats/top-agents", get(get_top_agents))
        .route("/stats/top-tools", get(get_top_tools))
}

async fn get_agent_stats(
    Path(agent_id): Path<String>,
    Query(params): Query<StatsQueryParams>,
    State(state): State<AppState>,
) -> impl IntoResponse {
    let agent_id = parse_agent_id(&agent_id)?;
    
    let query = StatsQuery {
        agent_id: Some(agent_id),
        user_id: None,
        from_ms: params.from,
        to_ms: params.to,
        bucket: params.bucket.unwrap_or(TimeBucket::Hour),
        include_breakdowns: params.detailed.unwrap_or(false),
    };
    
    let response = state.stats_engine.query(query).await?;
    
    Json(response)
}

#[derive(Debug, Deserialize)]
struct StatsQueryParams {
    from: Option<u64>,
    to: Option<u64>,
    bucket: Option<TimeBucket>,
    detailed: Option<bool>,
}
```

---

## 9) CLI Integration

### 9.1 Stats Command

```rust
// In aura-cli, add /stats command

/// Stats subcommand
#[derive(Debug, clap::Subcommand)]
pub enum StatsCommand {
    /// Show stats for current agent
    Show {
        /// Time range: 1h, 24h, 7d, 30d
        #[arg(short, long, default_value = "24h")]
        range: String,
        
        /// Show detailed breakdowns
        #[arg(short, long)]
        detailed: bool,
    },
    
    /// Show token usage summary
    Tokens {
        /// Time range
        #[arg(short, long, default_value = "24h")]
        range: String,
    },
    
    /// Show tool usage summary
    Tools {
        /// Time range
        #[arg(short, long, default_value = "24h")]
        range: String,
    },
    
    /// Show code change summary
    Code {
        /// Time range
        #[arg(short, long, default_value = "24h")]
        range: String,
    },
    
    /// Export stats to JSON
    Export {
        /// Output file
        #[arg(short, long)]
        output: PathBuf,
        
        /// Time range
        #[arg(short, long, default_value = "30d")]
        range: String,
    },
}

// Display implementation
fn display_stats(stats: &AggregatedStats, detailed: bool) {
    println!("┌─ STATS ─────────────────────────────────────────────────────────┐");
    println!("│                                                                  │");
    println!("│  TOKENS                                                          │");
    println!("│  ───────                                                         │");
    println!("│  Input:    {:>12} tokens                                   │", 
        format_number(stats.tokens.input_tokens));
    println!("│  Output:   {:>12} tokens                                   │", 
        format_number(stats.tokens.output_tokens));
    println!("│  Total:    {:>12} tokens                                   │", 
        format_number(stats.tokens.total_tokens));
    println!("│  Calls:    {:>12}                                          │", 
        stats.tokens.model_calls);
    println!("│                                                                  │");
    println!("│  MESSAGES                                                        │");
    println!("│  ─────────                                                       │");
    println!("│  Inbound:  {:>12}                                          │", 
        stats.messages.inbound);
    println!("│  Outbound: {:>12}                                          │", 
        stats.messages.outbound);
    println!("│  Total:    {:>12}                                          │", 
        stats.messages.total);
    println!("│                                                                  │");
    println!("│  TOOLS                                                           │");
    println!("│  ─────                                                           │");
    println!("│  Calls:    {:>12}                                          │", 
        stats.tools.total_calls);
    println!("│  Success:  {:>12}                                          │", 
        stats.tools.successful);
    println!("│  Failed:   {:>12}                                          │", 
        stats.tools.failed);
    println!("│  Avg time: {:>12}ms                                        │", 
        stats.tools.avg_duration_ms as u32);
    println!("│                                                                  │");
    println!("│  CODE CHANGES                                                    │");
    println!("│  ────────────                                                    │");
    println!("│  Lines +:  {:>12}                                          │", 
        stats.code.lines_added);
    println!("│  Lines -:  {:>12}                                          │", 
        stats.code.lines_removed);
    println!("│  Net:      {:>+12}                                          │", 
        stats.code.lines_net);
    println!("│  Files:    {:>12}                                          │", 
        stats.code.files_created + stats.code.files_modified);
    println!("│                                                                  │");
    
    if detailed {
        // Show breakdowns
        if !stats.tokens.by_model.is_empty() {
            println!("│  TOKEN BREAKDOWN BY MODEL                                      │");
            for (model, model_stats) in &stats.tokens.by_model {
                println!("│    {:<20} {:>8} in / {:>8} out               │",
                    truncate(model, 20),
                    format_number(model_stats.input_tokens),
                    format_number(model_stats.output_tokens));
            }
            println!("│                                                                  │");
        }
        
        if !stats.tools.by_tool.is_empty() {
            println!("│  TOOL BREAKDOWN                                                │");
            for (tool, tool_stats) in &stats.tools.by_tool {
                let success_rate = if tool_stats.calls > 0 {
                    (tool_stats.successful as f64 / tool_stats.calls as f64) * 100.0
                } else {
                    0.0
                };
                println!("│    {:<20} {:>6} calls ({:>5.1}% success)          │",
                    truncate(tool, 20),
                    tool_stats.calls,
                    success_rate);
            }
            println!("│                                                                  │");
        }
    }
    
    println!("└──────────────────────────────────────────────────────────────────┘");
}
```

---

## 10) Terminal UI Integration

### 10.1 Stats Panel Component

```rust
// In aura-terminal/src/components/stats_panel.rs

use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

pub struct StatsPanel {
    stats: Option<AggregatedStats>,
    compact: bool,
}

impl StatsPanel {
    pub fn new() -> Self {
        Self {
            stats: None,
            compact: false,
        }
    }
    
    pub fn update(&mut self, stats: AggregatedStats) {
        self.stats = Some(stats);
    }
    
    pub fn render(&self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let Some(stats) = &self.stats else {
            return;
        };
        
        let block = Block::default()
            .title(" 📊 STATS ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(theme.muted));
        
        let inner = block.inner(area);
        frame.render_widget(block, area);
        
        if self.compact {
            self.render_compact(frame, inner, stats, theme);
        } else {
            self.render_full(frame, inner, stats, theme);
        }
    }
    
    fn render_compact(&self, frame: &mut Frame, area: Rect, stats: &AggregatedStats, theme: &Theme) {
        let text = format!(
            "Tokens: {} │ Tools: {} │ Lines: +{}/−{}",
            format_compact(stats.tokens.total_tokens),
            stats.tools.total_calls,
            stats.code.lines_added,
            stats.code.lines_removed,
        );
        
        let paragraph = Paragraph::new(text)
            .style(Style::default().fg(theme.muted));
        
        frame.render_widget(paragraph, area);
    }
    
    fn render_full(&self, frame: &mut Frame, area: Rect, stats: &AggregatedStats, theme: &Theme) {
        let lines = vec![
            Line::from(vec![
                Span::styled("Tokens:  ", Style::default().fg(theme.muted)),
                Span::styled(
                    format!("{}", format_number(stats.tokens.total_tokens)),
                    Style::default().fg(theme.primary)
                ),
            ]),
            Line::from(vec![
                Span::styled("Tools:   ", Style::default().fg(theme.muted)),
                Span::styled(
                    format!("{} calls", stats.tools.total_calls),
                    Style::default().fg(theme.secondary)
                ),
            ]),
            Line::from(vec![
                Span::styled("Code:    ", Style::default().fg(theme.muted)),
                Span::styled(
                    format!("+{}", stats.code.lines_added),
                    Style::default().fg(theme.success)
                ),
                Span::raw(" / "),
                Span::styled(
                    format!("−{}", stats.code.lines_removed),
                    Style::default().fg(theme.error)
                ),
            ]),
            Line::from(vec![
                Span::styled("Messages:", Style::default().fg(theme.muted)),
                Span::styled(
                    format!(" {}", stats.messages.total),
                    Style::default().fg(theme.foreground)
                ),
            ]),
        ];
        
        let paragraph = Paragraph::new(lines);
        frame.render_widget(paragraph, area);
    }
}

fn format_number(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn format_compact(n: u64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}
```

### 10.2 Status Bar Integration

Update the status bar to show live token count:

```
┌─ STATUS ────────────────────────────────────────────────────────────────┐
│  ● Ready  │  Tokens: 12.4k/100k  │  Tools: 3 used  │  ⏱ 2.3s last     │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## 11) Crate Structure

```
aura-stats/
├── Cargo.toml
├── src/
│   ├── lib.rs                    # Public API exports
│   ├── types.rs                  # StatEvent, AggregatedStats, etc.
│   ├── collector.rs              # StatsCollector implementation
│   ├── aggregator.rs             # Aggregation logic
│   ├── store.rs                  # StatsStore trait
│   ├── rocks_store.rs            # RocksDB implementation
│   ├── query.rs                  # Query engine
│   └── retention.rs              # Retention policy + cleanup
└── tests/
    ├── collector_tests.rs
    ├── aggregator_tests.rs
    └── query_tests.rs
```

### Cargo.toml

```toml
[package]
name = "aura-stats"
version = "0.1.0"
edition = "2021"
description = "Usage statistics collection and aggregation for AURA OS"
license = "MIT"
rust-version = "1.75"  # Match workspace MSRV

[dependencies]
# Async runtime
tokio = { version = "1.41", features = ["rt", "sync", "time"] }
async-trait = "0.1"

# Serialization
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"

# Storage
rocksdb = "0.22"

# Concurrency
dashmap = "6.0"

# Time
chrono = { version = "0.4", features = ["serde"] }

# Hashing/IDs
uuid = { version = "1.0", features = ["v4"] }
hex = "0.4"

# Error handling
thiserror = "2.0"
anyhow = "1.0"

# Logging/tracing
tracing = "0.1"

# Internal crates
aura-core = { path = "../aura-core" }

[dev-dependencies]
tokio = { version = "1.41", features = ["rt-multi-thread", "macros", "test-util"] }
tempfile = "3.0"
proptest = "1.0"  # Optional: property-based testing

[lints.rust]
unsafe_code = "forbid"

[lints.clippy]
# Enforce strict lints
all = "warn"
pedantic = "warn"
nursery = "warn"

# Allow specific lints where justified
module_name_repetitions = "allow"  # e.g., StatsStats is fine
missing_errors_doc = "warn"        # Upgrade to deny after initial impl
missing_panics_doc = "warn"        # Upgrade to deny after initial impl
```

---

## 12) Implementation Checklist

### Phase 0: Crate Setup & Code Quality Foundation
- [ ] Create `aura-stats` crate with proper `Cargo.toml`
- [ ] Add to workspace `Cargo.toml`
- [ ] Create `src/lib.rs` with module structure
- [ ] Create `src/error.rs` with `thiserror` error types
- [ ] Verify `cargo fmt` passes
- [ ] Verify `cargo clippy --all-targets --all-features -- -D warnings` passes

### Phase 1: Core Types
- [ ] Define `StatEvent` and `StatEventKind` types
- [ ] Define `AggregatedStats` and all sub-types
- [ ] Implement `merge()` for aggregations
- [ ] Implement `TimeBucket` and time bucketing logic
- [ ] Add `#[must_use]` and `const fn` where appropriate
- [ ] Add doc comments with backticks for technical terms
- [ ] Add `# Errors` and `# Panics` sections to public functions

### Phase 2: Storage
- [ ] Define `StatsStore` trait with full documentation
- [ ] Add column families to `aura-store`
- [ ] Implement key encoding/decoding (handle truncation safely)
- [ ] Implement `RocksStatsStore` with proper error context
- [ ] Add retention/cleanup logic with timeouts
- [ ] Use `tokio::task::spawn_blocking` for blocking RocksDB calls

### Phase 3: Collection
- [ ] Implement `StatsCollector` with `tracing` instrumentation
- [ ] Implement `Aggregator` with `DashMap`
- [ ] Add batching and flush logic
- [ ] Add channel-based event ingestion
- [ ] Handle channel backpressure gracefully (log warnings, don't panic)

### Phase 4: Integration
- [ ] Emit token stats from `aura-reasoner`
- [ ] Emit tool stats from `aura-tools`
- [ ] Emit code change stats from fs tools
- [ ] Emit message stats from `aura-kernel`
- [ ] Emit turn completion stats
- [ ] Use `try_send` with logging for non-critical stat emission

### Phase 5: Query API
- [ ] Implement `StatsQueryEngine` with timeouts
- [ ] Add HTTP endpoints to `aura-swarm`
- [ ] Implement top-N queries
- [ ] Add proper error responses (not panics)

### Phase 6: CLI & UI
- [ ] Add `/stats` command to `aura-cli`
- [ ] Add stats panel to `aura-terminal`
- [ ] Update status bar with live tokens

### Phase 7: Testing & Quality Gates
- [ ] Unit tests for aggregation logic
- [ ] Integration tests for store (using `tempfile`)
- [ ] Serialization round-trip tests
- [ ] End-to-end stats collection test
- [ ] Retention policy tests
- [ ] Verify all tests are deterministic (no flaky sleeps)
- [ ] Run full CI checks: `fmt`, `clippy -D warnings`, `test`

---

## 13) Acceptance Criteria

### Code Quality (Must Pass)
- [ ] `cargo fmt --all -- --check` passes with no changes
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes with zero warnings
- [ ] `cargo test --all --all-features` passes with zero failures
- [ ] No `unwrap()` or `expect()` in production code paths
- [ ] All public items have doc comments
- [ ] All `Result`-returning functions document errors
- [ ] No blocking operations on async runtime threads
- [ ] All external calls have timeouts

### Must Have (Functional)
- [ ] Token counts tracked per agent and per user
- [ ] Message counts tracked (inbound/outbound)
- [ ] Tool execution tracked (name, success, duration)
- [ ] Lines of code changed tracked (add/remove/net)
- [ ] Stats persisted to `RocksDB`
- [ ] Stats queryable via API
- [ ] CLI command to view stats
- [ ] Proper error types with `thiserror`
- [ ] Structured logging with `tracing`

### Should Have
- [ ] Time-bucketed aggregations (minute/hour/day)
- [ ] Breakdown by model for tokens
- [ ] Breakdown by tool for tool stats
- [ ] Breakdown by file extension for code stats
- [ ] Real-time stats in terminal UI
- [ ] `const fn` for pure computation functions
- [ ] `#[must_use]` on builder methods

### Nice to Have
- [ ] Historical trends visualization
- [ ] Export to JSON/CSV
- [ ] Top-N queries (agents, tools)
- [ ] Alerts when thresholds exceeded
- [ ] Cost estimation based on token usage
- [ ] Property-based tests with `proptest`

---

## 14) Future Extensions

### Cost Tracking
```rust
pub struct CostStats {
    pub estimated_cost_usd: f64,
    pub by_model: HashMap<String, f64>,
}

impl TokenStats {
    pub fn estimate_cost(&self, pricing: &Pricing) -> f64 {
        let mut cost = 0.0;
        for (model, stats) in &self.by_model {
            if let Some(prices) = pricing.get(model) {
                cost += stats.input_tokens as f64 * prices.input_per_token;
                cost += stats.output_tokens as f64 * prices.output_per_token;
            }
        }
        cost
    }
}
```

### Quotas & Limits
```rust
pub struct Quota {
    pub agent_id: AgentId,
    pub daily_token_limit: u64,
    pub monthly_token_limit: u64,
    pub tool_call_limit_per_turn: u32,
}

pub enum QuotaStatus {
    Ok,
    Warning { usage_percent: f64 },
    Exceeded { by: u64 },
}
```

### Analytics Dashboard
Future web dashboard could display:
- Token usage over time (line chart)
- Tool usage breakdown (pie chart)
- Code impact metrics (bar chart)
- Per-agent comparisons
- Cost projections

---

## 15) Code Quality & Clippy Compliance

This section defines mandatory code quality standards for `aura-stats`. All code must comply with the project's `rules.md` and pass Clippy with warnings denied.

### 15.1 Non-Negotiables

The following are absolute requirements (from `rules.md`):

- Code must compile in CI with **no warnings**
- Formatting must be clean (`cargo fmt`)
- Linting must be clean (`cargo clippy --all-targets --all-features -- -D warnings`)
- Tests must pass (unit + integration)
- No `unsafe` Rust unless explicitly approved, isolated, and tested
- External side effects must be behind explicit boundaries

### 15.2 Clippy Lint Categories

Based on the project's Clippy analysis, watch for these common lint categories:

#### Documentation Lints (`doc_markdown`)

Use backticks for technical terms in doc comments:

```rust
// ❌ Bad
/// Store stats in RocksDB using WriteBatch.

// ✅ Good
/// Store stats in `RocksDB` using `WriteBatch`.
```

Technical terms requiring backticks:
- `RocksDB`, `WriteBatch`, `TimeBucket`
- Field names: `agent_id`, `user_id`, `period_start_ms`
- Type names when referenced in prose

#### Missing Documentation Lints

All public items must have documentation:

```rust
// ✅ Required doc comments
/// Aggregated statistics for a time period.
///
/// # Errors
///
/// Returns an error if the store connection fails or serialization fails.
pub async fn store_agent_stats(...) -> anyhow::Result<()>

/// Creates a new stats collector.
///
/// # Panics
///
/// Panics if `buffer_size` is zero.
pub fn new(buffer_size: usize) -> Self
```

#### Const Function Lints (`missing_const_for_fn`)

Mark functions `const` when they don't require runtime:

```rust
// ❌ Clippy warning
impl TimeBucket {
    pub fn as_u8(&self) -> u8 { ... }
}

// ✅ Good
impl TimeBucket {
    #[must_use]
    pub const fn as_u8(&self) -> u8 { ... }
}
```

Candidates in `aura-stats`:
- `TimeBucket::as_u8()`
- `TimeBucket::bucket_start_ms()` (if pure computation)
- Builder pattern methods like `with_retention()`
- `StatsQueryEngine::new()`

#### Cast Truncation Lints (`cast_possible_truncation`)

Handle potential truncation explicitly:

```rust
// ❌ Clippy warning
let duration_ms = instant.elapsed().as_millis() as u64;

// ✅ Good - use try_from with fallback
let duration_ms = u64::try_from(instant.elapsed().as_millis())
    .unwrap_or(u64::MAX);

// ✅ Good - use saturating conversion
let duration_ms = instant.elapsed().as_millis().min(u64::MAX as u128) as u64;
```

#### Format String Lints (`uninlined_format_args`)

Inline format arguments:

```rust
// ❌ Clippy warning
format!("Agent {} has {} tokens", agent_id, tokens)

// ✅ Good
format!("Agent {agent_id} has {tokens} tokens")
```

#### Option/Result Handling Lints

Use `map_or` instead of `map().unwrap_or()`:

```rust
// ❌ Clippy warning
response.usage.cache_tokens.map(|t| t as u64).unwrap_or(0)

// ✅ Good
response.usage.cache_tokens.map_or(0, |t| t as u64)
```

Use `if let` instead of single-arm match:

```rust
// ❌ Clippy warning
match event.user_id {
    Some(uid) => self.update_user_bucket(uid, ...),
    None => {}
}

// ✅ Good
if let Some(uid) = event.user_id {
    self.update_user_bucket(uid, ...);
}
```

#### Derivable Impls (`derivable_impls`)

Use `#[derive(Default)]` when possible:

```rust
// ❌ Clippy warning
impl Default for TokenStats {
    fn default() -> Self {
        Self {
            input_tokens: 0,
            output_tokens: 0,
            // ... all zeros
        }
    }
}

// ✅ Good
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TokenStats { ... }
```

### 15.3 Error Handling Rules

#### No `unwrap()`/`expect()` in Production

```rust
// ❌ Never in production code
let stats = self.store.get_agent_stats(id).unwrap();

// ✅ Propagate errors
let stats = self.store.get_agent_stats(id)?;

// ✅ Or handle explicitly
let stats = self.store.get_agent_stats(id)
    .map_err(|e| StatsError::StoreFailed { source: e, agent_id: id })?;
```

#### Use `thiserror` for Library Errors

```rust
// aura-stats/src/error.rs

use thiserror::Error;

/// Errors that can occur in stats operations.
#[derive(Error, Debug)]
pub enum StatsError {
    #[error("failed to store stats for agent {agent_id}")]
    StoreFailed {
        agent_id: String,
        #[source]
        source: anyhow::Error,
    },
    
    #[error("failed to deserialize stats: {0}")]
    DeserializationFailed(#[from] serde_json::Error),
    
    #[error("invalid time range: from={from_ms} to={to_ms}")]
    InvalidTimeRange { from_ms: u64, to_ms: u64 },
    
    #[error("agent not found: {0}")]
    AgentNotFound(String),
}
```

#### Context on Fallible Operations

```rust
// ✅ Include relevant context
self.store
    .store_events(batch)
    .await
    .map_err(|e| anyhow::anyhow!(
        "failed to store {count} events for agent {agent_id}: {e}",
        count = batch.len(),
        agent_id = hex::encode(agent_id)
    ))?;
```

### 15.4 Async Conventions

#### Never Block the Runtime

```rust
// ❌ Blocking in async context
async fn process_batch(&self, batch: &[StatEvent]) -> Result<()> {
    std::fs::write("debug.log", format!("{:?}", batch))?; // BLOCKS!
    Ok(())
}

// ✅ Use tokio's async IO
async fn process_batch(&self, batch: &[StatEvent]) -> Result<()> {
    tokio::fs::write("debug.log", format!("{batch:?}")).await?;
    Ok(())
}

// ✅ Or spawn blocking for CPU-heavy work
let aggregated = tokio::task::spawn_blocking(move || {
    heavy_aggregation_computation(&events)
}).await??;
```

#### Timeouts at Boundaries

```rust
// ✅ All external calls must have timeouts
use tokio::time::timeout;

let stats = timeout(
    Duration::from_secs(5),
    self.store.get_agent_stats(agent_id, bucket, from_ms, to_ms)
).await
    .map_err(|_| StatsError::Timeout { operation: "get_agent_stats" })??;
```

### 15.5 Visibility Rules

Default to `pub(crate)`:

```rust
// ❌ Over-exposed
pub struct Aggregator { ... }
pub fn internal_helper() { ... }

// ✅ Minimal public API
pub(crate) struct Aggregator { ... }
pub(crate) fn internal_helper() { ... }

// Only expose what consumers need
pub use types::{StatEvent, AggregatedStats, StatsQuery, StatsResponse};
pub use collector::StatsCollector;
pub use query::StatsQueryEngine;
```

### 15.6 Type Safety

#### Use Newtypes for IDs

```rust
// ✅ Don't pass raw bytes - use domain types from aura-core
use aura_core::{AgentId, UserId};

// If aura-stats needs its own ID types:
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EventId([u8; 16]);

impl EventId {
    #[must_use]
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().into_bytes())
    }
}
```

#### Borrow by Default

```rust
// ❌ Unnecessary clone
pub fn apply_event(&mut self, event: StatEvent) { ... }

// ✅ Take reference
pub fn apply_event(&mut self, event: &StatEvent) { ... }
```

### 15.7 Logging with Tracing

```rust
use tracing::{debug, info, warn, error, instrument};

impl StatsCollector {
    #[instrument(skip(self, batch), fields(batch_size = batch.len()))]
    async fn process_batch(&self, batch: &mut Vec<StatEvent>) -> anyhow::Result<()> {
        debug!("processing stats batch");
        
        self.store.store_events(batch).await.map_err(|e| {
            error!(error = %e, "failed to store events");
            e
        })?;
        
        info!(count = batch.len(), "stats batch processed");
        batch.clear();
        Ok(())
    }
}
```

Log levels:
- `info` — Lifecycle events (collector started, batch flushed)
- `debug` — Detailed operations (individual events, aggregations)
- `warn` — Recoverable issues (retention cleanup failed, will retry)
- `error` — Failures that need attention

### 15.8 Testing Requirements

#### Required Test Coverage

```rust
// tests/aggregator_tests.rs

#[test]
fn test_token_stats_aggregation() {
    let mut stats = AggregatedStats::default();
    
    let event = StatEvent {
        kind: StatEventKind::TokenUsage {
            model: "claude-sonnet-4-20250514".into(),
            input_tokens: 100,
            output_tokens: 50,
            cache_read_tokens: None,
            cache_write_tokens: None,
        },
        ..test_event()
    };
    
    stats.apply_event(&event);
    
    assert_eq!(stats.tokens.input_tokens, 100);
    assert_eq!(stats.tokens.output_tokens, 50);
    assert_eq!(stats.tokens.total_tokens, 150);
    assert_eq!(stats.tokens.model_calls, 1);
}

#[test]
fn test_stats_merge() {
    let mut a = AggregatedStats::default();
    let mut b = AggregatedStats::default();
    
    // ... setup ...
    
    a.merge(&b);
    
    // Verify merged correctly
    assert_eq!(a.tokens.total_tokens, expected_total);
}

#[tokio::test]
async fn test_serialization_roundtrip() {
    let stats = create_test_stats();
    let bytes = serde_json::to_vec(&stats).unwrap();
    let deserialized: AggregatedStats = serde_json::from_slice(&bytes).unwrap();
    
    assert_eq!(stats.tokens.total_tokens, deserialized.tokens.total_tokens);
}
```

#### Deterministic Tests

```rust
// ❌ Non-deterministic (flaky)
#[tokio::test]
async fn test_flush_interval() {
    let collector = StatsCollector::new(...);
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(collector.flushed());
}

// ✅ Deterministic with controlled time
#[tokio::test]
async fn test_flush_interval() {
    tokio::time::pause(); // Control time
    let collector = StatsCollector::new(...);
    tokio::time::advance(Duration::from_secs(2)).await;
    assert!(collector.flushed());
}
```

#### Use `tempfile` for Storage Tests

```rust
#[tokio::test]
async fn test_rocks_stats_store() {
    let temp_dir = tempfile::tempdir().unwrap();
    let store = RocksStatsStore::open(temp_dir.path()).unwrap();
    
    // ... test ...
    
    // temp_dir automatically cleaned up
}
```

### 15.9 Pre-Commit Checklist

Before committing `aura-stats` code:

```bash
# Format
cargo fmt --all

# Lint (must pass with no warnings)
cargo clippy --all-targets --all-features -- -D warnings

# Test
cargo test --all --all-features

# Optional: Check for unused deps
cargo machete
```

### 15.10 Module Documentation Template

Each module must document invariants:

```rust
//! # Stats Aggregator
//!
//! Aggregates raw stat events into time-bucketed summaries.
//!
//! ## Invariants
//!
//! - Events are processed in timestamp order within each bucket
//! - Aggregations are immutable once the bucket period has passed
//! - All numeric fields use saturating arithmetic to prevent overflow
//!
//! ## Thread Safety
//!
//! The aggregator uses `DashMap` for concurrent access. Multiple events
//! can be processed in parallel as long as they target different buckets.
//!
//! ## Failure Modes
//!
//! - If storage fails, events remain in the in-memory buffer
//! - Buffer overflow drops oldest events (logged as warning)
//! - Deserialization errors skip the affected event (logged as error)
```

---

## 16) Summary

`aura-stats` provides comprehensive usage tracking for the AURA platform:

| Metric | Scope | Granularity |
|--------|-------|-------------|
| **Tokens** | Per agent, per user, global | Input/output, by model |
| **Messages** | Per agent, per user | By type, by direction |
| **Tools** | Per agent, global | By tool, success rate |
| **Code** | Per agent | Lines +/−, by extension |
| **Turns** | Per agent | Steps, duration |

The system:
- Collects events asynchronously via channels
- Aggregates into time buckets (minute → month)
- Persists to RocksDB with configurable retention
- Exposes query API for CLI, UI, and HTTP
- Integrates with existing kernel, tools, and reasoner
