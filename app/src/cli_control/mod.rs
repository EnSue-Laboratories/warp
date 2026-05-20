//! Client-side handler for `warp-oss control …` subcommands.
//!
//! Connects to the control socket exposed by a running Warp GUI instance
//! (see `crate::control_server`) and proxies the request.

use std::io::{BufReader, BufWriter};

use anyhow::{anyhow, Context, Result};
use warp_cli::control::{
    BlockCommand, BlockIdArg, BlockListArgs, ControlCommand, KeystrokeArgs, PaneCommand,
    PaneIdArg, PaneListArgs, PaneReadArgs, SendInputArgs, SplitArgs, SplitDirection, TabCommand,
    TabIdArg, WriteBytesArgs,
};
use warp_cli::GlobalOptions;
use warpui::AppContext;

use crate::control_server::framing::{read_frame_sync, write_frame_sync};
use crate::control_server::socket_path;
use crate::control_server::wire::{
    BlockEntry, PaneSummary, Request, Response, SplitDir, TabSummary,
};

/// Dispatch `warp control …` from the full CLI plumbing (after AppContext
/// init). This path exists for compatibility with the agent_sdk dispatcher;
/// the fast path in `app/src/lib.rs` calls `run_standalone` directly without
/// spinning up an `AppContext`.
pub fn run(
    _ctx: &mut AppContext,
    _global_options: GlobalOptions,
    command: ControlCommand,
) -> Result<()> {
    run_standalone(command)
}

/// Connect to the control socket, send the request, print the response.
/// Pure client; does not require an AppContext.
pub fn run_standalone(command: ControlCommand) -> Result<()> {
    let request = build_request(command)?;
    let response = send(request)?;
    print_response(response)
}

fn build_request(cmd: ControlCommand) -> Result<Request> {
    Ok(match cmd {
        ControlCommand::Tab(TabCommand::List) => Request::ListTabs,
        ControlCommand::Tab(TabCommand::New) => Request::NewTab,
        ControlCommand::Tab(TabCommand::Close(TabIdArg { id })) => Request::CloseTab {
            tab: parse_u64(&id, "tab")?,
        },
        ControlCommand::Tab(TabCommand::Focus(TabIdArg { id })) => Request::FocusTab {
            tab: parse_u64(&id, "tab")?,
        },
        ControlCommand::Pane(PaneCommand::List(PaneListArgs { tab })) => Request::ListPanes {
            tab: match tab {
                Some(s) => Some(parse_u64(&s, "tab")?),
                None => None,
            },
        },
        ControlCommand::Pane(PaneCommand::Send(SendInputArgs { pane, command })) => {
            Request::SendInput {
                pane: match pane {
                    Some(s) => Some(parse_u64(&s, "pane")?),
                    None => None,
                },
                text: command.join(" "),
            }
        }
        ControlCommand::Pane(PaneCommand::Write(WriteBytesArgs { pane, text })) => {
            Request::WriteBytes {
                pane: match pane {
                    Some(s) => Some(parse_u64(&s, "pane")?),
                    None => None,
                },
                bytes: text.into_bytes(),
            }
        }
        ControlCommand::Pane(PaneCommand::Keystroke(KeystrokeArgs { pane, key })) => {
            Request::Keystroke {
                pane: match pane {
                    Some(s) => Some(parse_u64(&s, "pane")?),
                    None => None,
                },
                key,
            }
        }
        ControlCommand::Pane(PaneCommand::Read(PaneReadArgs { pane, blocks })) => {
            Request::ReadPane {
                pane: match pane {
                    Some(s) => Some(parse_u64(&s, "pane")?),
                    None => None,
                },
                blocks,
            }
        }
        ControlCommand::Pane(PaneCommand::Focus(PaneIdArg { id })) => Request::FocusPane {
            pane: parse_u64(&id, "pane")?,
        },
        ControlCommand::Pane(PaneCommand::Split(SplitArgs { pane, direction })) => {
            Request::SplitPane {
                pane: match pane {
                    Some(s) => Some(parse_u64(&s, "pane")?),
                    None => None,
                },
                direction: match direction {
                    SplitDirection::Left => SplitDir::Left,
                    SplitDirection::Right => SplitDir::Right,
                    SplitDirection::Up => SplitDir::Up,
                    SplitDirection::Down => SplitDir::Down,
                },
            }
        }
        ControlCommand::Pane(PaneCommand::Close(PaneIdArg { id })) => Request::ClosePane {
            pane: parse_u64(&id, "pane")?,
        },
        ControlCommand::Block(BlockCommand::List(BlockListArgs { pane, limit })) => {
            Request::ListBlocks {
                pane: match pane {
                    Some(s) => Some(parse_u64(&s, "pane")?),
                    None => None,
                },
                limit,
            }
        }
        ControlCommand::Block(BlockCommand::Read(BlockIdArg { id })) => {
            Request::ReadBlock { block: id }
        }
    })
}

