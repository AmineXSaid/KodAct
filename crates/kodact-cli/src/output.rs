//! Writing stream-json lines and converting engine events.

use std::io::Write;
use std::sync::Mutex;

use kodact_engine::{EngineEvent, EventSink, NoticeLevel, TurnResult};
use kodact_types::sdk::{
    AssistantMessage, ResultMessage, SdkMessage, StreamEventMessage, SystemMessage, ToolProgress, UserMessage,
    UserPayload,
};
use kodact_types::{new_uuid, MessageContent, Role};
use serde_json::Value;

/// Serialized line output on stdout.
#[derive(Default)]
pub struct Out {
    lock: Mutex<()>,
}

impl Out {
    pub fn line(&self, msg: &SdkMessage) {
        let _g = self.lock.lock().unwrap();
        stdout_line(format_args!("{}", msg.to_line()));
    }
}

/// Write one line to stdout. If the reader is gone (`kodact ... | head`, a host that
/// exited), stop quietly: nobody is left to read the rest. SIGPIPE stays ignored (the
/// Rust default) so that a child that exits before reading its stdin, such as a hook or
/// an MCP server, cannot kill KodAct.
pub fn stdout_line(args: std::fmt::Arguments) {
    let mut so = std::io::stdout().lock();
    let r = so.write_fmt(args).and_then(|_| so.write_all(b"\n")).and_then(|_| so.flush());
    if let Err(e) = r {
        if e.kind() == std::io::ErrorKind::BrokenPipe {
            std::process::exit(crate::exit::OK);
        }
    }
}

pub fn result_message(r: &TurnResult, session_id: &str) -> ResultMessage {
    ResultMessage {
        subtype: r.subtype,
        is_error: r.is_error,
        duration_ms: r.duration_ms,
        duration_api_ms: r.duration_api_ms,
        num_turns: r.num_turns,
        result: r.result.clone().or_else(|| r.prompt_blocked.clone()),
        stop_reason: r.stop_reason.clone(),
        session_id: session_id.to_string(),
        total_cost_usd: r.total_cost_usd,
        usage: r.usage.clone(),
        model_usage: r.model_usage.clone(),
        permission_denials: r.permission_denials.clone(),
        structured_output: r.structured_output.clone(),
        errors: r.errors.clone(),
        verification: r.verification.as_ref().map(|v| {
            let mut record = v.record();
            if !r.missing.is_empty() {
                record["missing"] = serde_json::json!(r.missing);
            }
            record
        }),
        uuid: new_uuid(),
        immediate: None,
    }
}

/// Engine events as stream-json lines.
pub struct StreamSink {
    pub out: std::sync::Arc<Out>,
    /// Set once the session is built (it is not known when the sink is created).
    pub session_id: std::sync::Arc<Mutex<String>>,
    pub include_partial: bool,
    pub replay_user_messages: bool,
    pub verbose: bool,
}

impl EventSink for StreamSink {
    fn emit(&self, event: EngineEvent) {
        let sid = self.session_id.lock().unwrap().clone();
        let msg = match event {
            EngineEvent::Stream { event, parent_tool_use_id } => {
                if !self.include_partial {
                    return;
                }
                SdkMessage::StreamEvent(StreamEventMessage {
                    event,
                    parent_tool_use_id,
                    session_id: sid,
                    uuid: new_uuid(),
                })
            }
            EngineEvent::PromptAccepted { message, uuid } => {
                if !self.replay_user_messages {
                    return;
                }
                SdkMessage::User(UserMessage {
                    message: UserPayload { role: Role::User, content: MessageContent::Blocks(message.content) },
                    parent_tool_use_id: None,
                    session_id: sid,
                    uuid: Some(uuid),
                    is_replay: Some(true),
                    is_synthetic: None,
                    tool_use_result: None,
                    priority: None,
                })
            }
            EngineEvent::Assistant { message, uuid, parent_tool_use_id } => SdkMessage::Assistant(AssistantMessage {
                message,
                parent_tool_use_id,
                session_id: sid,
                uuid,
                error: None,
            }),
            EngineEvent::User { message, uuid, tool_use_result, is_meta, parent_tool_use_id } => {
                SdkMessage::User(UserMessage {
                    message: UserPayload { role: Role::User, content: MessageContent::Blocks(message.content) },
                    parent_tool_use_id,
                    session_id: sid,
                    uuid: Some(uuid),
                    is_replay: None,
                    is_synthetic: is_meta.then_some(true),
                    tool_use_result,
                    priority: None,
                })
            }
            EngineEvent::System { subtype, mut data } => {
                if let Value::Object(m) = &mut data {
                    m.insert("session_id".into(), Value::String(sid));
                    m.insert("uuid".into(), Value::String(new_uuid()));
                }
                SdkMessage::System(SystemMessage::new(&subtype, data))
            }
            EngineEvent::ToolProgress { tool_use_id, tool_name, elapsed_secs } => {
                SdkMessage::ToolProgress(ToolProgress {
                    tool_use_id,
                    tool_name,
                    parent_tool_use_id: None,
                    elapsed_time_seconds: elapsed_secs,
                    session_id: sid,
                    uuid: new_uuid(),
                })
            }
            EngineEvent::Notice { level, text } => {
                if self.verbose || level == NoticeLevel::Error {
                    eprintln!("{text}");
                }
                return;
            }
        };
        self.out.line(&msg);
    }
}

/// Print mode (text / json): diagnostics only, on stderr. Errors are reported
/// once, with the run's result, so error notices are not repeated here.
pub struct QuietSink {
    pub verbose: bool,
    pub quiet: bool,
}

impl EventSink for QuietSink {
    fn emit(&self, event: EngineEvent) {
        let EngineEvent::Notice { level, text } = event else { return };
        let show = match level {
            NoticeLevel::Error => false,
            NoticeLevel::Warning => !self.quiet,
            NoticeLevel::Info => self.verbose,
        };
        if show {
            eprintln!("{} {text}", crate::term::yellow("kodact:"));
        }
    }
}
