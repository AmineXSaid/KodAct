//! The query engine.
//!
//! [`Engine::submit`] runs one user turn: it streams the model's reply,
//! executes tool calls (contract C2), loops until the model stops, and returns
//! a [`TurnResult`]. Interrupts (C3), fallback models (C6) and budgets (C7)
//! are handled here. Everything the engine does is reported to an
//! [`EventSink`]; permission prompts go to a [`PermissionPrompter`].

mod diagnose;
mod effort_loop;
mod engine;
mod events;
mod exec;
pub mod finish;
pub mod format;
pub(crate) mod held;
mod limits;
pub mod lsp;
mod named;
pub mod nudges;
mod prefix;
pub mod prompts;
mod repeat;
pub mod request;
mod shell_edit;
mod snapshot;
mod stuck;
mod textcall;
pub mod verify;

pub use engine::{
    AgentRegistration, CompactInfo, Engine, EngineConfig, EngineError, EngineHandle, EngineParts, Pricing, PromptPoint,
    Runtime, Steer, TimeLimit, TurnResult, TurnState, FAST_MODE_BETA, STEER_REMINDER,
};
pub use events::{
    DenyPrompter, EngineEvent, EventSink, ForwardSink, NoticeLevel, NullSink, PermissionAnswer, PermissionPrompt,
    PermissionPrompter, SerializedPrompter, VecSink,
};
pub use lsp::{LspConfig, ServerSpec};
pub use nudges::NudgeConfig;
pub use prompts::{build_system, EnvInfo, SystemPromptOptions};
pub use snapshot::{side_question_request, EngineSnapshot, TurnProgress};
pub use verify::{detect_checks, Verified, VerifyConfig};

/// Synthetic user text recorded when a turn is interrupted (contract C3).
pub const INTERRUPT_MARKER: &str = "[Request interrupted by user]";
pub const INTERRUPT_MARKER_TOOLS: &str = "[Request interrupted by user for tool use]";

#[cfg(test)]
mod tests;
