//! Host requests from a kernel (§4.1) and the child operations they share with RPC.

use std::path::Path;
use std::sync::Arc;

use recurse_protocol::{
    AgentMessageParams, ChildInfo, ChildResult, ChildrenListResult, DeleteSubagentParams,
    EventIdResult, HostCall, Role, RpcError, SkillInfo, SkillsListResult, SpawnParams, Target,
    WithdrawResult, bounds,
};
use serde::Serialize;
use serde_json::Value;

use crate::app::App;
use crate::events;
use crate::registry::{invalid, limit, not_found};
use crate::time::now_ms;

pub async fn handle(
    app: &Arc<App>,
    owner: &Target,
    kernel_pid: u32,
    method: &str,
    params: Value,
) -> Result<Value, RpcError> {
    match HostCall::parse(method, params)? {
        HostCall::Spawn(params) => json(&spawn(app, owner, params)?),
        HostCall::ListSubagents => json(&ChildrenListResult {
            children: app.state.read(|core| core.registry.children_of(owner)),
        }),
        HostCall::DeleteSubagent(selector) => json(&ChildResult {
            child: delete_child(app, owner, &selector).await?,
        }),
        HostCall::AgentMessage(params) => json(&EventIdResult {
            event_id: send_message(app, owner, params)?,
        }),
        HostCall::BashFinished(params) => {
            let event = events::bash_finished(owner, kernel_pid, params);
            let event_id = app.state.update(&app.log, |core| Ok(core.enqueue(event)))?;

            json(&EventIdResult { event_id })
        }
        HostCall::Withdraw(params) => {
            let withdrawn = app.state.update(&app.log, |core| {
                Ok(core.queue.withdraw(owner, &params.event_id))
            })?;

            json(&WithdrawResult { withdrawn })
        }
        HostCall::SkillsList => json(&SkillsListResult {
            skills: skills_for(app, Some(owner)),
        }),
    }
}

fn json<T: Serialize>(value: &T) -> Result<Value, RpcError> {
    serde_json::to_value(value)
        .map_err(|error| RpcError::new(recurse_protocol::ErrorCode::Internal, error.to_string()))
}

fn spawn(app: &App, owner: &Target, params: SpawnParams) -> Result<ChildInfo, RpcError> {
    let max_depth = app.options.max_depth;

    app.state.update(&app.log, |core| {
        let child = core.registry.admit(owner, params, max_depth, now_ms())?;

        core.enqueue(events::child_spawn(&child));

        Ok(child)
    })
}

/// Marks a child deleted, unbinds its session, and shuts down that session's kernel.
pub async fn delete_child(
    app: &Arc<App>,
    parent: &Target,
    selector: &DeleteSubagentParams,
) -> Result<ChildInfo, RpcError> {
    let child = app
        .state
        .update(&app.log, |core| core.registry.delete(parent, selector))?;

    if let Some(session) = &child.session {
        app.kernels
            .shutdown_target(session, app.options.shutdown_grace)
            .await;
    }

    Ok(child)
}

fn send_message(
    app: &App,
    sender: &Target,
    params: AgentMessageParams,
) -> Result<String, RpcError> {
    if params.message.len() > bounds::MESSAGE {
        return Err(limit("message exceeds 32 KiB"));
    }

    match params.receiver_role {
        Role::Parent => app.state.update(&app.log, |core| {
            let child = core
                .registry
                .binding(sender)
                .cloned()
                .ok_or_else(|| not_found(format!("{sender} is not a bound recurse child")))?;
            let sequence = core.registry.next_sequence(&child.child_id);

            Ok(core.enqueue(events::message_to_parent(&child, sequence, params.message)))
        }),
        Role::Child => {
            let name = params
                .receiver_name
                .ok_or_else(|| invalid("receiver_role \"child\" needs receiver_name"))?;

            app.state.update(&app.log, |core| {
                let session = core
                    .registry
                    .running_child_named(sender, &name)
                    .and_then(|child| child.session.clone())
                    .ok_or_else(|| not_found(format!("no running child named {name}")))?;
                let sequence = core.registry.next_sequence(&sender.key());

                Ok(core.enqueue(events::message_to_child(
                    sender,
                    &session,
                    sequence,
                    params.message,
                )))
            })
        }
    }
}

/// Skills visible to `target` (its registered cwd adds project skills).
pub fn skills_for(app: &App, target: Option<&Target>) -> Vec<SkillInfo> {
    let cwd = target.and_then(|target| {
        app.state.read(|core| {
            core.registry
                .record(target)
                .map(|record| record.cwd.clone())
        })
    });

    crate::skills::discover(&app.options.skill_dirs(cwd.as_deref().map(Path::new)))
}
