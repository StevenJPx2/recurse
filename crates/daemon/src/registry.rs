//! Target registrations and the child registry: the rules of §4.1, without I/O.

use std::collections::BTreeMap;
use std::hash::BuildHasher;

use recurse_protocol::{
    ChildInfo, ChildStatus, DeleteSubagentParams, ErrorCode, HostInfo, RegisterParams,
    RegisterResult, RpcError, SpawnParams, Target, bounds, is_valid_name,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TargetRecord {
    pub target: Target,
    pub cwd: String,
    pub host: HostInfo,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registry {
    #[serde(default)]
    pub targets: BTreeMap<String, TargetRecord>,
    #[serde(default)]
    pub children: Vec<ChildInfo>,
    /// Per-sender `agent.message` sequence numbers.
    #[serde(default)]
    pub sequences: BTreeMap<String, u64>,
}

impl Registry {
    pub fn register(&mut self, params: RegisterParams) -> Result<RegisterResult, RpcError> {
        let key = params.target.key();

        if !self.targets.contains_key(&key) && self.targets.len() >= bounds::REGISTERED_TARGETS {
            return Err(limit(format!(
                "at most {} targets may be registered",
                bounds::REGISTERED_TARGETS
            )));
        }

        let record = TargetRecord {
            target: params.target.clone(),
            cwd: params.cwd,
            host: params.host,
            model: params.model,
        };

        self.targets.insert(key, record);

        Ok(self.describe(&params.target))
    }

    pub fn record(&self, target: &Target) -> Option<&TargetRecord> {
        self.targets.get(&target.key())
    }

    pub fn describe(&self, target: &Target) -> RegisterResult {
        let child = self.binding(target).cloned();

        RegisterResult {
            target: target.clone(),
            depth: child.as_ref().map_or(0, |child| child.depth),
            parent: child.as_ref().map(|child| child.parent.clone()),
            child,
        }
    }

    /// The running child whose host session is `target`.
    pub fn binding(&self, target: &Target) -> Option<&ChildInfo> {
        self.children
            .iter()
            .find(|child| child.bound_session() == Some(target))
    }

    pub fn depth(&self, target: &Target) -> u32 {
        self.binding(target).map_or(0, |child| child.depth)
    }

    pub fn children_of(&self, parent: &Target) -> Vec<ChildInfo> {
        self.children
            .iter()
            .filter(|child| &child.parent == parent)
            .cloned()
            .collect()
    }

    fn live_children<'a>(&'a self, parent: &'a Target) -> impl Iterator<Item = &'a ChildInfo> {
        self.children
            .iter()
            .filter(move |child| &child.parent == parent && !child.is_deleted())
    }

    pub fn running_child_named<'a>(
        &'a self,
        parent: &'a Target,
        name: &str,
    ) -> Option<&'a ChildInfo> {
        self.live_children(parent)
            .find(|child| child.name == name && child.status == ChildStatus::Running)
    }

    pub fn admit(
        &mut self,
        parent: &Target,
        params: SpawnParams,
        max_depth: u32,
        now: u64,
    ) -> Result<ChildInfo, RpcError> {
        let record = self
            .record(parent)
            .ok_or_else(|| not_found(format!("target {parent} is not registered")))?;

        if !record.host.supports_children {
            return Err(RpcError::new(
                ErrorCode::UnsupportedHost,
                format!(
                    "this host ({}) cannot create child sessions",
                    host_name(&record.host)
                ),
            ));
        }

        let depth = self.depth(parent).saturating_add(1);

        if depth > max_depth {
            return Err(RpcError::new(
                ErrorCode::DepthExceeded,
                format!("children may not spawn children (RECURSE_MAX_DEPTH={max_depth})"),
            ));
        }

        self.check_admission(parent, &params)?;

        let child = ChildInfo {
            child_id: self.new_child_id(now),
            name: params.name,
            task: params.task,
            model: params.model.or_else(|| record.model.clone()),
            status: ChildStatus::Pending,
            parent: parent.clone(),
            session: None,
            session_dir: record.cwd.clone(),
            depth,
            created_at: now,
        };

        self.children.push(child.clone());

        Ok(child)
    }

    fn check_admission(&self, parent: &Target, params: &SpawnParams) -> Result<(), RpcError> {
        if self.live_children(parent).count() >= bounds::CHILDREN_PER_PARENT {
            return Err(limit(format!(
                "at most {} children per parent; delete one first",
                bounds::CHILDREN_PER_PARENT
            )));
        }

        if !is_valid_name(&params.name) {
            return Err(invalid(format!(
                "child name {:?} must match ^[a-z0-9][a-z0-9-]{{0,62}}$",
                params.name
            )));
        }

        if self
            .live_children(parent)
            .any(|child| child.name == params.name)
        {
            return Err(invalid(format!(
                "a child named {} already exists",
                params.name
            )));
        }

        if params.task.trim().is_empty() {
            return Err(invalid("task must not be empty"));
        }

        if params.task.len() > bounds::TASK {
            return Err(limit("task exceeds 32 KiB"));
        }

        Ok(())
    }

    pub fn bind(
        &mut self,
        parent: &Target,
        child_id: &str,
        session: &Target,
    ) -> Result<ChildInfo, RpcError> {
        if session == parent {
            return Err(invalid("a child session cannot be its parent"));
        }

        if let Some(other) = self
            .binding(session)
            .filter(|other| other.child_id != child_id)
        {
            return Err(invalid(format!(
                "session {session} is already bound to child {}",
                other.child_id
            )));
        }

        let child = self.child_mut(parent, child_id)?;

        match child.status {
            ChildStatus::Pending => {
                child.status = ChildStatus::Running;
                child.session = Some(session.clone());
            }
            ChildStatus::Running if child.session.as_ref() == Some(session) => {}
            ChildStatus::Running | ChildStatus::Failed | ChildStatus::Deleted => {
                return Err(invalid(format!(
                    "child {child_id} is {} and cannot be bound",
                    status_name(child.status)
                )));
            }
        }

        Ok(child.clone())
    }

    pub fn fail(&mut self, parent: &Target, child_id: &str) -> Result<ChildInfo, RpcError> {
        let child = self.child_mut(parent, child_id)?;

        match child.status {
            ChildStatus::Pending => child.status = ChildStatus::Failed,
            ChildStatus::Failed => {}
            ChildStatus::Running | ChildStatus::Deleted => {
                return Err(invalid(format!(
                    "child {child_id} is {} and cannot fail to start",
                    status_name(child.status)
                )));
            }
        }

        Ok(child.clone())
    }

    pub fn delete(
        &mut self,
        parent: &Target,
        selector: &DeleteSubagentParams,
    ) -> Result<ChildInfo, RpcError> {
        let child_id = match (&selector.child_id, &selector.name) {
            (Some(child_id), _) => child_id.clone(),
            (None, Some(name)) => self
                .live_children(parent)
                .find(|child| &child.name == name)
                .map(|child| child.child_id.clone())
                .ok_or_else(|| not_found(format!("no child named {name}")))?,
            (None, None) => return Err(invalid("delete needs child_id or name")),
        };
        let child = self.child_mut(parent, &child_id)?;

        child.status = ChildStatus::Deleted;

        Ok(child.clone())
    }

    pub fn next_sequence(&mut self, sender: &str) -> u64 {
        let sequence = self.sequences.entry(sender.to_owned()).or_insert(0);

        *sequence = sequence.saturating_add(1);
        *sequence
    }

    fn child_mut(&mut self, parent: &Target, child_id: &str) -> Result<&mut ChildInfo, RpcError> {
        self.children
            .iter_mut()
            .find(|child| &child.parent == parent && child.child_id == child_id)
            .ok_or_else(|| not_found(format!("no child {child_id} for {parent}")))
    }

    fn new_child_id(&self, now: u64) -> String {
        let state = std::hash::RandomState::new();

        (0u64..)
            .map(|salt| format!("c_{:08x}", state.hash_one((now, salt)) & 0xffff_ffff))
            .find(|id| self.children.iter().all(|child| &child.child_id != id))
            .unwrap_or_default()
    }
}

fn host_name(host: &HostInfo) -> String {
    serde_json::to_value(host.name)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}

const fn status_name(status: ChildStatus) -> &'static str {
    match status {
        ChildStatus::Pending => "pending",
        ChildStatus::Running => "running",
        ChildStatus::Failed => "failed",
        ChildStatus::Deleted => "deleted",
    }
}

pub fn invalid(message: impl Into<String>) -> RpcError {
    RpcError::new(ErrorCode::InvalidRequest, message)
}

pub fn not_found(message: impl Into<String>) -> RpcError {
    RpcError::new(ErrorCode::NotFound, message)
}

pub fn limit(message: impl Into<String>) -> RpcError {
    RpcError::new(ErrorCode::LimitExceeded, message)
}
