//! The data layer: read Claude Code transcripts, dedupe, price, bucket into
//! the three limit windows, and aggregate. No Tauri here — `ledger-cli` runs
//! this end to end without a GUI, and that is the acceptance test.

pub mod index;
pub mod pricing;
pub mod record;
pub mod snapshot;
pub mod usageapi;
pub mod usagecache;
pub mod windows;

pub use index::{Index, ScanStats};
pub use pricing::PriceTable;
pub use record::{Rec, Title, Usage};
pub use snapshot::{build_snapshot, Snapshot};
pub use usageapi::FetchError;
pub use usagecache::UsageCache;
pub use windows::{Calibration, Window, WindowKind, WeeklyReset};
