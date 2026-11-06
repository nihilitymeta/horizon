//! The debug overlay's pure helpers: the frame-time window behind the 1% and
//! 0.1% lows, and the process's memory usage.

mod frame_time;
mod memory;

pub use frame_time::FrameTimeWindow;
pub use memory::{MemoryUsage, memory_label, memory_usage};
