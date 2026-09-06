//! PresentMon CSV parsing and process invocation.

pub mod parser;
pub mod present_mode;
pub mod presentmon;
pub mod runner;

pub use parser::{FrameSample, aggregate_metrics, parse_presentmon_csv};
pub use present_mode::{PresentModeCategory, classify_present_mode};
pub use presentmon::{PmArgs, PresentMonController, build_presentmon_args};
