//! Commands that talk to the daemon.

use std::io::{Read, Write};
use std::process::ExitCode;
use std::time::Duration;

use recurse_client::{DaemonClient, connect_or_spawn};
use recurse_protocol::{
    CellResult, CellStatus, HostInfo, HostName, RegisterParams, SseFrame, Target,
};
use serde_json::{Value, json};

use crate::args::Args;

pub async fn health() -> Result<ExitCode, String> {
    let client = DaemonClient::from_env().map_err(|error| error.to_string())?;

    match client.health().await {
        Ok(health) => {
            print_json(&serde_json::to_value(health).unwrap_or_default());
            Ok(ExitCode::SUCCESS)
        }
        Err(error) => {
            eprintln!(
                "recurse: daemon at {} is not healthy: {error}",
                client.base_url()
            );
            Ok(ExitCode::FAILURE)
        }
    }
}

async fn client() -> Result<DaemonClient, String> {
    connect_or_spawn().await.map_err(|error| error.to_string())
}

fn print_json(value: &Value) {
    println!(
        "{}",
        serde_json::to_string_pretty(value).unwrap_or_default()
    );
}

fn subcommand(args: &Args) -> (&str, &[String]) {
    args.positional
        .split_first()
        .map_or(("", &[][..]), |(first, rest)| (first.as_str(), rest))
}

async fn call(client: &DaemonClient, method: &str, params: Value) -> Result<Value, String> {
    client
        .call(method, &params)
        .await
        .map_err(|error| error.to_string())
}

pub async fn exec(args: &Args) -> Result<ExitCode, String> {
    let code = read_code(args)?;
    let target = args.target()?;
    let timeout = args.number("--timeout")?;
    let client = client().await?;

    register(&client, &target).await?;

    let mut params = json!({"target": target, "code": code});

    if let Some(timeout) = timeout {
        params["timeout_sec"] = json!(timeout);
    }

    let deadline = Duration::from_secs(timeout.unwrap_or(3600).saturating_add(30));
    let cell: CellResult = client
        .call_with_timeout("kernel.execute", &params, deadline)
        .await
        .map_err(|error| error.to_string())?;

    Ok(print_cell(&cell))
}

fn read_code(args: &Args) -> Result<String, String> {
    match (args.value("-c"), args.positional.as_slice()) {
        (Some(code), []) => Ok(code.to_owned()),
        (None, [path]) if path == "-" => {
            let mut code = String::new();

            std::io::stdin()
                .read_to_string(&mut code)
                .map_err(|error| format!("read stdin: {error}"))?;
            Ok(code)
        }
        (None, [path]) => {
            std::fs::read_to_string(path).map_err(|error| format!("read {path}: {error}"))
        }
        _ => Err("exec needs exactly one of -c CODE, FILE, or -".into()),
    }
}

async fn register(client: &DaemonClient, target: &Target) -> Result<(), String> {
    let cwd = std::env::current_dir().map_err(|error| format!("current dir: {error}"))?;
    let params = RegisterParams {
        target: target.clone(),
        cwd: cwd.to_string_lossy().into_owned(),
        host: HostInfo {
            name: HostName::Cli,
            version: Some(env!("CARGO_PKG_VERSION").into()),
            supports_children: false,
        },
        model: None,
    };
    let _: Value = client
        .call("target.register", &params)
        .await
        .map_err(|error| error.to_string())?;

    Ok(())
}

fn print_cell(cell: &CellResult) -> ExitCode {
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();

    let _ = stdout.write_all(cell.stdout.as_bytes());
    let _ = stderr.write_all(cell.stderr.as_bytes());

    if let Some(result) = &cell.result {
        let _ = writeln!(stdout, "{result}");
    }

    if let Some(error) = &cell.error {
        if error.traceback.is_empty() {
            let _ = writeln!(stderr, "{}: {}", error.ename, error.evalue);
        } else {
            let _ = writeln!(stderr, "{}", error.traceback.trim_end());
        }
    }

    if cell.truncated {
        let _ = writeln!(stderr, "[recurse] output was truncated");
    }

    match cell.status {
        CellStatus::Ok => ExitCode::SUCCESS,
        CellStatus::Error => ExitCode::FAILURE,
        CellStatus::Timeout | CellStatus::Interrupted => {
            let _ = writeln!(stderr, "[recurse] cell {}", cell.status.as_str());
            ExitCode::FAILURE
        }
    }
}

pub async fn kernel(args: &Args) -> Result<ExitCode, String> {
    let method = match subcommand(args) {
        ("status", []) => "kernel.status",
        ("restart", []) => "kernel.restart",
        ("interrupt", []) => "kernel.interrupt",
        _ => return Err("usage: recurse kernel status|restart|interrupt [--target-id ID]".into()),
    };
    let target = args.target()?;
    let client = client().await?;

    print_json(&call(&client, method, json!({"target": target})).await?);
    Ok(ExitCode::SUCCESS)
}

pub async fn children(args: &Args) -> Result<ExitCode, String> {
    if subcommand(args) != ("list", &[][..]) {
        return Err("usage: recurse children list [--target-kind K --target-id ID]".into());
    }

    let target = args.target()?;
    let client = client().await?;

    print_json(&call(&client, "children.list", json!({"target": target})).await?);
    Ok(ExitCode::SUCCESS)
}

pub async fn events(args: &Args) -> Result<ExitCode, String> {
    let target = args.target()?;
    let (command, rest) = subcommand(args);

    if !matches!(
        (command, rest.is_empty()),
        ("list" | "follow", true) | ("ack", false)
    ) {
        return Err("usage: recurse events list|follow|ack <id>… [--target-id ID]".into());
    }

    let client = client().await?;

    match command {
        "list" => print_json(&call(&client, "events.list", json!({"target": target})).await?),
        "ack" => print_json(
            &call(
                &client,
                "events.ack",
                json!({"target": target, "event_ids": rest}),
            )
            .await?,
        ),
        _ => follow(&client, &target).await?,
    }

    Ok(ExitCode::SUCCESS)
}

/// Prints each event as a JSON line and acks it, until the daemon closes the stream.
async fn follow(client: &DaemonClient, target: &Target) -> Result<(), String> {
    let mut stream = client
        .subscribe(target)
        .await
        .map_err(|error| error.to_string())?;

    while let Some(frame) = stream
        .next_frame()
        .await
        .map_err(|error| error.to_string())?
    {
        let SseFrame::Event { events, .. } = frame else {
            continue;
        };
        for event in &events {
            println!("{}", serde_json::to_string(event).unwrap_or_default());
        }

        let ids: Vec<&str> = events.iter().map(|event| event.id.as_str()).collect();

        if !ids.is_empty() {
            call(
                client,
                "events.ack",
                json!({"target": target, "event_ids": ids}),
            )
            .await?;
        }
    }

    Ok(())
}
