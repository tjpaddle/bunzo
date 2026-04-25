//! Runtime policy types and request-scoped evaluation context.
//!
//! M6's first slice keeps the model intentionally narrow: a runtime policy is
//! defined by `subject + action + resource -> decision(scope)`, and policy
//! checks are recorded against the current task/task-run through the runtime
//! store.

use anyhow::Result;
use serde_json::Value;

use crate::store::{PreparedRequest, RuntimeStore};

pub const SUBJECT_SHELL_REQUEST: &str = "shell_request";
pub const SUBJECT_SCHEDULED_JOB: &str = "scheduled_job";
pub const ACTION_INVOKE_SKILL: &str = "invoke_skill";
pub const READ_LOCAL_FILE_SKILL: &str = "read-local-file";
pub const LOCAL_FILE_READ_RESOURCE_PREFIX: &str = "read-local-file:fs-read:";
const INVALID_LOCAL_FILE_PATH_RESOURCE: &str = "read-local-file:fs-read:<invalid-path>";

/// Runtime policy resources name the concrete object being touched, not only
/// the tool. Filesystem-read skills should use this pattern:
///
/// `skill-name:fs-read:/absolute/path`
///
/// For the current local-file skill, approving
/// `read-local-file:fs-read:/etc/os-release` allows only that file resource;
/// it does not approve another path or expand the skill manifest ceiling.
pub fn skill_invocation_resource(skill_name: &str, args_json: &str) -> String {
    if skill_name == READ_LOCAL_FILE_SKILL {
        return read_local_file_path(args_json)
            .map(|path| local_file_read_resource(&path))
            .unwrap_or_else(|| INVALID_LOCAL_FILE_PATH_RESOURCE.to_string());
    }
    skill_name.to_string()
}

pub fn local_file_read_resource(path: &str) -> String {
    format!("{LOCAL_FILE_READ_RESOURCE_PREFIX}{path}")
}

pub fn read_local_file_path(args_json: &str) -> Option<String> {
    let args: Value = serde_json::from_str(args_json).ok()?;
    let path = args.get("path")?.as_str()?.trim();
    if path.is_empty() {
        None
    } else {
        Some(path.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
    RequireApproval,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Deny => "deny",
            Self::RequireApproval => "require_approval",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "allow" => Some(Self::Allow),
            "deny" => Some(Self::Deny),
            "require_approval" => Some(Self::RequireApproval),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantScope {
    Once,
    Task,
    Session,
    Persistent,
}

impl GrantScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Once => "once",
            Self::Task => "task",
            Self::Session => "session",
            Self::Persistent => "persistent",
        }
    }

    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "once" => Some(Self::Once),
            "task" => Some(Self::Task),
            "session" => Some(Self::Session),
            "persistent" => Some(Self::Persistent),
            _ => None,
        }
    }

    pub fn precedence(self) -> u8 {
        match self {
            Self::Once => 4,
            Self::Task => 3,
            Self::Session => 2,
            Self::Persistent => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicySource {
    Default,
    Rule,
    Capability,
}

impl PolicySource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Rule => "rule",
            Self::Capability => "capability",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PolicyEvaluation {
    pub policy_id: Option<String>,
    pub source: PolicySource,
    pub subject: String,
    pub action: String,
    pub resource: String,
    pub decision: Decision,
    pub grant_scope: GrantScope,
    pub detail: String,
}

#[derive(Debug, Clone)]
pub struct NewRuntimePolicy {
    pub subject: String,
    pub action: String,
    pub resource: String,
    pub decision: Decision,
    pub grant_scope: GrantScope,
    pub conversation_id: Option<String>,
    pub task_id: Option<String>,
    pub task_run_id: Option<String>,
    pub note_text: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ToolPolicyContext {
    store: RuntimeStore,
    request: PreparedRequest,
}

impl ToolPolicyContext {
    pub fn new(store: RuntimeStore, request: PreparedRequest) -> Self {
        Self { store, request }
    }

    pub fn evaluate_skill_invocation(
        &self,
        skill_name: &str,
        args_json: &str,
    ) -> Result<PolicyEvaluation> {
        let resource = skill_invocation_resource(skill_name, args_json);
        self.evaluate_skill_resource(&resource)
    }

    pub fn evaluate_skill_resource(&self, resource: &str) -> Result<PolicyEvaluation> {
        self.store.evaluate_policy(
            &self.request,
            &self.request.policy_subject,
            ACTION_INVOKE_SKILL,
            resource,
        )
    }

    pub fn deny_skill_resource_by_capability(
        &self,
        resource: &str,
        detail: String,
    ) -> Result<PolicyEvaluation> {
        let evaluation = PolicyEvaluation {
            policy_id: None,
            source: PolicySource::Capability,
            subject: self.request.policy_subject.clone(),
            action: ACTION_INVOKE_SKILL.to_string(),
            resource: resource.to_string(),
            decision: Decision::Deny,
            grant_scope: GrantScope::Once,
            detail,
        };
        self.store
            .record_policy_evaluation(&self.request, &evaluation)?;
        Ok(evaluation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_local_file_policy_resource_includes_path() {
        let resource =
            skill_invocation_resource(READ_LOCAL_FILE_SKILL, r#"{"path":"/etc/os-release"}"#);
        assert_eq!(resource, "read-local-file:fs-read:/etc/os-release");
    }

    #[test]
    fn read_local_file_policy_resource_does_not_fall_back_to_skill_name() {
        let resource = skill_invocation_resource(READ_LOCAL_FILE_SKILL, r#"{"path":""}"#);
        assert_eq!(resource, INVALID_LOCAL_FILE_PATH_RESOURCE);
    }
}
