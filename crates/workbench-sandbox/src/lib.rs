//! Cross-platform command sandboxing, process-group lifecycle management, shell invocation helpers,
//! and SSRF-safe public-network host resolution.
pub mod appcontainer;
pub mod confine;
pub mod process;
pub mod public_host;
pub mod sandbox;
pub mod shell;

pub use process::{
    Captured, EndOnDrop, continue_group, force, group_alive, group_of, in_own_group, launch_env,
    leave_session, no_window, no_window_blocking, pause_group, run_captured, terminate,
};
#[cfg(windows)]
pub use process::{process_started, tree_usage};
