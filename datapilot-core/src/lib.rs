//! DataPilot agent core (ADR-0012 M1). Pure Rust, no async runtime: one thread per running turn, std channels.
//! Shares prompts (Resources/prompts), tool contracts (protocol/tools.snapshot.json), the Python kernel and the
//! transcript / memory file formats with the Swift core.
pub mod agent;
pub mod chrome;
pub mod compact;
pub mod dream;
pub mod events;
pub mod extract;
pub mod index;
pub mod kernel;
pub mod llm;
pub mod mcp;
pub mod newapi;
pub mod memory;
pub mod model_table;
pub mod notes;
pub mod paths;
pub mod permissions;
pub mod platform;
pub mod prompt;
pub mod recall;
pub mod sandbox;
pub mod secrets;
pub mod server;
pub mod session;
pub mod snapshots;
pub mod source_sync;
pub mod ssh_tunnel;
pub mod subagent;
pub mod text;
pub mod tools;
pub mod types;
pub mod ui;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