fn parse_u64(s: &str, what: &str) -> Result<u64> {
    s.parse::<u64>()
        .map_err(|_| anyhow!("{what} id must be a number, got {s:?}"))
}

fn send(request: Request) -> Result<Response> {
    let path = socket_path();
    let stream = std::os::unix::net::UnixStream::connect(&path).with_context(|| {
        format!(
            "could not connect to Warp control socket at {} — is Warp running?",
            path.display()
        )
    })?;
    let stream_read = stream.try_clone().context("clone stream")?;
    let mut reader = BufReader::new(stream_read);
    let mut writer = BufWriter::new(stream);
    write_frame_sync(&mut writer, &request)?;
    drop(writer); // flush + half-close write side
    let response: Response = read_frame_sync(&mut reader)?;
    Ok(response)
}

fn print_response(response: Response) -> Result<()> {
    match response {
        Response::Pong => println!("pong"),
        Response::Ok => println!("ok"),
        Response::Tabs { tabs } => print_tabs(&tabs),
        Response::Panes { panes } => print_panes(&panes),
        Response::PaneOutput { pane, blocks } => print_pane_output(pane, &blocks),
        Response::Blocks { blocks } => print_blocks(&blocks),
        Response::Block { block } => print_one_block(&block),
        Response::Error { message } => return Err(anyhow!("{message}")),
    }
    Ok(())
}

fn print_tabs(tabs: &[TabSummary]) {
    if tabs.is_empty() {
        println!("(no tabs)");
        return;
    }
    println!("{:<8} {:<6} {:<10} {}", "TAB", "INDEX", "ACTIVE", "PANES");
    for t in tabs {
        let active = if t.active { "yes" } else { "" };
        let panes = t
            .pane_ids
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "{:<8} {:<6} {:<10} {}",
            t.id,
            t.index,
            active,
            if panes.is_empty() { "-" } else { &panes }
        );
    }
}

fn print_panes(panes: &[PaneSummary]) {
    if panes.is_empty() {
        println!("(no panes)");
        return;
    }
    println!(
        "{:<10} {:<8} {:<6} {:<8} {}",
        "PANE", "TAB", "INDEX", "FOCUSED", "CWD"
    );
    for p in panes {
        let cwd = p.cwd.as_deref().unwrap_or("-");
        let focused = if p.focused { "yes" } else { "" };
        println!(
            "{:<10} {:<8} {:<6} {:<8} {}",
            p.id, p.tab_id, p.tab_index, focused, cwd
        );
    }
}

fn print_pane_output(pane: u64, blocks: &[BlockEntry]) {
    if blocks.is_empty() {
        println!("(pane {pane} has no blocks)");
        return;
    }
    println!("# pane {pane}: last {} block(s)", blocks.len());
    for b in blocks {
        print_one_block(b);
    }
}

fn print_blocks(blocks: &[BlockEntry]) {
    if blocks.is_empty() {
        println!("(no blocks)");
        return;
    }
    println!("{:<40} {:<10} {:<8} {}", "BLOCK", "PANE", "EXIT", "COMMAND");
    for b in blocks {
        let exit = match b.exit_code {
            Some(c) => c.to_string(),
            None => "-".into(),
        };
        let command = b.command.as_deref().unwrap_or("-").lines().next().unwrap_or("");
        println!("{:<40} {:<10} {:<8} {}", b.id, b.pane_id, exit, command);
    }
}

fn print_one_block(b: &BlockEntry) {
    println!("--- block {} (pane {}) ---", b.id, b.pane_id);
    if let Some(pwd) = &b.pwd {
        println!("pwd: {pwd}");
    }
    if let Some(cmd) = &b.command {
        println!("$ {cmd}");
    }
    if !b.output.is_empty() {
        println!("{}", b.output.trim_end());
    }
    if let Some(code) = b.exit_code {
        println!("(exit {code})");
    }
}
