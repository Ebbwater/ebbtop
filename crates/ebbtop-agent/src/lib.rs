//! Library interface for ebbtop_agent functionality.

pub mod discovery;
pub mod envfile;
pub mod gpu;
pub mod kill;
pub mod metrics;
pub mod pairing;
pub mod platform;
pub mod probe;
pub mod proc_util;
pub mod proto;
pub mod secret;
pub mod server;
pub mod state;
pub mod types;
pub mod ws;

pub use metrics::{collect_journal_entries, collect_process_metrics};
pub use state::{AppState, AuthScope, CacheEntry};
pub use types::{
    DetailedProcessInfo, JournalEntry, JournalResponse, KillOutcome, LogLevel,
    ProcessMetricsResponse,
};
