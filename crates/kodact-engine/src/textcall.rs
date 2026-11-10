//! Tool calls a model wrote as text (GOALS pillar 4).
//!
//! Small and local models often answer with the call spelled out in their reply instead of as
//! a structured call: `<tool_call>{"name": …, "arguments": {…}}</tool_call>`, or a reply that
//! is nothing but that JSON object. Read as an answer, the turn ends with the work not done and
//! the person looking at a line of JSON. Such a reply is turned back into the call it is.
//!
//! Only when it can't be anything else: the reply has no structured call, the text names a
//! tool that exists, and its arguments are a JSON object. JSON shown inside prose (an example,
//! an explanation) is left as text, and a model that calls tools properly never gets here.

use kodact_types::ContentBlock;
use serde_json::Value;

const OPEN: &str = "<tool_call>";
const CLOSE: &str = "</tool_call>";

/// One call read from text: the tool and its arguments.
fn as_call(v: &Value, known: &dyn Fn(&str) -> bool) -> Option<(String, Value)> {
    let obj = v.as_object()?;
    // `{"function": {"name": …, "arguments": …}}`, as chat-completions spell a call.
    let obj = match obj.get("function").and_then(Value::as_object) {
        Some(f) => f,
        None => obj,
    };
    let name = obj.get("name")?.as_str()?;
    if !known(name) {
        return None;
    }
    let args = ["arguments", "parameters", "input"].iter().find_map(|k| obj.get(*k))?;
    // Arguments as a JSON string, as chat-completions send them.
    let args = match args {
        Value::String(s) => serde_json::from_str::<Value>(s).ok()?,
        other => other.clone(),
    };
    args.is_object().then(|| (name.to_string(), args))
}

/// The text without a Markdown code fence around all of it.
fn unfenced(text: &str) -> &str {
    let t = text.trim();
    let Some(rest) = t.strip_prefix("```") else { return t };
    let Some(body) = rest.strip_suffix("```") else { return t };
    // Drop the language tag on the opening line.
    match body.split_once('\n') {
        Some((tag, inner)) if tag.chars().all(|c| c.is_ascii_alphanumeric()) => inner.trim(),
        _ => t,
    }
}

/// The calls in `text` and the text before them, or `None` when it is an ordinary answer.
fn parse(text: &str, known: &dyn Fn(&str) -> bool) -> Option<(String, Vec<(String, Value)>)> {
    if let Some(first) = text.find(OPEN) {
        let mut calls = vec![];
        let mut rest = &text[first..];
        while let Some(start) = rest.find(OPEN) {
            // Anything between two calls but whitespace makes it prose about calls.
            if !rest[..start].trim().is_empty() {
                return None;
            }
            let body = &rest[start + OPEN.len()..];
            let end = body.find(CLOSE)?;
            calls.push(as_call(&serde_json::from_str(body[..end].trim()).ok()?, known)?);
            rest = &body[end + CLOSE.len()..];
        }
        return (rest.trim().is_empty() && !calls.is_empty()).then(|| (text[..first].trim().to_string(), calls));
    }
    // The whole reply is one call, or a list of them.
    let v: Value = serde_json::from_str(unfenced(text)).ok()?;
    let calls = match &v {
        Value::Array(items) if !items.is_empty() => {
            items.iter().map(|i| as_call(i, known)).collect::<Option<Vec<_>>>()?
        }
        other => vec![as_call(other, known)?],
    };
    Some((String::new(), calls))
}

