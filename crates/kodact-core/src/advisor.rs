//! The Advisor tool (`/advisor <model>`): a second model the main one can
//! consult at key moments. It sees the conversation so far (read from the
//! transcript) and answers one question. Its cost counts toward the session,
//! including `--max-budget-usd`.

use std::sync::{Arc, RwLock};

use kodact_api::Provider;
use kodact_tools::{Tool, ToolContext, ToolOutput};
use kodact_types::{ContentBlock, Message, MessagesRequest, SystemBlock};
use serde_json::{json, Value};

/// The advisor model, while one is set. Shared by the tool and `/advisor`.
pub type AdvisorCell = Arc<RwLock<Option<String>>>;

const ADVISOR_PROMPT: &str = "You are an experienced software engineer advising a coding agent partway \
through a task. You get the conversation so far (the user's requests, the agent's messages, its tool calls and \
the end of each result) and the agent's question. Answer in a few short paragraphs or a list:\n- the risks or \
mistakes you see in what it has done or plans to do;\n- the best next step;\n- what it should check before calling \
the work done.\nBe concrete: name files, commands and cases. If the transcript doesn't show enough to judge, say what \
to look at. Don't restate the conversation.";

pub struct Advisor {
    pub provider: Arc<dyn Provider>,
    pub model: AdvisorCell,
}

#[async_trait::async_trait]
impl Tool for Advisor {
    fn name(&self) -> &str {
        "Advisor"
    }

    fn description(&self) -> String {
        "Ask a second, independent model for advice. It sees this conversation so far (messages, tool calls and \
         results) and answers your question with risks, the best next step and what to verify.\n\n\
         Call it at key moments: before a large or risky change, when you are stuck after two failed attempts, and \
         before you call a complex task done. Ask one specific question. Its answer is advice: weigh it against \
         what you know, and don't call it for routine steps (each call costs a request)."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {"question": {"type": "string", "description": "What you want advice on"}},
            "required": ["question"]
        })
    }

    fn is_read_only(&self, _: &Value) -> bool {
        true
    }

    fn is_enabled(&self) -> bool {
        self.model.read().unwrap().is_some()
    }

    async fn call(&self, input: Value, ctx: &ToolContext) -> ToolOutput {
        let Some(model) = self.model.read().unwrap().clone() else {
            return ToolOutput::error("No advisor is set (/advisor <model>).");
        };
        let question = input["question"].as_str().unwrap_or_default().trim().to_string();
        let transcript = ctx
            .transcript_path
            .as_deref()
            .and_then(|p| kodact_session::LoadedSession::load(p, None).ok())
            .map(|l| crate::goal::evaluator_transcript(&l.messages.into_iter().map(|e| e.message).collect::<Vec<_>>()))
            .unwrap_or_default();
        let context = if transcript.is_empty() {
            "(The conversation isn't available: session persistence is off.)".to_string()
        } else {
            transcript
        };
        let req = MessagesRequest {
            model: model.clone(),
            max_tokens: 2_000,
            messages: vec![Message::user(vec![ContentBlock::text(format!(
                "Conversation so far:\n{context}\n\nThe agent asks:\n{question}"
            ))])],
            system: vec![SystemBlock::text(ADVISOR_PROMPT)],
            tools: vec![],
            tool_choice: None,
            thinking: None,
            temperature: None,
            metadata: None,
            output_config: None,
            speed: None,
            stream: true,
            betas: vec![],
            carry_reasoning: false,
        };
        let msg = match kodact_api::complete(self.provider.as_ref(), req, &ctx.cancel).await {
            Ok(m) => m,
            Err(e) => return ToolOutput::error(format!("The advisor couldn't answer: {}", e.describe())),
        };
        let answer = msg.content.iter().filter_map(|b| b.as_text()).collect::<Vec<_>>().join("").trim().to_string();
        let text = if answer.is_empty() { "(the advisor gave no answer)".to_string() } else { answer };
        // The engine prices the request with its own table, `modelPricing` included.
        let side = json!({"model": model, "usage": msg.usage});
        ToolOutput::text(text).with_structured(json!({"advisorModel": model, "sideUsage": side}))
    }
}
