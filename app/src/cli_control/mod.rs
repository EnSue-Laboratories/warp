//! Client-side handler for `warp-oss control …` subcommands.
//!
//! Connects to the control socket exposed by a running Warp GUI instance
//! (see `crate::control_server`) and proxies the request.

use std::io::{BufReader, BufWriter};

use anyhow::{anyhow, Context, Result};
use warp_cli::control::{
    BlockCommand, BlockIdArg, BlockListArgs, ControlCommand, KeystrokeArgs, PaneCommand, PaneIdArg,
    PaneListArgs, PaneReadArgs, PaneScreenArgs, PaneShareArgs, PaneSnapshotArgs, PaneTargetArgs,
    SendInputArgs, ShareScrollback as CliShareScrollback, SplitArgs, SplitDirection, TabCommand,
    TabIdArg, WaitForTextArgs, WaitForTextBlockField as CliWaitForTextBlockField,
    WaitForTextMode as CliWaitForTextMode, WaitForTextSince as CliWaitForTextSince, WriteBytesArgs,
};
use warp_cli::GlobalOptions;
use warpui::AppContext;

use crate::control_server::framing::{read_frame_sync, write_frame_sync};
use crate::control_server::socket_path;
use crate::control_server::wire::{
    BlockEntry, PaneSnapshot, PaneSummary, Request, Response, ShareScrollback, SplitDir,
    TabSummary, TextMatch, TextMatchSource, WaitForTextBlockField, WaitForTextMode,
    WaitForTextSince,
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
        ControlCommand::Tab(TabCommand::New(args)) => Request::NewTab {
            config: args.config,
        },
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
        ControlCommand::Pane(PaneCommand::Send(SendInputArgs {
            pane,
            wait,
            timeout,
            command,
        })) => Request::SendInput {
            pane: match pane {
                Some(s) => Some(parse_u64(&s, "pane")?),
                None => None,
            },
            text: command.join(" "),
            wait,
            timeout_ms: timeout.map(|secs| secs.saturating_mul(1000)),
        },
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
        ControlCommand::Pane(PaneCommand::Screen(PaneScreenArgs { pane })) => Request::ReadScreen {
            pane: match pane {
                Some(s) => Some(parse_u64(&s, "pane")?),
                None => None,
            },
        },
        ControlCommand::Pane(PaneCommand::Snapshot(PaneSnapshotArgs {
            pane,
            blocks,
            no_screen,
            max_output_bytes,
            json,
        })) => Request::SnapshotPane {
            pane: match pane {
                Some(s) => Some(parse_u64(&s, "pane")?),
                None => None,
            },
            blocks,
            include_screen: !no_screen,
            max_output_bytes,
            json,
        },
        ControlCommand::Pane(PaneCommand::WaitForText(WaitForTextArgs {
            pane,
            regex,
            timeout,
            mode,
            case_insensitive,
            since,
            blocks,
            block_field,
            max_output_bytes,
            json,
            text,
        })) => Request::WaitForText {
            pane: match pane {
                Some(s) => Some(parse_u64(&s, "pane")?),
                None => None,
            },
            text,
            regex,
            timeout_ms: timeout.saturating_mul(1000),
            mode: match mode {
                CliWaitForTextMode::Screen => WaitForTextMode::Screen,
                CliWaitForTextMode::Blocks => WaitForTextMode::Blocks,
                CliWaitForTextMode::Both => WaitForTextMode::Both,
            },
            case_insensitive,
            since: match since {
                CliWaitForTextSince::All => WaitForTextSince::All,
                CliWaitForTextSince::Now => WaitForTextSince::Now,
            },
            blocks,
            block_field: match block_field {
                CliWaitForTextBlockField::Output => WaitForTextBlockField::Output,
                CliWaitForTextBlockField::Command => WaitForTextBlockField::Command,
                CliWaitForTextBlockField::Both => WaitForTextBlockField::Both,
            },
            max_output_bytes,
            json,
        },
        ControlCommand::Pane(PaneCommand::Share(PaneShareArgs { pane, scrollback })) => {
            Request::SharePane {
                pane: match pane {
                    Some(s) => Some(parse_u64(&s, "pane")?),
                    None => None,
                },
                scrollback: match scrollback {
                    CliShareScrollback::None => ShareScrollback::None,
                    CliShareScrollback::All => ShareScrollback::All,
                },
            }
        }
        ControlCommand::Pane(PaneCommand::ShareLink(PaneTargetArgs { pane })) => {
            Request::SharePaneLink {
                pane: match pane {
                    Some(s) => Some(parse_u64(&s, "pane")?),
                    None => None,
                },
            }
        }
        ControlCommand::Pane(PaneCommand::Unshare(PaneTargetArgs { pane })) => {
            Request::UnsharePane {
                pane: match pane {
                    Some(s) => Some(parse_u64(&s, "pane")?),
                    None => None,
                },
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
        Response::Screen {
            pane,
            alt_screen,
            text,
        } => {
            let mode = if alt_screen { "alt-screen" } else { "primary" };
            println!("# pane {pane} screen ({mode}):");
            println!("{}", text.trim_end());
        }
        Response::PaneSnapshot { snapshot, json } => {
            if json {
                println!("{}", serde_json::to_string_pretty(&snapshot)?);
            } else {
                print_snapshot(&snapshot);
            }
        }
        Response::WaitForTextMatched {
            pane: _,
            elapsed_ms,
            matched,
            snapshot,
            json,
        } => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "matched": true,
                        "elapsed_ms": elapsed_ms,
                        "match": matched,
                        "snapshot": snapshot,
                    }))?
                );
            } else {
                print_text_match(&matched, elapsed_ms);
            }
        }
        Response::WaitForTextTimedOut {
            pane,
            timeout_ms,
            elapsed_ms,
            snapshot,
            json,
        } => {
            if json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "matched": false,
                        "pane": pane,
                        "timeout_ms": timeout_ms,
                        "elapsed_ms": elapsed_ms,
                        "snapshot": snapshot,
                    }))?
                );
            }
            return Err(anyhow!(
                "text did not appear in pane {pane} after {}",
                format_duration_ms(timeout_ms)
            ));
        }
        Response::ShareStarted { pane } => {
            println!("sharing started for pane {pane} (pending)");
        }
        Response::ShareLink { pane: _, url } => println!("{url}"),
        Response::SharePending { pane } => {
            println!("sharing pending for pane {pane}; retry pane share-link");
        }
        Response::ShareStopped { pane } => println!("sharing stopped for pane {pane}"),
        Response::Blocks { blocks } => print_blocks(&blocks),
        Response::Block { block } => print_one_block(&block),
        Response::SendTimedOut {
            pane: _,
            timeout_ms,
            block,
        } => {
            print_one_block(&block);
            return Err(anyhow!(
                "command still running after {}; it was not stopped",
                format_duration_ms(timeout_ms)
            ));
        }
        Response::Error { message } => return Err(anyhow!("{message}")),
    }
    Ok(())
}

