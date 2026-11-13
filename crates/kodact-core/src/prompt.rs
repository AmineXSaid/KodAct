//! The system prompt's ingredients, kept for the life of the session so the
//! prompt can be rebuilt when one of them changes: the output style
//! (`/output-style`), the model (`/model`, for the environment section) or
//! an SDK host's `initialize` request.

use std::path::PathBuf;

use kodact_engine::{EnvInfo, SystemPromptOptions};
use kodact_types::SystemBlock;

#[derive(Debug, Clone)]
pub struct PromptSpec {
    /// `--system-prompt`: replaces the default prompt.
    pub replace: Option<String>,
    /// `--append-system-prompt`.
    pub append: Option<String>,
    /// `--agent`: the main agent's instructions.
    pub agent: Option<String>,
    /// Instructions from connected MCP servers.
    pub mcp_instructions: Option<String>,
    /// `KODACT_PROMPTS_DIR`.
    pub prompts_dir: Option<PathBuf>,
    pub exclude_dynamic: bool,
    /// The output style's instructions (none for the default style).
    pub output_style: Option<String>,
    /// `--autonomous`: the unattended-run section.
    pub autonomous: bool,
    /// `prompt.short`: the shorter built-in prompt.
    pub short: bool,
    pub env: EnvInfo,
}

impl PromptSpec {
    pub fn options(&self) -> SystemPromptOptions {
        let parts: Vec<&str> =
            [&self.agent, &self.append, &self.mcp_instructions].into_iter().flatten().map(String::as_str).collect();
        SystemPromptOptions {
            replace: self.replace.clone(),
            append: (!parts.is_empty()).then(|| parts.join("\n\n")),
            prompts_dir: self.prompts_dir.clone(),
            exclude_dynamic: self.exclude_dynamic,
            output_style: self.output_style.clone(),
            autonomous: self.autonomous,
            short: self.short,
        }
    }

    /// The system blocks, and the environment text when it is deferred to the first message.
    pub fn build(&self) -> (Vec<SystemBlock>, Option<String>) {
        kodact_engine::build_system(&self.options(), &self.env)
    }

    /// The model the environment section names.
    pub fn set_model(&mut self, model: &str) {
        self.env.model = model.to_string();
        self.env.model_name =
            kodact_api::models::model_info(model).map(|m| m.display_name.to_string()).unwrap_or_else(|| model.into());
    }
}
