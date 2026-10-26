//! Cross-agent session and transcript inspector.
//!
//! Provides streaming, memory-safe JSONL inspection of Claude Code and Antigravity trajectories,
//! reporting turns, tool calls, errors, and rate limits without loading entire multi-megabyte files.

use anyhow::{bail, Context, Result};
use nuvek_core::scrub::{redact, redact_config};
use serde_json::Value;
use std::collections::VecDeque;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionTool {
    Claude,
    Agy,
}

impl std::fmt::Display for SessionTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Claude => write!(f, "claude"),
            Self::Agy => write!(f, "agy"),
        }
    }
}

impl SessionTool {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "claude" | "claude-code" => Some(Self::Claude),
            "agy" | "antigravity" | "gemini" => Some(Self::Agy),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionMeta {
    pub id: String,
    pub path: PathBuf,
    pub tool: SessionTool,
    pub modified: SystemTime,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventRole {
    User,
    Assistant,
    ToolCall,
    ToolResult,
    RateLimit,
    Error,
    System,
}

impl std::fmt::Display for EventRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::User => write!(f, "USER"),
            Self::Assistant => write!(f, "ASSISTANT"),
            Self::ToolCall => write!(f, "TOOL_CALL"),
            Self::ToolResult => write!(f, "TOOL_RESULT"),
            Self::RateLimit => write!(f, "RATE_LIMIT"),
            Self::Error => write!(f, "ERROR"),
            Self::System => write!(f, "SYSTEM"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct SessionEvent {
    pub role: EventRole,
    pub timestamp: Option<String>,
    pub summary: String,
    pub details: Option<String>,
    pub tool_name: Option<String>,
}

#[derive(Debug, Default)]
pub struct SessionStats {
    pub user_prompts: usize,
    pub assistant_turns: usize,
    pub tool_calls: usize,
    pub tool_results: usize,
    pub errors: usize,
    pub rate_limits: usize,
    pub total_lines: usize,
    pub start_time: Option<String>,
    pub end_time: Option<String>,
    pub last_prompt: Option<String>,
    pub last_response: Option<String>,
    pub rate_limit_details: Option<String>,
    pub last_model: Option<String>,
}

/// Scrub credentials from a string with the nuvek-core redactor.
pub fn scrub_text(text: &str) -> String {
    redact(&redact_config(text))
}

/// Find all transcripts across both tools under `$HOME`.
pub fn discover_sessions(
    home: &Path,
    filter_tool: Option<SessionTool>,
) -> Result<Vec<SessionMeta>> {
    let mut sessions = Vec::new();

    // 1. Claude sessions: ~/.claude/projects/*/*.jsonl
    if filter_tool.is_none() || filter_tool == Some(SessionTool::Claude) {
        let projects_dir = home.join(".claude/projects");
        if projects_dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&projects_dir) {
                for proj in entries.flatten() {
                    let ppath = proj.path();
                    if !ppath.is_dir() {
                        continue;
                    }
                    if let Ok(files) = std::fs::read_dir(&ppath) {
                        for f in files.flatten() {
                            let path = f.path();
                            if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                                if let Ok(meta) = f.metadata() {
                                    let id = path
                                        .file_stem()
                                        .map(|s| s.to_string_lossy().to_string())
                                        .unwrap_or_default();
                                    sessions.push(SessionMeta {
                                        id,
                                        path,
                                        tool: SessionTool::Claude,
                                        modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                                        size_bytes: meta.len(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // 2. Antigravity sessions: ~/.gemini/antigravity-cli/brain/*/.system_generated/logs/transcript.jsonl
    if filter_tool.is_none() || filter_tool == Some(SessionTool::Agy) {
        let brain_dir = home.join(".gemini/antigravity-cli/brain");
        if brain_dir.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&brain_dir) {
                for sess in entries.flatten() {
                    let spath = sess.path();
                    if !spath.is_dir() {
                        continue;
                    }
                    let transcript = spath.join(".system_generated/logs/transcript.jsonl");
                    if transcript.is_file() {
                        if let Ok(meta) = transcript.metadata() {
                            let id = spath
                                .file_name()
                                .map(|s| s.to_string_lossy().to_string())
                                .unwrap_or_default();
                            sessions.push(SessionMeta {
                                id,
                                path: transcript,
                                tool: SessionTool::Agy,
                                modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                                size_bytes: meta.len(),
                            });
                        }
                    }
                }
            }
        }
    }

    // Sort by modified time descending (newest first)
    sessions.sort_by_key(|a| std::cmp::Reverse(a.modified));
    Ok(sessions)
}

/// Find a specific session by ID, filename prefix, path, or find the most recent.
pub fn resolve_session(
    home: &Path,
    target: Option<&str>,
    tool_filter: Option<SessionTool>,
) -> Result<SessionMeta> {
    if let Some(t) = target {
        let p = PathBuf::from(t);
        if p.is_file() {
            let meta = p.metadata()?;
            let tool = if p.to_string_lossy().contains("transcript.jsonl") {
                SessionTool::Agy
            } else {
                SessionTool::Claude
            };
            return Ok(SessionMeta {
                id: p
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| "unknown".into()),
                path: p,
                tool,
                modified: meta.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                size_bytes: meta.len(),
            });
        }

        // Try exact or prefix match against all discovered sessions
        let sessions = discover_sessions(home, tool_filter)?;
        if let Some(found) = sessions
            .iter()
            .find(|s| s.id == t || s.id.starts_with(t) || s.path.to_string_lossy().contains(t))
        {
            return Ok(found.clone());
        }
        bail!("session '{t}' not found in Claude or Antigravity storage");
    }

    // Return the newest session
    let sessions = discover_sessions(home, tool_filter)?;
    sessions
        .into_iter()
        .next()
        .context("no agent sessions found in Claude or Antigravity directories")
}

/// Parse a single Claude Code JSON line into zero or more normalized events.
pub fn parse_claude_line(val: &Value) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    let timestamp = val
        .get("timestamp")
        .and_then(Value::as_str)
        .map(String::from);
    let typ = val.get("type").and_then(Value::as_str).unwrap_or_default();

    // Check for rate-limiting or errors at line root
    if let Some(err) = val.get("error").and_then(Value::as_str) {
        if err == "rate_limit" || val.get("apiErrorStatus").and_then(Value::as_i64) == Some(429) {
            let mut summary = String::from("Rate limit encountered");
            if let Some(msg_content) = val.pointer("/message/content") {
                if let Some(arr) = msg_content.as_array() {
                    for item in arr {
                        if item.get("type").and_then(Value::as_str) == Some("text") {
                            if let Some(txt) = item.get("text").and_then(Value::as_str) {
                                summary = txt.trim().to_string();
                            }
                        }
                    }
                }
            }
            let resets_at = val
                .pointer("/quotaLimits/resetsAt")
                .and_then(Value::as_i64)
                .map(chrono_timestamp_str);
            events.push(SessionEvent {
                role: EventRole::RateLimit,
                timestamp: timestamp.clone(),
                summary: summary.clone(),
                details: resets_at.map(|r| format!("Resets at: {r}")),
                tool_name: None,
            });
            return events;
        }
    }

    match typ {
        "user" => {
            if let Some(content) = val.pointer("/message/content") {
                if let Some(txt) = content.as_str() {
                    events.push(SessionEvent {
                        role: EventRole::User,
                        timestamp,
                        summary: txt.to_string(),
                        details: None,
                        tool_name: None,
                    });
                } else if let Some(arr) = content.as_array() {
                    for block in arr {
                        let b_type = block
                            .get("type")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        if b_type == "text" {
                            if let Some(t) = block.get("text").and_then(Value::as_str) {
                                events.push(SessionEvent {
                                    role: EventRole::User,
                                    timestamp: timestamp.clone(),
                                    summary: t.to_string(),
                                    details: None,
                                    tool_name: None,
                                });
                            }
                        } else if b_type == "tool_result" {
                            let tool_id = block
                                .get("tool_use_id")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            let output = block
                                .get("content")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            let is_err = block
                                .get("is_error")
                                .and_then(Value::as_bool)
                                .unwrap_or(false);
                            events.push(SessionEvent {
                                role: if is_err {
                                    EventRole::Error
                                } else {
                                    EventRole::ToolResult
                                },
                                timestamp: timestamp.clone(),
                                summary: format!("Tool result: {tool_id}"),
                                details: Some(output.to_string()),
                                tool_name: Some(tool_id.to_string()),
                            });
                        }
                    }
                }
            }
        }
        "assistant" => {
            if let Some(content) = val.pointer("/message/content").and_then(Value::as_array) {
                for block in content {
                    let b_type = block
                        .get("type")
                        .and_then(Value::as_str)
                        .unwrap_or_default();
                    if b_type == "text" {
                        if let Some(t) = block.get("text").and_then(Value::as_str) {
                            events.push(SessionEvent {
                                role: EventRole::Assistant,
                                timestamp: timestamp.clone(),
                                summary: t.to_string(),
                                details: None,
                                tool_name: None,
                            });
                        }
                    } else if b_type == "tool_use" {
                        let name = block
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown");
                        let input = block
                            .get("input")
                            .map(|v| v.to_string())
                            .unwrap_or_default();
                        events.push(SessionEvent {
                            role: EventRole::ToolCall,
                            timestamp: timestamp.clone(),
                            summary: format!("Tool call: {name}"),
                            details: Some(input),
                            tool_name: Some(name.to_string()),
                        });
                    }
                }
            }
        }
        "last-prompt" => {
            if let Some(prompt) = val.get("lastPrompt").and_then(Value::as_str) {
                events.push(SessionEvent {
                    role: EventRole::User,
                    timestamp,
                    summary: prompt.to_string(),
                    details: None,
                    tool_name: None,
                });
            }
        }
        "system" => {
            if let Some(content) = val.get("content").and_then(Value::as_str) {
                events.push(SessionEvent {
                    role: EventRole::System,
                    timestamp,
                    summary: content.to_string(),
                    details: None,
                    tool_name: None,
                });
            }
        }
        _ => {}
    }

    events
}

/// Parse a single Antigravity JSON line into zero or more normalized events.
pub fn parse_agy_line(val: &Value) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    let timestamp = val
        .get("created_at")
        .and_then(Value::as_str)
        .map(String::from);
    let typ = val.get("type").and_then(Value::as_str).unwrap_or_default();
    let status = val
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default();

    if status == "ERROR" {
        let content = val
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or("Unknown AGY error");
        events.push(SessionEvent {
            role: EventRole::Error,
            timestamp: timestamp.clone(),
            summary: content.to_string(),
            details: None,
            tool_name: None,
        });
        return events;
    }

    match typ {
        "USER_INPUT" => {
            let content = val
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default();
            events.push(SessionEvent {
                role: EventRole::User,
                timestamp,
                summary: content.to_string(),
                details: None,
                tool_name: None,
            });
        }
        "PLANNER_RESPONSE" => {
            if let Some(tool_calls) = val.get("tool_calls").and_then(Value::as_array) {
                for tc in tool_calls {
                    let name = tc.get("name").and_then(Value::as_str).unwrap_or("unknown");
                    let args = tc.get("args").map(|v| v.to_string()).unwrap_or_default();
                    events.push(SessionEvent {
                        role: EventRole::ToolCall,
                        timestamp: timestamp.clone(),
                        summary: format!("Tool call: {name}"),
                        details: Some(args),
                        tool_name: Some(name.to_string()),
                    });
                }
            }
            if let Some(content) = val.get("content").and_then(Value::as_str) {
                if !content.trim().is_empty() {
                    events.push(SessionEvent {
                        role: EventRole::Assistant,
                        timestamp: timestamp.clone(),
                        summary: content.to_string(),
                        details: None,
                        tool_name: None,
                    });
                }
            }
        }
        "GENERIC" => {
            let content = val
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if !content.trim().is_empty() {
                events.push(SessionEvent {
                    role: EventRole::ToolResult,
                    timestamp,
                    summary: "Output / Result".into(),
                    details: Some(content.to_string()),
                    tool_name: None,
                });
            }
        }
        "SYSTEM_MESSAGE" => {
            let content = val
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default();
            events.push(SessionEvent {
                role: EventRole::System,
                timestamp,
                summary: content.to_string(),
                details: None,
                tool_name: None,
            });
        }
        _ => {}
    }

    events
}

fn chrono_timestamp_str(epoch_secs: i64) -> String {
    // Format timestamp without requiring extra chrono features
    let d = std::time::UNIX_EPOCH + std::time::Duration::from_secs(epoch_secs.max(0) as u64);
    format!("{epoch_secs} (approx. {:?})", d)
}

/// Stream-parse a session file, populating session stats and calling `on_event` for each event.
pub fn parse_session_file<P: AsRef<Path>, F: FnMut(SessionEvent)>(
    path: P,
    tool: SessionTool,
    mut on_event: F,
) -> Result<SessionStats> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);

    let mut stats = SessionStats::default();

    for line_res in reader.lines() {
        let line = match line_res {
            Ok(l) => l,
            Err(_) => continue,
        };
        stats.total_lines += 1;
        let line_trimmed = line.trim();
        if line_trimmed.is_empty() {
            continue;
        }

        let val: Value = match serde_json::from_str(line_trimmed) {
            Ok(v) => v,
            Err(_) => continue, // Ignore truncated trailing line safely
        };

        let events = match tool {
            SessionTool::Claude => {
                if let Some(m) = val.pointer("/message/model").and_then(Value::as_str) {
                    if m != "<synthetic>" {
                        stats.last_model = Some(m.to_string());
                    }
                }
                parse_claude_line(&val)
            }
            SessionTool::Agy => parse_agy_line(&val),
        };

        for ev in events {
            if stats.start_time.is_none() && ev.timestamp.is_some() {
                stats.start_time.clone_from(&ev.timestamp);
            }
            if ev.timestamp.is_some() {
                stats.end_time.clone_from(&ev.timestamp);
            }

            match ev.role {
                EventRole::User => {
                    stats.user_prompts += 1;
                    stats.last_prompt = Some(ev.summary.clone());
                }
                EventRole::Assistant => {
                    stats.assistant_turns += 1;
                    stats.last_response = Some(ev.summary.clone());
                }
                EventRole::ToolCall => {
                    stats.tool_calls += 1;
                }
                EventRole::ToolResult => {
                    stats.tool_results += 1;
                }
                EventRole::RateLimit => {
                    stats.rate_limits += 1;
                    stats.rate_limit_details = Some(ev.summary.clone());
                }
                EventRole::Error => {
                    stats.errors += 1;
                }
                EventRole::System => {}
            }
            on_event(ev);
        }
    }

    Ok(stats)
}

/// Print command `session last`: shows the last prompt, response, rate limits, and summary.
pub fn cmd_last(home: &Path, tool_name: Option<&str>) -> Result<()> {
    let tool_filter = tool_name.and_then(SessionTool::parse);
    let session = resolve_session(home, None, tool_filter)?;
    let stats = parse_session_file(&session.path, session.tool, |_| ())?;

    say!("Session ID: {}", session.id);
    say!("Tool:       {}", session.tool);
    say!("Path:       {}", session.path.display());
    say!(
        "Size:       {:.2} MB",
        session.size_bytes as f64 / 1_048_576.0
    );
    if let Some(start) = stats.start_time {
        say!("Started:    {start}");
    }
    if let Some(end) = stats.end_time {
        say!("Last event: {end}");
    }
    if let Some(model) = stats.last_model {
        say!("Model:      {model}");
    }
    say!(
        "Activity:   {} prompts · {} assistant turns · {} tool calls · {} errors",
        stats.user_prompts,
        stats.assistant_turns,
        stats.tool_calls,
        stats.errors
    );

    if let Some(rl) = &stats.rate_limit_details {
        say!("\n⚠️  RATE LIMIT STATUS:");
        say!("    {}", scrub_text(rl));
    }

    if let Some(prompt) = &stats.last_prompt {
        say!("\n--- LAST USER PROMPT ---");
        let scrubbed = scrub_text(prompt);
        let preview: String = scrubbed.lines().take(15).collect::<Vec<_>>().join("\n");
        say!("{preview}");
        if scrubbed.lines().count() > 15 {
            say!(
                "    [... truncated {} lines]",
                scrubbed.lines().count() - 15
            );
        }
    }

    if let Some(resp) = &stats.last_response {
        say!("\n--- LAST ASSISTANT RESPONSE ---");
        let scrubbed = scrub_text(resp);
        let preview: String = scrubbed.lines().take(20).collect::<Vec<_>>().join("\n");
        say!("{preview}");
        if scrubbed.lines().count() > 20 {
            say!(
                "    [... truncated {} lines]",
                scrubbed.lines().count() - 20
            );
        }
    }

    Ok(())
}

/// Print command `session tail`: prints the last N events matching optional filter.
pub fn cmd_tail(
    home: &Path,
    session_target: Option<&str>,
    n: usize,
    filter: Option<&str>,
) -> Result<()> {
    let session = resolve_session(home, session_target, None)?;
    let mut ring: VecDeque<SessionEvent> = VecDeque::with_capacity(n.min(1000));
    let mut total_events = 0;
    let mut total_matching = 0;

    let _stats = parse_session_file(&session.path, session.tool, |ev| {
        total_events += 1;
        let matches = match filter.unwrap_or_default() {
            "dialogue" => ev.role == EventRole::User || ev.role == EventRole::Assistant,
            "tools" => ev.role == EventRole::ToolCall || ev.role == EventRole::ToolResult,
            "errors" => ev.role == EventRole::Error || ev.role == EventRole::RateLimit,
            _ => true,
        };
        if matches {
            total_matching += 1;
            if ring.len() == n && n > 0 {
                ring.pop_front();
            }
            if n > 0 {
                ring.push_back(ev);
            }
        }
    })?;

    say!(
        "Tailing last {} events for {} ({}) [filtered from {} matching / {} total]:\n",
        ring.len(),
        session.id,
        session.tool,
        total_matching,
        total_events
    );

    for ev in ring {
        let ts = ev.timestamp.as_deref().unwrap_or("unknown-time");
        let label = match ev.role {
            EventRole::User => "\x1b[32m[USER]\x1b[0m".to_string(),
            EventRole::Assistant => "\x1b[34m[ASSISTANT]\x1b[0m".to_string(),
            EventRole::ToolCall => {
                if let Some(tn) = &ev.tool_name {
                    format!("\x1b[33m[TOOL: {tn}]\x1b[0m")
                } else {
                    "\x1b[33m[TOOL CALL]\x1b[0m".to_string()
                }
            }
            EventRole::ToolResult => {
                if let Some(tn) = &ev.tool_name {
                    format!("\x1b[36m[RESULT: {tn}]\x1b[0m")
                } else {
                    "\x1b[36m[TOOL RESULT]\x1b[0m".to_string()
                }
            }
            EventRole::RateLimit => "\x1b[31;1m[RATE LIMIT]\x1b[0m".to_string(),
            EventRole::Error => "\x1b[31m[ERROR]\x1b[0m".to_string(),
            EventRole::System => "\x1b[90m[SYSTEM]\x1b[0m".to_string(),
        };
        say!("{ts} {label} {}", scrub_text(&ev.summary));
        if let Some(details) = &ev.details {
            let scrubbed = scrub_text(details);
            let preview = scrubbed.lines().take(5).collect::<Vec<_>>().join("\n");
            say!("    {preview}");
            if scrubbed.lines().count() > 5 {
                say!("    ... ({} more lines)", scrubbed.lines().count() - 5);
            }
        }
    }

    Ok(())
}

/// Print command `session status`: comprehensive status table for session.
pub fn cmd_status(home: &Path, session_target: Option<&str>) -> Result<()> {
    let session = resolve_session(home, session_target, None)?;
    let stats = parse_session_file(&session.path, session.tool, |_| ())?;

    say!("Session Summary");
    say!("===============");
    say!("ID:           {}", session.id);
    say!("Tool:         {}", session.tool);
    say!("Path:         {}", session.path.display());
    say!(
        "Size:         {:.2} MB",
        session.size_bytes as f64 / 1_048_576.0
    );
    say!("Lines:        {}", stats.total_lines);
    say!(
        "Start:        {}",
        stats.start_time.as_deref().unwrap_or("N/A")
    );
    say!(
        "End:          {}",
        stats.end_time.as_deref().unwrap_or("N/A")
    );
    if let Some(m) = stats.last_model {
        say!("Model:        {m}");
    }
    say!("\nBreakdown");
    say!("---------");
    say!("User Prompts:     {}", stats.user_prompts);
    say!("Assistant Turns:  {}", stats.assistant_turns);
    say!("Tool Calls:       {}", stats.tool_calls);
    say!("Tool Results:     {}", stats.tool_results);
    say!("Errors:           {}", stats.errors);
    say!("Rate Limits:      {}", stats.rate_limits);

    let state = if stats.rate_limits > 0 {
        "Rate-Limited"
    } else if stats.errors > 0 {
        "Completed with Errors"
    } else {
        "Normal"
    };
    say!("\nState: {}", state);

    if let Some(rl) = stats.rate_limit_details {
        say!("Rate limit details: {}", scrub_text(&rl));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_claude_user_and_assistant() {
        let user_line = serde_json::json!({
            "type": "user",
            "timestamp": "2026-09-26T10:00:00Z",
            "message": { "content": "Please check the status." }
        });
        let evs = parse_claude_line(&user_line);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].role, EventRole::User);
        assert_eq!(evs[0].summary, "Please check the status.");

        let asst_line = serde_json::json!({
            "type": "assistant",
            "timestamp": "2026-09-26T10:00:05Z",
            "message": {
                "content": [
                    { "type": "text", "text": "Checking status now." },
                    { "type": "tool_use", "name": "Bash", "input": { "command": "git status" } }
                ]
            }
        });
        let evs2 = parse_claude_line(&asst_line);
        assert_eq!(evs2.len(), 2);
        assert_eq!(evs2[0].role, EventRole::Assistant);
        assert_eq!(evs2[1].role, EventRole::ToolCall);
        assert_eq!(evs2[1].tool_name.as_deref(), Some("Bash"));
    }

    #[test]
    fn parse_claude_rate_limit() {
        let rl_line = serde_json::json!({
            "type": "assistant",
            "timestamp": "2026-09-26T14:27:51Z",
            "error": "rate_limit",
            "apiErrorStatus": 429,
            "quotaLimits": { "resetsAt": 1790460000 },
            "message": {
                "content": [
                    { "type": "text", "text": "You've hit your weekly limit · resets 6pm" }
                ]
            }
        });
        let evs = parse_claude_line(&rl_line);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].role, EventRole::RateLimit);
        assert!(evs[0].summary.contains("You've hit your weekly limit"));
    }

    #[test]
    fn parse_agy_turns() {
        let user_line = serde_json::json!({
            "type": "USER_INPUT",
            "created_at": "2026-09-26T14:55:09Z",
            "content": "Run tests"
        });
        let evs = parse_agy_line(&user_line);
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].role, EventRole::User);

        let planner_line = serde_json::json!({
            "type": "PLANNER_RESPONSE",
            "created_at": "2026-09-26T14:55:10Z",
            "content": "Running test suite",
            "tool_calls": [
                { "name": "run_command", "args": { "CommandLine": "cargo test" } }
            ]
        });
        let evs2 = parse_agy_line(&planner_line);
        assert_eq!(evs2.len(), 2);
        assert_eq!(evs2[0].role, EventRole::ToolCall);
        assert_eq!(evs2[1].role, EventRole::Assistant);
    }
}
