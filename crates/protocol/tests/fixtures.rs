use std::path::PathBuf;

use recurse_protocol::*;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../protocol/fixtures")
        .join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{path:?}: {error}"));

    serde_json::from_str(&text).unwrap()
}

fn round_trip<T: DeserializeOwned + Serialize>(value: &Value) -> T {
    let typed: T =
        serde_json::from_value(value.clone()).unwrap_or_else(|error| panic!("{error}: {value:#}"));
    let back = serde_json::to_value(&typed).unwrap();

    assert_eq!(&back, value, "round trip changed the JSON");

    typed
}

fn params_value(request: &Request) -> Value {
    let value = match request {
        Request::Health => serde_json::to_value(EmptyParams {}),
        Request::TargetRegister(params) => serde_json::to_value(params),
        Request::KernelExecute(params) => serde_json::to_value(params),
        Request::KernelInterrupt(params)
        | Request::KernelRestart(params)
        | Request::KernelStatus(params)
        | Request::ChildrenList(params)
        | Request::EventsList(params) => serde_json::to_value(params),
        Request::ChildrenBind(params) => serde_json::to_value(params),
        Request::ChildrenFail(params) => serde_json::to_value(params),
        Request::ChildrenDelete(params) => serde_json::to_value(params),
        Request::EventsAck(params) => serde_json::to_value(params),
        Request::SkillsList(params) => serde_json::to_value(params),
    };

    value.unwrap()
}

fn check_result(method: &str, result: &Value) {
    match method {
        "health" => drop(round_trip::<HealthResult>(result)),
        "target.register" => drop(round_trip::<RegisterResult>(result)),
        "kernel.execute" => drop(round_trip::<CellResult>(result)),
        "kernel.interrupt" => drop(round_trip::<InterruptResult>(result)),
        "kernel.restart" => drop(round_trip::<RestartResult>(result)),
        "kernel.status" => drop(round_trip::<KernelStatus>(result)),
        "children.list" => drop(round_trip::<ChildrenListResult>(result)),
        "children.bind" | "children.fail" | "children.delete" => {
            drop(round_trip::<ChildResult>(result));
        }
        "events.ack" => drop(round_trip::<AckResult>(result)),
        "events.list" => drop(round_trip::<EventsListResult>(result)),
        "skills.list" => drop(round_trip::<SkillsListResult>(result)),
        other => panic!("no result type for {other}"),
    }
}

#[test]
fn rpc_fixtures_round_trip() {
    let fixtures = fixture("rpc.json");
    let entries = fixtures.as_object().unwrap();

    assert!(entries.len() >= 17);

    for (name, entry) in entries {
        let request: RpcRequest = round_trip(&entry["request"]);
        let response: RpcResponse = round_trip(&entry["response"]);

        assert_eq!(request.id, response.id, "{name}: id is echoed");

        match (
            Request::parse(&request.method, request.params.clone()),
            &response.error,
        ) {
            (Ok(parsed), None) => {
                assert_eq!(
                    Some(params_value(&parsed)),
                    request.params,
                    "{name}: params"
                );
                check_result(&request.method, response.result.as_ref().unwrap());
            }
            (Ok(parsed), Some(error)) => {
                assert_eq!(
                    Some(params_value(&parsed)),
                    request.params,
                    "{name}: params"
                );
                round_trip::<RpcError>(&entry["response"]["error"]);
                assert_eq!(error.code, ErrorCode::Busy, "{name}");
            }
            (Err(parse_error), Some(error)) => assert_eq!(&parse_error, error, "{name}"),
            (Err(parse_error), None) => panic!("{name}: {parse_error}"),
        }
    }
}

#[test]
fn requests_reject_unknown_fields() {
    let params = serde_json::json!({"target": {"kind": "cli", "id": "x"}, "extra": 1});
    let error = Request::parse("kernel.status", Some(params)).unwrap_err();

    assert_eq!(error.code, ErrorCode::InvalidRequest);

    let envelope = serde_json::json!({"method": "health", "bogus": true});

    assert!(serde_json::from_value::<RpcRequest>(envelope).is_err());
    assert_eq!(Request::parse("health", None).unwrap(), Request::Health);
}

#[test]
fn kernel_fixtures_round_trip() {
    let fixtures = fixture("kernel.json");

    for message in fixtures["to_kernel"].as_array().unwrap() {
        let parsed: ToKernel = round_trip(message);

        if let ToKernel::HostResponse {
            result: Some(result),
            ..
        } = parsed
        {
            round_trip::<ChildInfo>(&result);
        }
    }

    for message in fixtures["from_kernel"].as_array().unwrap() {
        if let FromKernel::HostRequest { method, params, .. } = round_trip(message) {
            let call = HostCall::parse(&method, params.clone()).unwrap();

            assert_eq!(call.method(), method);
            assert_eq!(host_params_value(&call), params);
        }
    }
}

fn host_params_value(call: &HostCall) -> Value {
    let value = match call {
        HostCall::Spawn(params) => serde_json::to_value(params),
        HostCall::DeleteSubagent(params) => serde_json::to_value(params),
        HostCall::AgentMessage(params) => serde_json::to_value(params),
        HostCall::BashFinished(params) => serde_json::to_value(params),
        HostCall::Withdraw(params) => serde_json::to_value(params),
        HostCall::ListSubagents | HostCall::SkillsList => serde_json::to_value(EmptyParams {}),
    };

    value.unwrap()
}

#[test]
fn event_fixtures_round_trip() {
    let fixtures = fixture("events.json");

    for (name, value) in fixtures.as_object().unwrap() {
        let event: Event = round_trip(value);

        assert!(name.starts_with(event.kind()), "{name} vs {}", event.kind());
        assert!(event.id.starts_with(event.kind()));
    }
}

#[test]
fn sse_fixtures_round_trip() {
    let fixtures = fixture("sse-frames.json");
    let frames = fixtures.as_array().unwrap();

    assert_eq!(frames.len(), 3);

    for frame in frames {
        round_trip::<SseFrame>(frame);
    }
}
