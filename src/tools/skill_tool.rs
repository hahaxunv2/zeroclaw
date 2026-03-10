use super::traits::{Tool, ToolResult};
use crate::runtime::RuntimeAdapter;
use crate::security::SecurityPolicy;
use crate::skills::SkillTool;
use async_trait::async_trait;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

/// Maximum execution time for skill tools.
const SKILL_TOOL_TIMEOUT_SECS: u64 = 300; // Skills might take longer (e.g. macro engine)
const MAX_OUTPUT_BYTES: usize = 1_048_576; // 1MB

#[derive(Clone)]
pub struct SkillToolExecutor {
    pub tool: SkillTool,
    pub security: Arc<SecurityPolicy>,
    pub runtime: Arc<dyn RuntimeAdapter>,
}

impl SkillToolExecutor {
    pub fn new(
        tool: SkillTool,
        security: Arc<SecurityPolicy>,
        runtime: Arc<dyn RuntimeAdapter>,
    ) -> Self {
        Self {
            tool,
            security,
            runtime,
        }
    }

    fn substitute_args(&self, command: &str, args: &[String]) -> String {
        let mut result = command.to_string();
        for (i, arg) in args.iter().enumerate() {
            let placeholder = format!("${{{}}}", i + 1);
            result = result.replace(&placeholder, arg);
            let simple_placeholder = format!("${}", i + 1);
            result = result.replace(&simple_placeholder, arg);
        }
        result
    }
}

#[async_trait]
impl Tool for SkillToolExecutor {
    fn name(&self) -> &str {
        &self.tool.name
    }

    fn description(&self) -> &str {
        &self.tool.description
    }

    fn parameters_schema(&self) -> serde_json::Value {
        // Generic schema for skill tools.
        // We allow "arguments" as a list of strings if the command uses ${1}, ${2}, etc.
        json!({
            "type": "object",
            "properties": {
                "arguments": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Positional arguments if the tool requires them"
                }
            }
        })
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        let args_val = args.get("arguments").cloned().unwrap_or_else(|| {
            // If direct parameter name was not used, fallback to positional parsing
            // if we have only one string param in the tool definition.
            if args.is_string() {
                args.clone()
            } else {
                serde_json::Value::Null
            }
        });

        let mut positional_args = Vec::new();
        if let Some(arr) = args_val.as_array() {
            for v in arr {
                positional_args.push(v.as_str().unwrap_or("").to_string());
            }
        } else if let Some(s) = args_val.as_str() {
            positional_args.push(s.to_string());
        }

        let command = if positional_args.is_empty() {
            self.tool.command.clone()
        } else {
            self.substitute_args(&self.tool.command, &positional_args)
        };

        if self.tool.kind != "shell" {
            return Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Unsupported skill tool kind: {}", self.tool.kind)),
            });
        }

        // Security check
        match self.security.validate_command_execution(&command, false) {
            Ok(_) => {}
            Err(reason) => {
                tracing::error!(tool = %self.tool.name, %reason, "skill tool blocked by security policy");
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Security block: {}", reason)),
                });
            }
        }

        let workspace_dir = self
            .tool
            .location
            .as_ref()
            .unwrap_or(&self.security.workspace_dir);

        let mut cmd = match self.runtime.build_shell_command(&command, workspace_dir) {
            Ok(cmd) => cmd,
            Err(e) => {
                return Ok(ToolResult {
                    success: false,
                    output: String::new(),
                    error: Some(format!("Failed to build command: {e}")),
                });
            }
        };

        // Skill tools often need the full environment if they are complex scripts
        // But we should stick to what ShellTool does for safety, or allow more if needed.
        // For now, let's match ShellTool behavior.
        cmd.env_clear();
        const SAFE_VARS: &[&str] = &["PATH", "HOME", "USER", "SHELL", "LANG", "TERM"];
        for var in SAFE_VARS {
            if let Ok(val) = std::env::var(var) {
                cmd.env(var, val);
            }
        }

        let result =
            tokio::time::timeout(Duration::from_secs(SKILL_TOOL_TIMEOUT_SECS), cmd.output()).await;

        match result {
            Ok(Ok(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout);
                let stderr = String::from_utf8_lossy(&output.stderr);

                let mut out = stdout.to_string();
                if out.len() > MAX_OUTPUT_BYTES {
                    out.truncate(MAX_OUTPUT_BYTES);
                    out.push_str("\n... [output truncated]");
                }

                if !output.status.success() {
                    tracing::error!(
                        tool = %self.tool.name,
                        exit_code = ?output.status.code(),
                        stderr = %stderr,
                        "skill tool exited with non-zero status"
                    );
                }
                Ok(ToolResult {
                    success: output.status.success(),
                    output: out,
                    error: if stderr.is_empty() {
                        None
                    } else {
                        Some(stderr.to_string())
                    },
                })
            }
            Ok(Err(e)) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Execution failed: {e}")),
            }),
            Err(_) => Ok(ToolResult {
                success: false,
                output: String::new(),
                error: Some(format!("Timed out after {}s", SKILL_TOOL_TIMEOUT_SECS)),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::config::Config;
    use crate::runtime::NativeRuntime;
    use crate::security::SecurityPolicy;
    use std::path::PathBuf;
    use std::sync::Arc;

    #[tokio::test]
    async fn test_macro_agent_skill_registration() {
        let config = Config::default();
        let security = Arc::new(SecurityPolicy::default());
        let runtime = Arc::new(NativeRuntime::new());

        // Use the actual open-skills directory if it exists
        // Based on logs, it's at /Users/yunyun/open-skills
        let open_skills_dir = PathBuf::from("/Users/yunyun/open-skills");
        if !open_skills_dir.exists() {
            println!("Skipping test: /Users/yunyun/open-skills not found");
            return;
        }

        let skills = crate::skills::load_skills_with_config(&open_skills_dir, &config);
        println!("Loaded {} skills", skills.len());

        let macro_skill = skills.iter().find(|s| s.name == "macro_agent");

        let skill =
            macro_skill.expect("macro_agent skill should be loaded from /Users/yunyun/open-skills");
        assert_eq!(skill.name, "macro_agent");

        let tools = crate::tools::skill_tools_to_registry(&skills, security, runtime);
        let found_tools: Vec<_> = tools
            .iter()
            .map(|t| t.spec().name)
            .filter(|name| name == "list_area_reports" || name == "read_master_report")
            .collect();

        assert!(
            !found_tools.is_empty(),
            "Macro agent tools should be in the registry"
        );
        println!("Verified tools: {:?}", found_tools);
    }
}
