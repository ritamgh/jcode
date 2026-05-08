use super::{Tool, ToolContext, ToolOutput};
use anyhow::Result;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

pub struct ReadToolArtifactTool;

impl ReadToolArtifactTool {
    pub fn new() -> Self {
        Self
    }
}

#[derive(Debug, Deserialize)]
struct ReadToolArtifactInput {
    #[serde(default)]
    call_id: Option<String>,
    #[serde(default)]
    artifact: Option<String>,
    #[serde(default)]
    start_line: Option<usize>,
    #[serde(default)]
    end_line: Option<usize>,
}

#[async_trait]
impl Tool for ReadToolArtifactTool {
    fn name(&self) -> &str {
        "read_tool_artifact"
    }

    fn description(&self) -> &str {
        "Read the full raw output for a spilled tool result artifact, by artifact path or unique call_id. Supports optional 1-based line ranges."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "intent": super::intent_schema_property(),
                "call_id": {
                    "type": "string",
                    "description": "Tool call id from a [tool result spilled] block. Must be unique if artifact is omitted."
                },
                "artifact": {
                    "type": "string",
                    "description": "Artifact path from a [tool result spilled] block. Preferred when available."
                },
                "start_line": {
                    "type": "integer",
                    "description": "Optional 1-based first line to read."
                },
                "end_line": {
                    "type": "integer",
                    "description": "Optional 1-based final line to read."
                }
            }
        })
    }

    async fn execute(&self, input: Value, ctx: ToolContext) -> Result<ToolOutput> {
        let params: ReadToolArtifactInput = serde_json::from_value(input)?;
        let path = if let Some(artifact) =
            params.artifact.as_deref().filter(|s| !s.trim().is_empty())
        {
            let p = PathBuf::from(artifact);
            if p.is_absolute() {
                p
            } else {
                ctx.resolve_path(Path::new(artifact))
            }
        } else if let Some(call_id) = params.call_id.as_deref().filter(|s| !s.trim().is_empty()) {
            crate::tool_artifacts::find_artifact_by_call_id(call_id)?
        } else {
            anyhow::bail!("provide either artifact or call_id")
        };

        let output =
            crate::tool_artifacts::read_artifact(&path, params.start_line, params.end_line)?;
        Ok(ToolOutput::new(output).with_title(format!("tool artifact: {}", path.display())))
    }
}
