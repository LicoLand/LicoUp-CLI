use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Debug)]
pub enum ToolError {
    InvalidArgs(String),
    Io(String),
    Denied(String),
}

impl std::fmt::Display for ToolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidArgs(m) | Self::Io(m) | Self::Denied(m) => write!(f, "{m}"),
        }
    }
}

pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters_schema(&self) -> Value;
    fn execute(&self, args: &Value) -> Result<String, ToolError>;
}

#[derive(Default)]
pub struct ToolRegistry {
    tools: Vec<Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.push(tool);
    }

    pub fn get(&self, name: &str) -> Option<&Arc<dyn Tool>> {
        self.tools.iter().find(|t| t.name() == name)
    }

    pub fn definitions_for_llm(&self) -> Vec<Value> {
        self.tools
            .iter()
            .map(|tool| {
                json!({
                    "type": "function",
                    "function": {
                        "name": tool.name(),
                        "description": tool.description(),
                        "parameters": tool.parameters_schema(),
                    }
                })
            })
            .collect()
    }

    pub fn names(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.name().to_string()).collect()
    }
}

struct ReadTool {
    workspace: PathBuf,
}

impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        "Read a UTF-8 text file under the workspace root."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path relative to workspace or absolute under workspace" }
            },
            "required": ["path"]
        })
    }

    fn execute(&self, args: &Value) -> Result<String, ToolError> {
        let rel = args
            .get("path")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidArgs("path required".into()))?;
        let path = resolve_under(&self.workspace, rel)?;
        std::fs::read_to_string(&path).map_err(|e| ToolError::Io(e.to_string()))
    }
}

struct WritePlanTool {
    plan_path: PathBuf,
}

impl Tool for WritePlanTool {
    fn name(&self) -> &str {
        "write_plan"
    }

    fn description(&self) -> &str {
        "Replace the bound plan document with the provided markdown content."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "content": { "type": "string", "description": "Full markdown plan content" }
            },
            "required": ["content"]
        })
    }

    fn execute(&self, args: &Value) -> Result<String, ToolError> {
        let content = args
            .get("content")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidArgs("content required".into()))?;
        // Defense in depth: only the bound literal path.
        std::fs::write(&self.plan_path, content).map_err(|e| ToolError::Io(e.to_string()))?;
        Ok(format!("wrote {}", self.plan_path.display()))
    }
}

fn resolve_under(workspace: &Path, requested: &str) -> Result<PathBuf, ToolError> {
    let candidate = if Path::new(requested).is_absolute() {
        PathBuf::from(requested)
    } else {
        workspace.join(requested)
    };
    let canonical_workspace = workspace
        .canonicalize()
        .map_err(|e| ToolError::Io(e.to_string()))?;
    let parent = candidate
        .parent()
        .ok_or_else(|| ToolError::Denied("invalid path".into()))?;
    let canonical_parent = parent
        .canonicalize()
        .map_err(|e| ToolError::Io(e.to_string()))?;
    if !canonical_parent.starts_with(&canonical_workspace) {
        return Err(ToolError::Denied("path outside workspace".into()));
    }
    let name = candidate
        .file_name()
        .ok_or_else(|| ToolError::Denied("invalid path".into()))?;
    Ok(canonical_parent.join(name))
}

pub fn read_tool(workspace: PathBuf) -> Arc<dyn Tool> {
    Arc::new(ReadTool { workspace })
}

pub fn write_plan_tool(plan_path: PathBuf) -> Arc<dyn Tool> {
    Arc::new(WritePlanTool { plan_path })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use uuid::Uuid;

    #[test]
    fn write_plan_replaces_bound_file() {
        let root = std::env::temp_dir().join(format!("lico-tool-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let plan = root.join("active-plan.md");
        fs::write(&plan, b"old").unwrap();
        let tool = write_plan_tool(plan.clone());
        tool.execute(&json!({"content": "# Plan\n"})).unwrap();
        assert_eq!(fs::read_to_string(&plan).unwrap(), "# Plan\n");
        let _ = fs::remove_dir_all(root);
    }
}
