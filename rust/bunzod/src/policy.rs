//! Runtime policy types and request-scoped evaluation context.
//!
//! M6's first slice keeps the model intentionally narrow: a runtime policy is
//! defined by `subject + action + resource -> decision(scope)`, and policy
//! checks are recorded against the current task/task-run through the runtime
//! store.

use anyhow::Result;

use crate::store::{PreparedRequest, RuntimeStore};

pub const SUBJECT_SHELL_REQUEST: &str = "shell_request";
pub const SUBJECT_SCHEDULED_JOB: &str = "scheduled_job";
pub const ACTION_INVOKE_SKILL: &str = "invoke_skill";

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
}

impl PolicySource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Rule => "rule",
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

    pub fn evaluate_skill_invocation(&self, skill_name: &str) -> Result<PolicyEvaluation> {
        self.store.evaluate_policy(
            &self.request,
            &self.request.policy_subject,
            ACTION_INVOKE_SKILL,
            skill_name,
        )
    }
}
