//! The app's rules: what an agent may do, with no I/O. `contract/cases` holds the cases they must answer.
pub mod chunks;
pub mod context;
pub mod diff;
pub mod hooks;
pub mod llm;
pub mod permissions;
pub mod policies;
pub mod prompt;
pub mod research;
pub mod skill_files;
pub mod skills;
pub mod toolkits;

pub use workbench_foundation::{hex, js, new_id, plain_path, random_hex, time};

/// A tool call's arguments, in the order the model sent them.
pub type Arguments = serde_json::Map<String, serde_json::Value>;

/// An argument as text, the way JavaScript's `String(value ?? "")` reads the values a tool call holds.
pub(crate) fn argument_text(arguments: &Arguments, key: &str) -> String {
    js::text(arguments.get(key))
}
