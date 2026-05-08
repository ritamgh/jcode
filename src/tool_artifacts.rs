use crate::config::config;
use crate::message::{ContentBlock, Message, Role};
use crate::storage::jcode_dir;
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};

pub const SPILLED_MARKER: &str = "[tool result spilled]";

#[derive(Debug, Clone)]
pub struct SpillStats {
    pub count: usize,
    pub original_bytes: usize,
    pub inline_bytes: usize,
}

impl SpillStats {
    pub fn saved_bytes(&self) -> usize {
        self.original_bytes.saturating_sub(self.inline_bytes)
    }
}

#[derive(Debug, Clone)]
pub struct SpilledToolResult {
    pub artifact_path: PathBuf,
    pub sha256: String,
    pub original_bytes: usize,
    pub original_lines: usize,
    pub inline_replacement: String,
}

#[derive(Debug, Clone)]
pub struct ToolResultSpillConfig {
    pub enabled: bool,
    pub inline_threshold_bytes: usize,
    pub preview_head_bytes: usize,
    pub preview_tail_bytes: usize,
    pub artifact_dir: PathBuf,
}

impl ToolResultSpillConfig {
    pub fn load() -> Self {
        let features = &config().features;
        Self {
            enabled: features.tool_result_spill_enabled,
            inline_threshold_bytes: features.tool_result_inline_threshold_bytes,
            preview_head_bytes: features.tool_result_preview_head_bytes,
            preview_tail_bytes: features.tool_result_preview_tail_bytes,
            artifact_dir: artifact_dir_from_config(features.tool_result_artifact_dir.as_deref()),
        }
    }
}

fn artifact_dir_from_config(configured: Option<&str>) -> PathBuf {
    if let Some(path) = configured.filter(|s| !s.trim().is_empty()) {
        if let Some(rest) = path.strip_prefix("~/") {
            if let Some(home) = dirs::home_dir() {
                return home.join(rest);
            }
        }
        return PathBuf::from(path);
    }
    jcode_dir()
        .unwrap_or_else(|_| {
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".jcode")
        })
        .join("tool-artifacts")
}

pub fn is_spilled_tool_result(content: &str) -> bool {
    content.trim_start().starts_with(SPILLED_MARKER)
}

pub fn spill_tool_result(
    session_id: &str,
    tool_call_id: &str,
    tool_name: &str,
    raw_text: &str,
    cfg: &ToolResultSpillConfig,
) -> Result<SpilledToolResult> {
    let original_bytes = raw_text.len();
    let original_lines = count_lines(raw_text);
    let sha256 = sha256_hex(raw_text.as_bytes());
    let artifact_path = artifact_path(&cfg.artifact_dir, session_id, tool_call_id, &sha256);

    write_artifact_atomic(&artifact_path, raw_text.as_bytes())?;

    let preview = head_tail_preview(raw_text, cfg.preview_head_bytes, cfg.preview_tail_bytes);
    let mut inline_replacement = format!(
        "{SPILLED_MARKER}\n\
tool={tool_name}\n\
call_id={tool_call_id}\n\
original_bytes={original_bytes}\n\
original_lines={original_lines}\n\
inline_bytes=0\n\
artifact={}\n\
sha256={sha256}\n\
preview_strategy=head_tail\n\
preview:\n{preview}",
        artifact_path.display(),
    );
    let inline_bytes = inline_replacement.len();
    inline_replacement =
        inline_replacement.replace("inline_bytes=0", &format!("inline_bytes={inline_bytes}"));

    Ok(SpilledToolResult {
        artifact_path,
        sha256,
        original_bytes,
        original_lines,
        inline_replacement,
    })
}

pub fn compact_historical_tool_results_in_messages(
    session_id: &str,
    messages: &mut [Message],
    cfg: &ToolResultSpillConfig,
) -> SpillStats {
    if !cfg.enabled {
        return SpillStats {
            count: 0,
            original_bytes: 0,
            inline_bytes: 0,
        };
    }

    let historical = historical_tool_result_message_flags(messages);
    let tool_names = tool_name_by_call_id_messages(messages);
    let mut stats = SpillStats {
        count: 0,
        original_bytes: 0,
        inline_bytes: 0,
    };

    for (idx, message) in messages.iter_mut().enumerate() {
        if !historical.get(idx).copied().unwrap_or(false) {
            continue;
        }
        for block in &mut message.content {
            let ContentBlock::ToolResult {
                tool_use_id,
                content,
                ..
            } = block
            else {
                continue;
            };
            if content.len() <= cfg.inline_threshold_bytes || is_spilled_tool_result(content) {
                continue;
            }
            let tool_name = tool_names
                .get(tool_use_id)
                .map(String::as_str)
                .unwrap_or("unknown");
            match spill_tool_result(session_id, tool_use_id, tool_name, content, cfg) {
                Ok(spilled) => {
                    let inline_bytes = spilled.inline_replacement.len();
                    crate::logging::info(&format!(
                        "TOOL_RESULT_SPILL tool={} call_id={} original_bytes={} inline_bytes={} saved_bytes={} artifact={}",
                        tool_name,
                        tool_use_id,
                        spilled.original_bytes,
                        inline_bytes,
                        spilled.original_bytes.saturating_sub(inline_bytes),
                        spilled.artifact_path.display(),
                    ));
                    stats.count += 1;
                    stats.original_bytes += spilled.original_bytes;
                    stats.inline_bytes += inline_bytes;
                    *content = spilled.inline_replacement;
                }
                Err(err) => {
                    crate::logging::warn(&format!(
                        "TOOL_RESULT_SPILL_FAILED call_id={} reason={} keeping_inline=true",
                        tool_use_id, err
                    ));
                }
            }
        }
    }

    stats
}

