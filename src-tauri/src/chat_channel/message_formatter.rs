use super::i18n::{self, Lang};
use super::tool_detail;
use super::types::{MessageLevel, RichMessage};

pub fn format_turn_complete(agent_type: &str, stop_reason: &str, lang: Lang) -> RichMessage {
    let reason = match stop_reason {
        "end_turn" => i18n::stop_reason_end_turn(lang),
        "cancelled" => i18n::stop_reason_cancelled(lang),
        _ => stop_reason,
    };
    RichMessage::info(i18n::turn_complete_body(lang, agent_type))
        .with_title(i18n::turn_complete_title(lang))
        .with_field(i18n::stop_reason_label(lang), reason)
}

pub fn format_agent_error(agent_type: &str, message: &str, lang: Lang) -> RichMessage {
    RichMessage {
        title: Some(i18n::agent_error_title(lang).to_string()),
        body: i18n::agent_error_body(lang, agent_type),
        fields: vec![(
            i18n::error_message_label(lang).to_string(),
            message.to_string(),
        )],
        level: MessageLevel::Error,
    }
}

/// Build the global-event-push notification for an agent permission request.
///
/// This is the passive, notification-only surface for sessions NOT initiated
/// from a chat channel (desktop / web): it tells the user an agent is blocked
/// waiting for approval and to act in Codeg. Chat-channel-initiated sessions
/// keep their interactive `/approve`,`/deny` flow in `session_event_subscriber`
/// and are suppressed here by the event subscriber (see `process_envelope`).
///
/// `tool_call` is the raw ACP tool-call object; the requested operation is
/// rendered with the same shared detail formatter the session relay uses, so a
/// `Bash` / `Write` / `Read` reads identically across both surfaces.
pub fn format_permission_request(tool_call: &serde_json::Value, lang: Lang) -> RichMessage {
    let tool_title = tool_call
        .get("title")
        .and_then(|v| v.as_str())
        .or_else(|| tool_call.get("tool_name").and_then(|v| v.as_str()))
        .unwrap_or("Unknown tool");

    let raw_input = tool_call
        .get("rawInput")
        .or_else(|| tool_call.get("raw_input"))
        .and_then(|v| match v {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Null => None,
            other => Some(other.to_string()),
        });

    let tool_desc = tool_detail::format_tool_call_detail(tool_title, raw_input.as_deref());

    RichMessage {
        title: Some(i18n::permission_request_title(lang).to_string()),
        body: i18n::permission_request_body(lang).to_string(),
        fields: vec![(
            i18n::permission_operation_label(lang).to_string(),
            tool_desc,
        )],
        level: MessageLevel::Warning,
    }
}

/// Build the "user message" notification for a prompt the user submitted from
/// the Codeg conversation UI. `text_preview` is the already-bounded message
/// text (see `ConnectionManager::send_prompt_linked`); it becomes the body so a
/// channel / webhook consumer sees what was sent.
pub fn format_user_prompt_sent(text_preview: &str, lang: Lang) -> RichMessage {
    RichMessage::info(text_preview.to_string()).with_title(i18n::user_message_title(lang))
}

pub struct DailyReportData {
    pub date: String,
    pub conversations_by_agent: Vec<(String, u32)>,
    pub total_conversations: u32,
    pub projects_involved: Vec<String>,
    pub key_activities: Vec<String>,
}

pub fn format_daily_report(report: &DailyReportData, lang: Lang) -> RichMessage {
    let mut body = i18n::daily_report_summary(lang, &report.date);

    body.push_str(&format!(
        "\n\n{}",
        i18n::total_sessions(lang, report.total_conversations)
    ));

    if !report.conversations_by_agent.is_empty() {
        body.push_str(&format!("\n\n{}", i18n::by_agent_label(lang)));
        for (agent, count) in &report.conversations_by_agent {
            body.push_str(&format!(
                "\n  {}",
                i18n::agent_session_count(lang, agent, *count)
            ));
        }
    }

    if !report.projects_involved.is_empty() {
        body.push_str(&format!(
            "\n\n{}",
            i18n::projects_label(lang, &report.projects_involved.join(", "))
        ));
    }

    if !report.key_activities.is_empty() {
        body.push_str(&format!("\n\n{}", i18n::key_activities_label(lang)));
        for activity in &report.key_activities {
            body.push_str(&format!("\n  • {}", activity));
        }
    }

    RichMessage::info(body).with_title(i18n::daily_report_title(lang))
}

#[cfg(test)]
mod permission_request_tests {
    use super::*;

    #[test]
    fn renders_title_warning_and_operation_from_object_input() {
        let tool_call = serde_json::json!({
            "title": "Bash",
            "rawInput": { "command": "rm -rf build" }
        });
        let msg = format_permission_request(&tool_call, Lang::En);
        assert_eq!(msg.level, MessageLevel::Warning);
        assert_eq!(msg.title.as_deref(), Some("Permission Request"));
        let text = msg.to_plain_text();
        assert!(text.contains("Bash: rm -rf build"), "got {text}");
    }

    #[test]
    fn handles_bare_string_raw_input_and_localizes_title() {
        let tool_call = serde_json::json!({
            "title": "Bash",
            "rawInput": "ls -la"
        });
        let msg = format_permission_request(&tool_call, Lang::ZhCn);
        assert_eq!(msg.title.as_deref(), Some("权限请求"));
        assert!(msg.to_plain_text().contains("Bash: ls -la"));
    }

    #[test]
    fn falls_back_to_unknown_tool_when_empty() {
        let msg = format_permission_request(&serde_json::json!({}), Lang::En);
        assert!(msg.to_plain_text().contains("Unknown tool"));
    }
}

#[cfg(test)]
mod user_prompt_sent_tests {
    use super::*;

    #[test]
    fn renders_localized_title_and_message_as_body() {
        let msg = format_user_prompt_sent("refactor the auth module", Lang::En);
        assert_eq!(msg.level, MessageLevel::Info);
        assert_eq!(msg.title.as_deref(), Some("User Message"));
        assert_eq!(msg.body, "refactor the auth module");
    }

    #[test]
    fn localizes_title_per_language() {
        let msg = format_user_prompt_sent("你好", Lang::ZhCn);
        assert_eq!(msg.title.as_deref(), Some("用户消息"));
        assert!(msg.to_plain_text().contains("你好"));
    }
}