fn format_duration_ms(ms: u64) -> String {
    if ms % 1000 == 0 {
        format!("{}s", ms / 1000)
    } else {
        format!("{ms}ms")
    }
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
        let command = b
            .command
            .as_deref()
            .unwrap_or("-")
            .lines()
            .next()
            .unwrap_or("");
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

fn print_snapshot(snapshot: &PaneSnapshot) {
    println!(
        "# pane {} snapshot (tab {}, focused: {})",
        snapshot.pane.id, snapshot.pane.tab_id, snapshot.pane.focused
    );
    if let Some(cwd) = &snapshot.pane.cwd {
        println!("cwd: {cwd}");
    }
    if let Some(screen) = &snapshot.screen {
        let mode = if screen.alt_screen {
            "alt-screen"
        } else {
            "primary"
        };
        println!("--- screen ({mode}) ---");
        println!("{}", screen.text.trim_end());
        if screen.text_truncated {
            println!("(screen text truncated)");
        }
    }
    if !snapshot.blocks.is_empty() {
        println!("--- recent blocks ---");
        for block in &snapshot.blocks {
            print_one_block(block);
            if block.output_truncated {
                println!("(output truncated)");
            }
        }
    }
}

fn print_text_match(matched: &TextMatch, elapsed_ms: u64) {
    let source = match matched.source {
        TextMatchSource::Screen => "screen".to_string(),
        TextMatchSource::Block => match &matched.block_id {
            Some(id) => format!("block {id}"),
            None => "block".to_string(),
        },
    };
    println!(
        "matched in pane {} {source} after {}",
        matched.pane_id,
        format_duration_ms(elapsed_ms)
    );
    if let Some(line) = &matched.line {
        println!("{line}");
    } else {
        println!("{}", matched.text);
    }
}
