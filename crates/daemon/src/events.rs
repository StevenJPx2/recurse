//! Event constructors: deterministic ids and texts from the §3 table.

use recurse_protocol::{
    AgentMessagePayload, BashFinishedParams, BashFinishedPayload, ChildInfo, ChildSpawnPayload,
    Event, EventBody, KernelExitedPayload, MessageFrom, Role, Target, bounds,
};

use crate::time::now_ms;

fn event(id: String, target: Target, body: EventBody, actionable: bool, mut text: String) -> Event {
    bounds::clip_head(&mut text, bounds::EVENT_TEXT);

    Event {
        id,
        target,
        body,
        actionable,
        at: now_ms(),
        text,
    }
}

pub fn child_prompt(child: &ChildInfo) -> String {
    format!(
        "{}\n\n---\nYou are recurse child `{}` ({}). When you have an answer, reply with \
         `await agent_message.send(message, receiver_role=\"parent\")`.",
        child.task, child.name, child.child_id
    )
}

pub fn child_spawn(child: &ChildInfo) -> Event {
    let text = format!(
        "[recurse] spawning child {} ({})",
        child.name, child.child_id
    );
    let body = EventBody::ChildSpawn(ChildSpawnPayload {
        child: child.clone(),
        prompt: child_prompt(child),
    });

    event(
        format!("child.spawn:{}", child.child_id),
        child.parent.clone(),
        body,
        false,
        text,
    )
}

/// A message from `child` to its parent, sequenced per child id.
pub fn message_to_parent(child: &ChildInfo, sequence: u64, message: String) -> Event {
    let text = format!(
        "[recurse] message from child {} ({}):\n\n{message}",
        child.name, child.child_id
    );
    let body = EventBody::AgentMessage(AgentMessagePayload {
        from: MessageFrom {
            role: Role::Child,
            name: Some(child.name.clone()),
            child_id: Some(child.child_id.clone()),
        },
        message,
    });

    event(
        format!("agent.message:{}:{sequence}", child.child_id),
        child.parent.clone(),
        body,
        true,
        text,
    )
}

/// A message from `parent` to the child session `receiver`, sequenced per parent key.
pub fn message_to_child(
    parent: &Target,
    receiver: &Target,
    sequence: u64,
    message: String,
) -> Event {
    let text = format!("[recurse] message from parent:\n\n{message}");
    let body = EventBody::AgentMessage(AgentMessagePayload {
        from: MessageFrom {
            role: Role::Parent,
            name: None,
            child_id: None,
        },
        message,
    });

    event(
        format!("agent.message:{}:{sequence}", parent.key()),
        receiver.clone(),
        body,
        true,
        text,
    )
}

pub fn bash_finished(owner: &Target, kernel_pid: u32, params: BashFinishedParams) -> Event {
    let exit = params
        .exit_code
        .map_or_else(|| "unknown".to_owned(), |code| code.to_string());
    let text = format!(
        "[recurse] Background command finished: pid {}, exit {exit} ({}). Inspect the saved handle \
         with poll(), output(), or tail() and continue the task.",
        params.pid, params.command
    );
    let body = EventBody::BashFinished(BashFinishedPayload {
        handle_id: params.handle_id.clone(),
        pid: params.pid,
        exit_code: params.exit_code,
        command: params.command,
    });

    event(
        format!("bash.finished:{kernel_pid}:{}", params.handle_id),
        owner.clone(),
        body,
        true,
        text,
    )
}

pub fn kernel_exited(
    owner: &Target,
    pid: u32,
    exit_code: Option<i32>,
    signal: Option<i32>,
) -> Event {
    let how = match (signal, exit_code) {
        (Some(signal), _) => format!("signal {signal}"),
        (None, Some(code)) => format!("exit code {code}"),
        (None, None) => "unknown status".to_owned(),
    };
    let text = format!(
        "[recurse] Python kernel {pid} exited ({how}). Its state is gone; the next ipython call \
         starts a fresh kernel."
    );
    let body = EventBody::KernelExited(KernelExitedPayload {
        pid,
        exit_code,
        signal,
    });

    event(
        format!("kernel.exited:{pid}"),
        owner.clone(),
        body,
        false,
        text,
    )
}

#[cfg(test)]
mod tests {
    use recurse_protocol::{BashFinishedParams, Event, Target, TargetKind};

    fn fixture(name: &str) -> Event {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../protocol/fixtures/events.json"
        );
        let all: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();

        serde_json::from_value(all[name].clone()).unwrap()
    }

    fn parent() -> Target {
        Target::new(TargetKind::OpencodeSession, "ses_abc")
    }

    #[test]
    fn texts_and_ids_match_the_fixtures() {
        let spawn = fixture("child.spawn");
        let recurse_protocol::EventBody::ChildSpawn(payload) = &spawn.body else {
            panic!("not a spawn")
        };
        let built = super::child_spawn(&payload.child);

        assert_eq!(
            (built.id.as_str(), built.text.as_str()),
            (spawn.id.as_str(), spawn.text.as_str())
        );
        assert_eq!(built.body, spawn.body);

        let message = fixture("agent.message");
        let built = super::message_to_parent(&payload.child, 1, "Found 2 issues.".into());

        assert_eq!(
            (built.id, built.text, built.body),
            (message.id, message.text, message.body)
        );

        let to_child = fixture("agent.message.to-child");
        let session = Target::new(TargetKind::OpencodeSession, "ses_child");
        let built = super::message_to_child(
            &parent(),
            &session,
            1,
            "Check the newly added regression test.".into(),
        );

        assert_eq!(
            (built.id, built.text, built.body),
            (to_child.id, to_child.text, to_child.body)
        );

        let bash = fixture("bash.finished");
        let params = BashFinishedParams {
            handle_id: "h3".into(),
            pid: 777,
            exit_code: Some(1),
            command: "npm test".into(),
        };
        let built = super::bash_finished(&parent(), 5151, params);

        assert_eq!(
            (built.id, built.text, built.body),
            (bash.id, bash.text, bash.body)
        );

        let exited = fixture("kernel.exited");
        let built = super::kernel_exited(&parent(), 5151, None, Some(9));

        assert_eq!(
            (built.id, built.text, built.body),
            (exited.id, exited.text, exited.body)
        );
    }
}
