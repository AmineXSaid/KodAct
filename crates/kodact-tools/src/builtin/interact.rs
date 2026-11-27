//! Tools that talk to the person: AskUserQuestion, EnterPlanMode and
//! ExitPlanMode. Each always asks (contract C8). The answer comes back in
//! the call's input (`answers`), and the engine switches permission modes
//! for the plan tools.

use serde_json::{json, Value};

use crate::{Tool, ToolContext, ToolOutput};

/// One parsed `AskUserQuestion` question, normalized across model call shapes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedQuestion {
    pub question: String,
    pub header: String,
    /// `(label, description)` pairs; may be empty for an open-ended question.
    pub options: Vec<(String, String)>,
    pub multi: bool,
}

fn parse_option(v: &Value) -> Result<(String, String), String> {
    match v {
        Value::String(s) => {
            let label = s.trim();
            if label.is_empty() {
                return Err("each option needs a non-empty label".into());
            }
            Ok((label.to_string(), String::new()))
        }
        Value::Object(o) => {
            let label = o
                .get("label")
                .or_else(|| o.get("title"))
                .or_else(|| o.get("value"))
                .or_else(|| o.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if label.is_empty() {
                return Err("each option needs a non-empty label".into());
            }
            let desc = o
                .get("description")
                .or_else(|| o.get("detail"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            Ok((label.to_string(), desc))
        }
        _ => Err("each option must be a string or an object with a label".into()),
    }
}

fn parse_one_question(v: &Value) -> Result<ParsedQuestion, String> {
    match v {
        Value::String(s) => {
            let question = s.trim();
            if question.is_empty() {
                return Err("each question needs its text".into());
            }
            Ok(ParsedQuestion {
                question: question.to_string(),
                header: String::new(),
                options: Vec::new(),
                multi: false,
            })
        }
        Value::Object(q) => {
            let question = q
                .get("question")
                .or_else(|| q.get("prompt"))
                .or_else(|| q.get("text"))
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim();
            if question.is_empty() {
                return Err("each question needs its text".into());
            }
            let header = q.get("header").and_then(Value::as_str).unwrap_or("").trim().to_string();
            let multi = q
                .get("multiSelect")
                .or_else(|| q.get("multi_select"))
                .or_else(|| q.get("multiple"))
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let options = match q.get("options").or_else(|| q.get("choices")) {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(arr)) => arr.iter().map(parse_option).collect::<Result<Vec<_>, _>>()?,
                Some(Value::String(s)) => {
                    let arr: Vec<Value> =
                        serde_json::from_str(s.trim()).map_err(|_| "options must be a list".to_string())?;
                    arr.iter().map(parse_option).collect::<Result<Vec<_>, _>>()?
                }
                Some(_) => return Err("options must be a list".into()),
            };
            Ok(ParsedQuestion { question: question.to_string(), header, options, multi })
        }
        _ => Err("each question must be an object with a question field".into()),
    }
}

/// Parse and validate the questions in an `AskUserQuestion` call.
///
/// Accepts the standard `{"questions": [...]}` list as well as common shapes
/// non-Messages models send: a JSON-encoded string list, a single top-level
/// `{"question": "...", "options": [...]}`, string options (`["SQLite", "Postgres"]`),
/// open-ended questions with no options, and lists with more than four options.
pub fn parse_questions(input: &Value) -> Result<Vec<ParsedQuestion>, String> {
    let raw_list: Vec<Value> = match input.get("questions") {
        Some(Value::Array(arr)) => arr.clone(),
        Some(Value::String(s)) => {
            serde_json::from_str::<Vec<Value>>(s.trim()).map_err(|_| "questions must be a list".to_string())?
        }
        Some(o @ Value::Object(_)) => vec![o.clone()],
        Some(_) => return Err("questions must be a list".into()),
        None if input.get("question").or_else(|| input.get("prompt")).is_some() => vec![input.clone()],
        None => return Err("questions must be a list".into()),
    };
    if raw_list.is_empty() {
        return Err("ask at least 1 question".into());
    }
    raw_list.iter().map(parse_one_question).collect()
}

pub struct AskUserQuestion;

#[async_trait::async_trait]
impl Tool for AskUserQuestion {
    fn name(&self) -> &str {
        "AskUserQuestion"
    }

    fn description(&self) -> String {
        "Ask the user multiple-choice or short-answer questions when a decision is genuinely theirs: requirements that \
         are ambiguous, a choice between approaches with real trade-offs, or preferences you cannot infer.\n\n\
         - Give `questions` as a list of question objects, each with `question` and `options` (`label` and optional \
           `description`); the user can always type their own answer instead.\n\
         - Put the recommended option first and add \"(Recommended)\" to its label.\n\
         - Set `multiSelect` when several options can apply.\n\
         - Do not ask what you can find out from the code, and do not use it to ask for approval of a plan \
           (ExitPlanMode does that)."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "minItems": 1,
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": {"type": "string", "description": "The full question, ending with ?"},
                            "header": {"type": "string", "description": "A short label (max 12 characters)"},
                            "options": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "label": {"type": "string"},
                                        "description": {"type": "string"}
                                    },
                                    "required": ["label"]
                                }
                            },
                            "multiSelect": {"type": "boolean", "default": false}
                        },
                        "required": ["question"]
                    }
                }
            },
            "required": ["questions"]
        })
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn needs_user(&self) -> bool {
        true
    }

    fn validate(&self, input: &Value, _ctx: &ToolContext) -> Result<(), String> {
        parse_questions(input).map(|_| ())
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> ToolOutput {
        let Some(answers) = input.get("answers").and_then(Value::as_object).filter(|a| !a.is_empty()) else {
            return ToolOutput::error(
                "No answers were given. Proceed with your best judgement and state your assumptions.",
            );
        };
        let lines: Vec<String> = answers
            .iter()
            .map(|(q, a)| format!("\"{q}\" = \"{}\"", a.as_str().map(str::to_string).unwrap_or_else(|| a.to_string())))
            .collect();
        ToolOutput::text(format!("The user answered: {}. Continue with these answers in mind.", lines.join(", ")))
            .with_structured(json!({"questions": input.get("questions"), "answers": answers}))
    }
}