/// Turn a reply that is a tool call written as text into that call. `known` says whether a
/// tool of that name exists. Returns the tools called, empty when the reply was left as it is.
pub(crate) fn promote(content: &mut Vec<ContentBlock>, known: &dyn Fn(&str) -> bool) -> Vec<String> {
    if content.iter().any(|b| matches!(b, ContentBlock::ToolUse { .. } | ContentBlock::ServerToolUse { .. })) {
        return vec![];
    }
    // One text block carries the answer; several is not a shape this reads.
    let mut texts = content.iter().enumerate().filter_map(|(i, b)| match b {
        ContentBlock::Text { text, .. } if !text.trim().is_empty() => Some((i, text.clone())),
        _ => None,
    });
    let (Some((at, text)), None) = (texts.next(), texts.next()) else { return vec![] };
    let Some((before, calls)) = parse(&text, known) else { return vec![] };
    let names: Vec<String> = calls.iter().map(|(n, _)| n.clone()).collect();
    let mut blocks: Vec<ContentBlock> = vec![];
    if !before.is_empty() {
        blocks.push(ContentBlock::text(before));
    }
    for (name, input) in calls {
        blocks.push(ContentBlock::ToolUse {
            id: format!("textcall_{}", kodact_types::new_uuid().replace('-', "")),
            name,
            input,
            cache_control: None,
            provider: None,
        });
    }
    content.splice(at..=at, blocks);
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn known(name: &str) -> bool {
        matches!(name, "Bash" | "Read")
    }

    fn calls(content: &[ContentBlock]) -> Vec<(String, Value)> {
        content
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { name, input, .. } => Some((name.clone(), input.clone())),
                _ => None,
            })
            .collect()
    }

    fn promoted(text: &str) -> Option<Vec<ContentBlock>> {
        let mut content = vec![ContentBlock::text(text)];
        (!promote(&mut content, &known).is_empty()).then_some(content)
    }

    #[test]
    fn tagged_calls_become_calls_and_keep_the_text_before_them() {
        let c = promoted("I'll list the files.\n<tool_call>\n{\"name\": \"Bash\", \"arguments\": {\"command\": \"ls\"}}\n</tool_call>").unwrap();
        assert_eq!(c[0].as_text(), Some("I'll list the files."));
        assert_eq!(calls(&c), vec![("Bash".to_string(), json!({"command": "ls"}))]);
        // Two calls in one reply, and arguments sent as a JSON string.
        let two = "<tool_call>{\"name\":\"Read\",\"arguments\":\"{\\\"file_path\\\":\\\"a\\\"}\"}</tool_call>\n<tool_call>{\"name\":\"Bash\",\"parameters\":{\"command\":\"pwd\"}}</tool_call>";
        let c = promoted(two).unwrap();
        assert_eq!(calls(&c).len(), 2);
        assert_eq!(calls(&c)[0].1, json!({"file_path": "a"}));
        let ids: Vec<&str> = c
            .iter()
            .filter_map(|b| match b {
                ContentBlock::ToolUse { id, .. } => Some(id.as_str()),
                _ => None,
            })
            .collect();
        assert!(ids[0] != ids[1] && ids.iter().all(|i| i.starts_with("textcall_")));
    }

    #[test]
    fn a_reply_that_is_only_the_call_becomes_the_call() {
        let bare = promoted(r#"{"name": "Bash", "arguments": {"command": "ls"}}"#).unwrap();
        assert_eq!(calls(&bare).len(), 1);
        assert_eq!(bare.len(), 1, "no empty text block is left");
        assert!(promoted("```json\n{\"name\": \"Bash\", \"input\": {\"command\": \"ls\"}}\n```").is_some());
        assert!(promoted(r#"{"function": {"name": "Read", "arguments": "{\"file_path\": \"a\"}"}}"#).is_some());
        assert!(promoted(r#"[{"name": "Read", "arguments": {"file_path": "a"}}]"#).is_some());
    }

    #[test]
    fn an_answer_is_left_as_it_is() {
        for text in [
            "Done. All tests pass.",
            // JSON shown inside prose is an example, not a call.
            "To list files the call is {\"name\": \"Bash\", \"arguments\": {\"command\": \"ls\"}} as shown.",
            // A tool that doesn't exist, arguments that aren't an object, JSON that is data.
            r#"{"name": "Teleport", "arguments": {"to": "mars"}}"#,
            r#"{"name": "Bash", "arguments": "ls"}"#,
            r#"{"name": "Bash"}"#,
            r#"{"users": 3, "name": "Bash"}"#,
            // Tags with prose between or after them, or cut off.
            "<tool_call>{\"name\":\"Bash\",\"arguments\":{\"command\":\"ls\"}}</tool_call> and then I will read it",
            "<tool_call>{\"name\":\"Bash\",\"arguments\":{\"command\":\"ls\"}}",
            "<tool_call>not json</tool_call>",
            "",
        ] {
            assert!(promoted(text).is_none(), "{text}");
        }
        // A reply that already has a structured call is never touched.
        let mut content = vec![
            ContentBlock::text(r#"{"name": "Bash", "arguments": {"command": "ls"}}"#),
            ContentBlock::ToolUse {
                id: "t1".into(),
                name: "Read".into(),
                input: json!({"file_path": "a"}),
                cache_control: None,
                provider: None,
            },
        ];
        let before = content.clone();
        assert!(promote(&mut content, &known).is_empty());
        assert_eq!(content, before);
    }

    #[test]
    fn reasoning_before_the_call_is_kept() {
        let mut content = vec![
            ContentBlock::Thinking { thinking: "plan".into(), signature: String::new() },
            ContentBlock::text(r#"{"name": "Bash", "arguments": {"command": "ls"}}"#),
        ];
        assert_eq!(promote(&mut content, &known), vec!["Bash".to_string()]);
        assert!(matches!(content[0], ContentBlock::Thinking { .. }));
        assert!(matches!(content[1], ContentBlock::ToolUse { .. }));
    }
}
