//! Library interface for ebbtop_agent functionality.

pub mod gpu;
pub mod kill;
pub mod metrics;
pub mod proto;
pub mod state;
pub mod proc_util;
pub mod types;
pub mod ws;

pub use metrics::{collect_journal_entries, collect_process_metrics};
pub use state::{AppState, CacheEntry};
pub use types::{
    DetailedProcessInfo, JournalEntry, JournalResponse, KillOutcome, LogLevel, ProcessMetricsResponse,
};