pub fn compact_historical_tool_results_in_stored_messages(
    session_id: &str,
    messages: &mut [jcode_session_types::StoredMessage],
    cfg: &ToolResultSpillConfig,
) -> SpillStats {
    let mut provider_messages: Vec<Message> = messages
        .iter()
        .map(jcode_session_types::StoredMessage::to_message)
        .collect();
    let stats =
        compact_historical_tool_results_in_messages(session_id, &mut provider_messages, cfg);
    if stats.count > 0 {
        for (stored, compacted) in messages.iter_mut().zip(provider_messages.into_iter()) {
            stored.content = compacted.content;
        }
    }
    stats
}

fn historical_tool_result_message_flags(messages: &[Message]) -> Vec<bool> {
    let mut has_later_assistant = vec![false; messages.len()];
    let mut seen_later_assistant = false;
    for (idx, message) in messages.iter().enumerate().rev() {
        has_later_assistant[idx] = seen_later_assistant;
        if matches!(message.role, Role::Assistant) {
            seen_later_assistant = true;
        }
    }
    has_later_assistant
}

fn tool_name_by_call_id_messages(messages: &[Message]) -> HashMap<String, String> {
    let mut names = HashMap::new();
    for message in messages {
        for block in &message.content {
            if let ContentBlock::ToolUse { id, name, .. } = block {
                names.insert(id.clone(), name.clone());
            }
        }
    }
    names
}

pub fn read_artifact(
    path: &Path,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> Result<String> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("failed to read tool artifact {}", path.display()))?;
    if start_line.is_none() && end_line.is_none() {
        return Ok(content);
    }
    let start = start_line.unwrap_or(1).max(1);
    let end = end_line.unwrap_or(usize::MAX);
    let mut out = String::new();
    for (idx, line) in content.lines().enumerate() {
        let line_no = idx + 1;
        if line_no < start {
            continue;
        }
        if line_no > end {
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    Ok(out)
}

pub fn find_artifact_by_call_id(call_id: &str) -> Result<PathBuf> {
    let cfg = ToolResultSpillConfig::load();
    let safe = safe_component(call_id);
    let mut matches = Vec::new();
    if cfg.artifact_dir.exists() {
        collect_matching_artifacts(&cfg.artifact_dir, &safe, &mut matches)?;
    }
    match matches.len() {
        0 => anyhow::bail!("no tool artifact found for call_id={call_id}"),
        1 => Ok(matches.remove(0)),
        _ => anyhow::bail!(
            "multiple tool artifacts found for call_id={call_id}; pass artifact path explicitly"
        ),
    }
}

fn collect_matching_artifacts(
    dir: &Path,
    safe_call_id: &str,
    matches: &mut Vec<PathBuf>,
) -> Result<()> {
    for entry in fs::read_dir(dir).with_context(|| format!("read_dir {}", dir.display()))? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_matching_artifacts(&path, safe_call_id, matches)?;
        } else if path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|name| name.starts_with(safe_call_id) && name.ends_with(".txt"))
        {
            matches.push(path);
        }
    }
    Ok(())
}

fn write_artifact_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if path.exists() {
        let existing = fs::read(path)?;
        if existing == bytes {
            return Ok(());
        }
    }
    let parent = path.parent().context("artifact path has no parent")?;
    fs::create_dir_all(parent)?;
    let tmp_path = path.with_extension(format!("txt.tmp.{}", std::process::id()));
    {
        let mut file = File::create(&tmp_path)?;
        file.write_all(bytes)?;
        let _ = file.sync_all();
    }
    fs::rename(&tmp_path, path)?;
    if !path.exists() {
        anyhow::bail!(
            "artifact rename succeeded but final path missing: {}",
            path.display()
        );
    }
    Ok(())
}

fn artifact_path(root: &Path, session_id: &str, tool_call_id: &str, sha256: &str) -> PathBuf {
    root.join(safe_component(session_id)).join(format!(
        "{}-{}.txt",
        safe_component(tool_call_id),
        &sha256[..12]
    ))
}

fn safe_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len().min(96));
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
            out.push(ch);
        } else {
            out.push('_');
        }
        if out.len() >= 120 {
            break;
        }
    }
    if out.is_empty() {
        "unknown".to_string()
    } else {
        out
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn count_lines(text: &str) -> usize {
    if text.is_empty() {
        0
    } else {
        text.lines().count()
    }
}

fn head_tail_preview(text: &str, head_bytes: usize, tail_bytes: usize) -> String {
    let total = text.len();
    if total <= head_bytes.saturating_add(tail_bytes) {
        return text.to_string();
    }
    let head_end = floor_char_boundary(text, head_bytes.min(total));
    let tail_start = ceil_char_boundary(text, total.saturating_sub(tail_bytes));
    format!("{}\n...\n{}", &text[..head_end], &text[tail_start..])
}

fn floor_char_boundary(text: &str, mut idx: usize) -> usize {
    while idx > 0 && !text.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn ceil_char_boundary(text: &str, mut idx: usize) -> usize {
    while idx < text.len() && !text.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spilled_marker_is_idempotent() {
        assert!(is_spilled_tool_result("[tool result spilled]\ncall_id=x"));
        assert!(!is_spilled_tool_result("normal output"));
    }

    #[test]
    fn preview_respects_utf8() {
        let text = "αβγδ".repeat(2000);
        let preview = head_tail_preview(&text, 1537, 1537);
        assert!(preview.contains("..."));
    }
}
