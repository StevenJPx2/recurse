//! `recurse`: run the daemon or MCP server, or drive a `cli` target from the shell.

mod args;
mod commands;
mod skills;

use std::process::ExitCode;

use args::Args;

const USAGE: &str = "\
usage: recurse <command> [options]

  daemon [--port N]                         run the daemon in the foreground
  mcp                                       stdio MCP server (a daemon client)
  health                                    check the daemon
  exec [--target-id ID] [--timeout SEC] (-c CODE | FILE | -)
                                            run one cell in the cli target's kernel
  kernel status|restart|interrupt [--target-id ID]
  children list [--target-kind K --target-id ID]
  events list|follow [--target-id ID]
  events ack <event-id>… [--target-id ID]
  skills list | get <name>… [--full] | path [name]
  --version

The cli target id defaults to this machine's hostname.";

fn main() -> ExitCode {
    let tokens: Vec<String> = std::env::args().skip(1).collect();
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => return fail(&format!("start runtime: {error}")),
    };

    match runtime.block_on(run(tokens)) {
        Ok(code) => code,
        Err(message) => fail(&message),
    }
}

fn fail(message: &str) -> ExitCode {
    eprintln!("recurse: {message}");
    ExitCode::from(2)
}

async fn run(tokens: Vec<String>) -> Result<ExitCode, String> {
    let (command, rest) = tokens
        .split_first()
        .map_or(("help", &[][..]), |(first, rest)| (first.as_str(), rest));
    let args = Args::parse(rest)?;

    match command {
        "--version" | "-V" | "version" => {
            println!("recurse {}", env!("CARGO_PKG_VERSION"));
            Ok(ExitCode::SUCCESS)
        }
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Ok(ExitCode::SUCCESS)
        }
        "daemon" => daemon(&args).await,
        "mcp" => recurse_mcp::run_stdio(recurse_mcp::Session::from_env())
            .await
            .map(|()| ExitCode::SUCCESS),
        "health" => commands::health().await,
        "exec" => commands::exec(&args).await,
        "kernel" => commands::kernel(&args).await,
        "children" => commands::children(&args).await,
        "events" => commands::events(&args).await,
        "skills" => skills::run(&args),
        other => Err(format!("unknown command {other}\n\n{USAGE}")),
    }
}

async fn daemon(args: &Args) -> Result<ExitCode, String> {
    let mut options = recurse_daemon::DaemonOptions::from_env()?;

    if let Some(port) = args.number("--port")? {
        options.port = u16::try_from(port).map_err(|_| format!("--port {port} is out of range"))?;
    }

    recurse_daemon::serve(options)
        .await
        .map(|()| ExitCode::SUCCESS)
}