pub struct EnterPlanMode;

#[async_trait::async_trait]
impl Tool for EnterPlanMode {
    fn name(&self) -> &str {
        "EnterPlanMode"
    }

    fn description(&self) -> String {
        "Switch to plan mode before a large or risky change: explore read-only and design the approach, then \
         present it with ExitPlanMode for the user's approval. Asks the user first. Skip it for small, clear tasks."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({"type": "object", "properties": {}, "additionalProperties": false})
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn needs_user(&self) -> bool {
        true
    }

    async fn call(&self, _input: Value, _ctx: &ToolContext) -> ToolOutput {
        ToolOutput::text(
            "Plan mode is on. Explore with read-only tools, design the approach, and present it with ExitPlanMode; \
             do not change files until the plan is approved.",
        )
        .with_structured(json!({"mode": "plan"}))
    }
}

pub struct ExitPlanMode;

#[async_trait::async_trait]
impl Tool for ExitPlanMode {
    fn name(&self) -> &str {
        "ExitPlanMode"
    }

    fn description(&self) -> String {
        "In plan mode, present your implementation plan for approval. Give the whole plan in `plan` (Markdown): \
         the steps, the files to change, and how you will verify the result. If the user approves, plan mode ends \
         and you can start; if not, revise the plan. Use it only to approve a plan for changing code, not to ask \
         questions (use AskUserQuestion)."
            .into()
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {"plan": {"type": "string", "description": "The plan, in Markdown"}},
            "required": ["plan"],
            "additionalProperties": false
        })
    }

    fn is_read_only(&self, _input: &Value) -> bool {
        true
    }

    fn needs_user(&self) -> bool {
        true
    }

    async fn call(&self, input: Value, _ctx: &ToolContext) -> ToolOutput {
        let plan = input.get("plan").and_then(Value::as_str).unwrap_or_default();
        ToolOutput::text("The user approved the plan and plan mode is off. Start the implementation, keeping the task list up to date.")
            .with_structured(json!({"plan": plan, "mode": "default"}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ask_user_question_accepts_model_shapes_and_rejects_blank_entries() {
        let ctx = ToolContext::new(std::path::Path::new("."));
        let tool = AskUserQuestion;

        // Standard shape, plus >4 options (as /fewer-permission-prompts produces).
        let six = json!({"questions": [{
            "question": "Which rules should be added?",
            "multiSelect": true,
            "options": [
                {"label": "Bash(cargo test *)", "description": "run tests"},
                {"label": "Bash(git status)", "description": "check status"},
                {"label": "Bash(git diff *)", "description": "inspect changes"},
                {"label": "Bash(rg *)", "description": "search"},
                {"label": "Read", "description": "read files"},
                {"label": "Edit(crates/**)"}
            ]
        }]});
        assert!(tool.validate(&six, &ctx).is_ok());
        let parsed = parse_questions(&six).unwrap();
        assert_eq!(parsed[0].options.len(), 6);
        assert!(parsed[0].multi);
        assert_eq!(parsed[0].options[5], ("Edit(crates/**)".into(), String::new()));

        // String options, snake_case multi_select, and open-ended (0-option) questions.
        let loose = json!({
            "question": "Which database?",
            "multi_select": true,
            "options": ["Postgres", "SQLite"]
        });
        let p = parse_questions(&loose).unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].question, "Which database?");
        assert!(p[0].multi);
        assert_eq!(p[0].options, vec![("Postgres".into(), String::new()), ("SQLite".into(), String::new())]);

        let open_ended = json!({"questions": [{"question": "What should the service be named?"}]});
        let p = parse_questions(&open_ended).unwrap();
        assert!(p[0].options.is_empty());

        // Stringified JSON questions list from OpenAI-compatible gateways.
        let encoded = json!({"questions": r#"[{"question":"Port?","options":["8080","3000"]}]"#});
        let p = parse_questions(&encoded).unwrap();
        assert_eq!(p[0].options.len(), 2);

        // Schema has no empty-properties `answers` object that Gemini's `parameters` would reject.
        assert!(tool.input_schema()["properties"].get("answers").is_none());

        // Blank questions, empty lists, and blank option labels are rejected.
        assert!(tool.validate(&json!({"questions": []}), &ctx).is_err());
        assert!(tool.validate(&json!({"questions": [{"question": "   "}]}), &ctx).is_err());
        assert!(tool.validate(&json!({"questions": [{"question": "Which?", "options": [""]}]}), &ctx).is_err());
        assert!(tool
            .validate(&json!({"questions": [{"question": "Which?", "options": [{"label": " "}]}]}), &ctx)
            .is_err());
    }
}
