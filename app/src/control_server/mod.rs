//! In-process control surface for a running Warp GUI instance.
//!
//! Binds a per-app Unix domain socket and serves a JSON-RPC protocol that
//! mirrors the `warp control …` CLI surface (see `crate::cli_control` and
//! `warp_cli::control`). All session/pane/block state lives in the UI
//! process's `warpui` Entity model, so RPC handlers hop onto the main thread
//! via the spawner before reading/writing state.
//!
//! Modeled on `crate::remote_server::unix::launch_daemon`. See `CLAUDE.md`
//! ("Warp-as-CLI" section) for design rationale.

pub mod framing;
pub mod wire;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use chrono::{SecondsFormat, Utc};
use futures::io::{BufReader, BufWriter};
use futures::AsyncReadExt as _;
use regex::RegexBuilder;
use warpui::r#async::Timer;
use warpui::{AppContext, Entity, EntityId, SingletonEntity, TypedActionView, ViewHandle};
use wire::{
    BlockEntry, PaneScreenSnapshot, PaneSnapshot, PaneSnapshotPane, PaneStatus, PaneSummary,
    Request, Response, ShareScrollback, SplitDir, TabSummary, TextMatch, TextMatchSource,
    WaitForTextBlockField, WaitForTextMode, WaitForTextSince,
};

use crate::pane_group::tree::Direction as PaneDirection;
use crate::pane_group::{PaneGroup, PaneGroupAction};
use crate::terminal::input::{CommandExecutionResult, DenyExecutionReason};
use crate::terminal::model::terminal_model::TerminalModel;
use crate::terminal::shared_session::manager::Manager as SharedSessionManager;
use crate::terminal::shared_session::{
    join_link, SharedSessionActionSource, SharedSessionScrollbackType, SharedSessionSource,
};
use crate::terminal::view::TerminalView;
use crate::user_config::WarpConfig;
use crate::workspace::action::WorkspaceAction;
use crate::workspace::registry::WorkspaceRegistry;
use crate::workspace::view::Workspace;

const SEND_WAIT_DEFAULT_TIMEOUT_MS: u64 = 120_000;
const SEND_WAIT_POLL_INTERVAL: Duration = Duration::from_millis(50);
const WAIT_TEXT_POLL_INTERVAL: Duration = Duration::from_millis(100);
const SNAPSHOT_SCHEMA_VERSION: u32 = 1;
const PANE_LIST_PREVIEW_MAX_CHARS: usize = 60;

/// Singleton model that owns the control socket task.
pub struct ControlModel;

impl Entity for ControlModel {
    type Event = ();
}

impl SingletonEntity for ControlModel {}

/// Path used by both the server (for `bind`) and the client (for `connect`).
pub fn socket_path() -> PathBuf {
    let base = dirs::data_local_dir().unwrap_or_else(std::env::temp_dir);
    base.join("dev.warp.WarpOss").join("control.sock")
}

/// Bind the control socket and start serving requests. Called from
/// `run_internal` only when `LaunchMode::App` is active so headless CLI
/// invocations don't try to bind.
pub fn launch(ctx: &mut AppContext) {
    let path = socket_path();
    if let Some(parent) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            log::warn!("control_server: failed to create {parent:?}: {e}");
            return;
        }
    }
    if path.exists() {
        // Stale socket from a previous run (or another instance still alive).
        if std::os::unix::net::UnixStream::connect(&path).is_ok() {
            log::info!(
                "control_server: another Warp instance owns {}; not binding",
                path.display()
            );
            return;
        }
        let _ = std::fs::remove_file(&path);
    }

    let listener = match std::os::unix::net::UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            log::warn!("control_server: failed to bind {}: {e}", path.display());
            return;
        }
    };
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    listener.set_nonblocking(true).ok();
    log::info!("control_server: listening on {}", path.display());

    ctx.add_singleton_model(move |ctx| {
        let spawner = ctx.spawner();
        let exec = ctx.background_executor();
        let exec_clone = exec.clone();

        exec.spawn(async move {
            let listener = match async_io::Async::new(listener) {
                Ok(l) => l,
                Err(e) => {
                    log::error!("control_server: async listener init failed: {e}");
                    return;
                }
            };
            loop {
                match listener.accept().await {
                    Ok((stream, _)) => {
                        let spawner = spawner.clone();
                        exec_clone
                            .spawn(handle_connection(stream, spawner))
                            .detach();
                    }
                    Err(e) => log::error!("control_server: accept error: {e}"),
                }
            }
        })
        .detach();

        ControlModel
    });
}

async fn handle_connection(
    stream: async_io::Async<std::os::unix::net::UnixStream>,
    spawner: warpui::ModelSpawner<ControlModel>,
) {
    let (read_half, write_half) = stream.split();
    let mut reader = BufReader::new(read_half);
    let mut writer = BufWriter::new(write_half);

    let request: Request = match framing::read_frame(&mut reader).await {
        Ok(r) => r,
        Err(e) => {
            let _ = framing::write_frame(
                &mut writer,
                &Response::Error {
                    message: format!("invalid request: {e}"),
                },
            )
            .await;
            return;
        }
    };

    let response = dispatch_async(request, spawner).await;

    if let Err(e) = framing::write_frame(&mut writer, &response).await {
        log::warn!("control_server: write response failed: {e}");
    }
}

async fn dispatch_async(request: Request, spawner: warpui::ModelSpawner<ControlModel>) -> Response {
    match request {
        Request::SendInput {
            pane,
            text,
            wait: true,
            timeout_ms,
        } => handle_send_input_wait(pane, text, timeout_ms, spawner).await,
        Request::WaitForText {
            pane,
            text,
            regex,
            timeout_ms,
            mode,
            case_insensitive,
            since,
            blocks,
            block_field,
            max_output_bytes,
            json,
        } => {
            let options = WaitForTextOptions {
                pane,
                text,
                regex,
                timeout_ms,
                mode,
                case_insensitive,
                since,
                blocks,
                block_field,
                max_output_bytes,
                json,
            };
            handle_wait_for_text(options, spawner).await
        }
        request => dispatch_on_main(request, spawner).await,
    }
}

async fn dispatch_on_main(
    request: Request,
    spawner: warpui::ModelSpawner<ControlModel>,
) -> Response {
    spawner
        .spawn(move |_me, ctx| dispatch(request, ctx))
        .await
        .unwrap_or_else(|_| Response::Error {
            message: "control_server: dispatch dropped (model gone)".into(),
        })
}

fn dispatch(request: Request, ctx: &mut AppContext) -> Response {
    match request {
        Request::Ping => Response::Pong,
        Request::ListTabs => handle_list_tabs(ctx),
        Request::ListPanes {
            tab,
            include_preview,
            json,
        } => handle_list_panes(tab, include_preview, json, ctx),
        Request::SendInput {
            pane,
            text,
            wait: _,
            timeout_ms: _,
        } => handle_send_input(pane, text, ctx),
        Request::ReadPane { pane, blocks } => handle_read_pane(pane, blocks, ctx),
        Request::ReadScreen { pane } => handle_read_screen(pane, ctx),
        Request::SnapshotPane {
            pane,
            blocks,
            include_screen,
            max_output_bytes,
            json,
        } => handle_snapshot_pane(pane, blocks, include_screen, max_output_bytes, json, ctx),
        Request::WaitForText { .. } => Response::Error {
            message: "control_server: wait-for-text must dispatch asynchronously".into(),
        },
        Request::SharePane { pane, scrollback } => handle_share_pane(pane, scrollback, ctx),
        Request::SharePaneLink { pane } => handle_share_pane_link(pane, ctx),
        Request::UnsharePane { pane } => handle_unshare_pane(pane, ctx),
        Request::NewTab { config } => handle_new_tab(config, ctx),
        Request::CloseTab { tab } => handle_close_tab(tab, ctx),
        Request::ListBlocks { pane, limit } => handle_list_blocks(pane, limit, ctx),
        Request::SplitPane { pane, direction } => handle_split_pane(pane, direction, ctx),
        Request::FocusTab { tab } => handle_focus_tab(tab, ctx),
        Request::FocusPane { pane } => handle_focus_pane(pane, ctx),
        Request::ClosePane { pane } => handle_close_pane(pane, ctx),
        Request::ReadBlock { block } => handle_read_block(block, ctx),
        Request::WriteBytes { pane, bytes } => handle_write_bytes(pane, bytes, ctx),
        Request::Keystroke { pane, key } => handle_keystroke(pane, key, ctx),
    }
}

// -------- helpers ----------------------------------------------------------

fn entity_id_to_u64(id: EntityId) -> u64 {
    // EntityId implements Display as its inner usize — round-trip via string.
    id.to_string().parse::<u64>().unwrap_or(0)
}

/// Return the Workspace control commands should target.
///
/// Prefer the frontmost Warp window (matches `crate::root_view::active_workspace`),
/// so multi-window users don't see commands hit an arbitrary workspace
/// (the `WorkspaceRegistry` HashMap iterates in arbitrary order).
///
/// When no Warp window is frontmost — the common case when invoking the
/// CLI from another terminal — fall back to any registered workspace,
/// since callers still expect *some* sensible target. With a single Warp
/// instance running, this is unambiguous; with multiple, the user can
/// focus the desired window first to disambiguate.
fn active_workspace(ctx: &AppContext) -> Option<ViewHandle<Workspace>> {
    if let Some(window_id) = ctx.windows().active_window() {
        if let Some(ws) = WorkspaceRegistry::as_ref(ctx).get(window_id, ctx) {
            return Some(ws);
        }
    }
    WorkspaceRegistry::as_ref(ctx)
        .all_workspaces(ctx)
        .into_iter()
        .map(|(_, ws)| ws)
        .next()
}

/// Find the `ViewHandle<TerminalView>` for a wire pane id. Wire ids are the
/// `EntityId` of the terminal view (matching what `list_tab_pane_groups`
/// returns in `terminal_ids`), so we iterate panes and compare ids.
fn lookup_terminal_view(wire_pane_id: u64, ctx: &AppContext) -> Option<ViewHandle<TerminalView>> {
    let workspace = active_workspace(ctx)?;
    let ws = workspace.as_ref(ctx);
    for tab in ws.tabs.iter() {
        let pg = tab.pane_group.as_ref(ctx);
        let pane_ids: Vec<_> = pg.terminal_pane_ids().collect();
        for pid in pane_ids {
            if let Some(view) = pg.terminal_view_from_pane_id(pid, ctx) {
                if entity_id_to_u64(view.id()) == wire_pane_id {
                    return Some(view);
                }
            }
        }
    }
    None
}

fn resolve_terminal_view(
    pane: Option<u64>,
    ctx: &AppContext,
) -> Result<(u64, ViewHandle<TerminalView>), Response> {
    let pane_wire = pane
        .or_else(|| first_pane_wire_id(ctx))
        .ok_or_else(|| Response::Error {
            message: "no pane specified and no focused pane found".into(),
        })?;
    let view_handle = lookup_terminal_view(pane_wire, ctx).ok_or_else(|| Response::Error {
        message: format!("pane {pane_wire} not found"),
    })?;
    Ok((pane_wire, view_handle))
}

/// The default pane for commands that omit `--pane`: the focused pane of the
/// active tab. Falls back to the first terminal pane in the active tab, then
/// to the first terminal pane overall.
fn first_pane_wire_id(ctx: &AppContext) -> Option<u64> {
    let workspace = active_workspace(ctx)?;
    let ws = workspace.as_ref(ctx);
    let active_idx = ws.active_tab_index();
    if let Some(active_tab) = ws.tabs.get(active_idx) {
        let pg = active_tab.pane_group.as_ref(ctx);
        let focused = pg.focused_pane_id(ctx);
        if let Some(view) = pg.terminal_view_from_pane_id(focused, ctx) {
            return Some(entity_id_to_u64(view.id()));
        }
        for pid in pg.terminal_pane_ids() {
            if let Some(view) = pg.terminal_view_from_pane_id(pid, ctx) {
                return Some(entity_id_to_u64(view.id()));
            }
        }
    }
    let groups = ws.list_tab_pane_groups(ctx);
    let first = groups.first()?;
    let term = first.terminal_ids.first()?;
    Some(entity_id_to_u64(*term))
}

// -------- handlers ---------------------------------------------------------

fn handle_list_tabs(ctx: &mut AppContext) -> Response {
    let Some(workspace) = active_workspace(ctx) else {
        return Response::Tabs { tabs: vec![] };
    };
    let ws = workspace.as_ref(ctx);
    let active_idx = ws.active_tab_index();
    let mut tabs = Vec::new();
    for (tab_idx, tab) in ws.tabs.iter().enumerate() {
        let pg = tab.pane_group.as_ref(ctx);
        let pane_ids: Vec<u64> = visible_terminal_views(pg, ctx)
            .into_iter()
            .map(|view| entity_id_to_u64(view.id()))
            .collect();
        tabs.push(TabSummary {
            id: entity_id_to_u64(tab.pane_group.id()),
            index: tab_idx,
            title: None,
            active: tab_idx == active_idx,
            pane_ids,
        });
    }
    Response::Tabs { tabs }
}

/// `PaneGroup::terminal_pane_ids` still returns panes that are hidden for
/// close (the undo-close machinery keeps them in `pane_contents`).
/// Filter those out so list responses match what the user sees.
fn visible_terminal_views(pg: &PaneGroup, ctx: &AppContext) -> Vec<ViewHandle<TerminalView>> {
    pg.terminal_pane_ids()
        .filter(|pid| !pg.is_pane_hidden_for_close(*pid))
        .filter_map(|pid| pg.terminal_view_from_pane_id(pid, ctx))
        .collect()
}

struct PaneActivitySummary {
    status: PaneStatus,
    running: bool,
    foreground_process: Option<String>,
    preview: Option<String>,
}

fn pane_activity_summary(
    view: &TerminalView,
    include_preview: bool,
    ctx: &AppContext,
) -> PaneActivitySummary {
    let model = view.model.lock();
    let active_block = model.block_list().active_block();
    let active_command = active_block.command_to_string();
    let running = pane_is_running(&model, active_command.trim().is_empty());
    let foreground_process = if running {
        active_block
            .top_level_command(view.sessions_model().as_ref(ctx))
            .or_else(|| foreground_process_from_command(&active_command))
            .map(strip_command_path)
    } else {
        None
    };
    let preview = include_preview.then(|| pane_preview(&model)).flatten();

    PaneActivitySummary {
        status: if running {
            PaneStatus::Running
        } else {
            PaneStatus::Idle
        },
        running,
        foreground_process,
        preview,
    }
}

fn pane_is_running(model: &TerminalModel, active_command_is_empty: bool) -> bool {
    let block_list = model.block_list();
    let active_block = block_list.active_block();
    !block_list.is_bootstrapped()
        || active_block.is_executing()
        || (active_block.started() && !active_command_is_empty && !active_block.is_done())
}

fn foreground_process_from_command(command: &str) -> Option<String> {
    warp_completer::parsers::simple::top_level_command(
        command,
        warp_util::path::EscapeChar::Backslash,
    )
    .or_else(|| {
        command
            .split_whitespace()
            .find(|part| !part.contains('='))
            .map(ToOwned::to_owned)
    })
}

fn strip_command_path(command: String) -> String {
    std::path::Path::new(&command)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&command)
        .to_string()
}

fn pane_preview(model: &TerminalModel) -> Option<String> {
    let (alt_screen, screen_text) = model.screen_to_string();
    if alt_screen {
        return preview_line_from_text(&screen_text);
    }

    preview_line_from_blocks(model).or_else(|| preview_line_from_text(&screen_text))
}

fn preview_line_from_blocks(model: &TerminalModel) -> Option<String> {
    for block in model.block_list().blocks().iter().rev() {
        if let Some(line) = preview_line_from_text(&block.output_to_string()) {
            return Some(line);
        }
        if let Some(line) = preview_line_from_text(&block.command_to_string()) {
            return Some(line);
        }
    }
    None
}

fn preview_line_from_text(text: &str) -> Option<String> {
    text.lines()
        .rev()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| truncate_preview_line(line, PANE_LIST_PREVIEW_MAX_CHARS))
}

fn truncate_preview_line(line: &str, max_chars: usize) -> String {
    let normalized = line.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max_chars {
        return normalized;
    }

    let keep = max_chars.saturating_sub(3);
    let mut truncated = normalized.chars().take(keep).collect::<String>();
    truncated.push_str("...");
    truncated
}

fn handle_list_panes(
    filter_tab: Option<u64>,
    include_preview: bool,
    json: bool,
    ctx: &mut AppContext,
) -> Response {
    let Some(workspace) = active_workspace(ctx) else {
        return Response::Panes {
            panes: vec![],
            include_preview,
            json,
        };
    };
    let ws = workspace.as_ref(ctx);
    let active_idx = ws.active_tab_index();
    let mut panes = Vec::new();
    for (tab_idx, tab) in ws.tabs.iter().enumerate() {
        let tab_id = entity_id_to_u64(tab.pane_group.id());
        if let Some(want) = filter_tab {
            if want != tab_id && want != tab_idx as u64 {
                continue;
            }
        }
        let pg = tab.pane_group.as_ref(ctx);
        let focused_pid = pg.focused_pane_id(ctx);
        let focused_view_id = pg
            .terminal_view_from_pane_id(focused_pid, ctx)
            .map(|v| entity_id_to_u64(v.id()));
        for view in visible_terminal_views(pg, ctx) {
            let wire_id = entity_id_to_u64(view.id());
            let terminal_view = view.as_ref(ctx);
            let cwd = terminal_view.pwd();
            let activity = pane_activity_summary(terminal_view, include_preview, ctx);
            let is_focused = tab_idx == active_idx && focused_view_id == Some(wire_id);
            panes.push(PaneSummary {
                id: wire_id,
                tab_id,
                tab_index: tab_idx,
                title: None,
                cwd,
                focused: is_focused,
                status: activity.status,
                running: activity.running,
                foreground_process: activity.foreground_process,
                preview: activity.preview,
            });
        }
    }
    Response::Panes {
        panes,
        include_preview,
        json,
    }
}

fn handle_send_input(pane: Option<u64>, text: String, ctx: &mut AppContext) -> Response {
    match submit_send_input(pane, text, ctx) {
        Ok(_) => Response::Ok,
        Err(response) => response,
    }
}

struct SubmittedCommand {
    pane_wire: u64,
    block_id: String,
}

fn submit_send_input(
    pane: Option<u64>,
    text: String,
    ctx: &mut AppContext,
) -> Result<SubmittedCommand, Response> {
    let pane_wire = pane
        .or_else(|| first_pane_wire_id(ctx))
        .ok_or_else(|| Response::Error {
            message: "no pane specified and no focused pane found".into(),
        })?;
    let view_handle = lookup_terminal_view(pane_wire, ctx).ok_or_else(|| Response::Error {
        message: format!("pane {pane_wire} not found"),
    })?;
    let (result, block_id) = view_handle.update(ctx, |view, ctx| {
        let result = view.execute_command_or_set_pending(&text, ctx);
        let block_id = if matches!(result, CommandExecutionResult::Executed) {
            Some(view.model.lock().block_list().active_block_id().to_string())
        } else {
            None
        };
        (result, block_id)
    });
    match result {
        CommandExecutionResult::Executed => {
            let Some(block_id) = block_id else {
                return Err(Response::Error {
                    message: "command executed but no block id was captured".into(),
                });
            };
            Ok(SubmittedCommand {
                pane_wire,
                block_id,
            })
        }
        CommandExecutionResult::Blocked(reason) => Err(Response::Error {
            message: deny_execution_message(reason).into(),
        }),
        CommandExecutionResult::NotExecuted => Err(Response::Error {
            message: "command was not executed".into(),
        }),
    }
}

async fn handle_send_input_wait(
    pane: Option<u64>,
    text: String,
    timeout_ms: Option<u64>,
    spawner: warpui::ModelSpawner<ControlModel>,
) -> Response {
    let submitted = match spawner
        .spawn(move |_me, ctx| submit_send_input(pane, text, ctx))
        .await
    {
        Ok(Ok(submitted)) => submitted,
        Ok(Err(response)) => return response,
        Err(_) => {
            return Response::Error {
                message: "control_server: dispatch dropped (model gone)".into(),
            }
        }
    };

    wait_for_submitted_block(submitted, timeout_ms, spawner).await
}

enum BlockWaitPoll {
    Running(BlockEntry),
    Done(BlockEntry),
    Error(Response),
}

async fn wait_for_submitted_block(
    submitted: SubmittedCommand,
    timeout_ms: Option<u64>,
    spawner: warpui::ModelSpawner<ControlModel>,
) -> Response {
    let timeout_ms = timeout_ms.unwrap_or(SEND_WAIT_DEFAULT_TIMEOUT_MS);
    let timeout = Duration::from_millis(timeout_ms);
    let started = Instant::now();

    loop {
        let pane_wire = submitted.pane_wire;
        let block_id = submitted.block_id.clone();
        let poll = spawner
            .spawn(move |_me, ctx| poll_submitted_block(pane_wire, block_id, ctx))
            .await
            .unwrap_or_else(|_| {
                BlockWaitPoll::Error(Response::Error {
                    message: "control_server: dispatch dropped (model gone)".into(),
                })
            });

        match poll {
            BlockWaitPoll::Done(block) => return Response::Block { block },
            BlockWaitPoll::Running(block) => {
                if started.elapsed() >= timeout {
                    return Response::SendTimedOut {
                        pane: submitted.pane_wire,
                        timeout_ms,
                        block,
                    };
                }
            }
            BlockWaitPoll::Error(response) => return response,
        }

        let remaining = timeout.saturating_sub(started.elapsed());
        Timer::after(remaining.min(SEND_WAIT_POLL_INTERVAL)).await;
    }
}

fn poll_submitted_block(pane_wire: u64, block_id: String, ctx: &mut AppContext) -> BlockWaitPoll {
    use crate::terminal::model::BlockId;

    let Some(view_handle) = lookup_terminal_view(pane_wire, ctx) else {
        return BlockWaitPoll::Error(Response::Error {
            message: format!("pane {pane_wire} not found"),
        });
    };
    let needle = BlockId::from(block_id.clone());
    let model = view_handle.as_ref(ctx).model.lock();
    let Some(block) = model.block_list().block_with_id(&needle) else {
        return BlockWaitPoll::Error(Response::Error {
            message: format!("block {block_id} not found"),
        });
    };
    let entry = block_to_entry(block, pane_wire);
    if block.is_done() {
        BlockWaitPoll::Done(entry)
    } else {
        BlockWaitPoll::Running(entry)
    }
}

fn deny_execution_message(reason: DenyExecutionReason) -> &'static str {
    match reason {
        DenyExecutionReason::NotBootstrapped => {
            "pane is not ready to execute commands yet because shell bootstrapping is still in progress"
        }
        DenyExecutionReason::ExistingActiveCommand => {
            "pane cannot execute commands while another command is already running"
        }
        DenyExecutionReason::HistoryNotAppendable => {
            "pane cannot execute commands because its history is not appendable"
        }
    }
}

fn handle_read_pane(pane: Option<u64>, blocks: usize, ctx: &mut AppContext) -> Response {
    let pane_wire = match pane.or_else(|| first_pane_wire_id(ctx)) {
        Some(p) => p,
        None => {
            return Response::Error {
                message: "no pane specified and no focused pane found".into(),
            }
        }
    };
    let Some(view_handle) = lookup_terminal_view(pane_wire, ctx) else {
        return Response::Error {
            message: format!("pane {pane_wire} not found"),
        };
    };
    let entries = view_handle.update(ctx, |view, _ctx| {
        let model = view.model.lock();
        let block_list = model.block_list();
        let all = block_list.blocks();
        let take = blocks.min(all.len());
        let start = all.len().saturating_sub(take);
        all[start..]
            .iter()
            .map(|b| block_to_entry(b, pane_wire))
            .collect::<Vec<_>>()
    });
    Response::PaneOutput {
        pane: pane_wire,
        blocks: entries,
    }
}

fn handle_read_screen(pane: Option<u64>, ctx: &mut AppContext) -> Response {
    let pane_wire = match pane.or_else(|| first_pane_wire_id(ctx)) {
        Some(p) => p,
        None => {
            return Response::Error {
                message: "no pane specified and no focused pane found".into(),
            }
        }
    };
    let Some(view_handle) = lookup_terminal_view(pane_wire, ctx) else {
        return Response::Error {
            message: format!("pane {pane_wire} not found"),
        };
    };
    let (alt_screen, text) = view_handle.update(ctx, |view, _ctx| {
        let model = view.model.lock();
        model.screen_to_string()
    });
    Response::Screen {
        pane: pane_wire,
        alt_screen,
        text,
    }
}

fn handle_snapshot_pane(
    pane: Option<u64>,
    blocks: usize,
    include_screen: bool,
    max_output_bytes: usize,
    json: bool,
    ctx: &mut AppContext,
) -> Response {
    match collect_pane_snapshot(pane, blocks, include_screen, max_output_bytes, ctx) {
        Ok(snapshot) => Response::PaneSnapshot { snapshot, json },
        Err(response) => response,
    }
}

fn collect_pane_snapshot(
    pane: Option<u64>,
    blocks: usize,
    include_screen: bool,
    max_output_bytes: usize,
    ctx: &mut AppContext,
) -> Result<PaneSnapshot, Response> {
    let (pane_wire, view_handle) = resolve_terminal_view(pane, ctx)?;
    let pane = pane_snapshot_summary(pane_wire, ctx).unwrap_or_else(|| {
        let cwd = view_handle.as_ref(ctx).pwd();
        PaneSnapshotPane {
            id: pane_wire,
            tab_id: 0,
            tab_index: 0,
            title: None,
            cwd,
            focused: false,
        }
    });

    let (screen, blocks) = view_handle.update(ctx, |view, _ctx| {
        let model = view.model.lock();
        let screen = if include_screen {
            let (alt_screen, text) = model.screen_to_string();
            let (text, text_truncated) = truncate_text(text, max_output_bytes);
            Some(PaneScreenSnapshot {
                alt_screen,
                text,
                text_truncated,
            })
        } else {
            None
        };

        let block_list = model.block_list();
        let all = block_list.blocks();
        let take = blocks.min(all.len());
        let start = all.len().saturating_sub(take);
        let blocks = all[start..]
            .iter()
            .map(|b| {
                let mut entry = block_to_entry(b, pane_wire);
                let (output, output_truncated) = truncate_text(entry.output, max_output_bytes);
                entry.output = output;
                entry.output_truncated = output_truncated;
                entry
            })
            .collect::<Vec<_>>();

        (screen, blocks)
    });

    Ok(PaneSnapshot {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        captured_at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        pane,
        screen,
        blocks,
    })
}

fn pane_snapshot_summary(pane_wire: u64, ctx: &AppContext) -> Option<PaneSnapshotPane> {
    let workspace = active_workspace(ctx)?;
    let ws = workspace.as_ref(ctx);
    let active_idx = ws.active_tab_index();
    for (tab_idx, tab) in ws.tabs.iter().enumerate() {
        let tab_id = entity_id_to_u64(tab.pane_group.id());
        let pg = tab.pane_group.as_ref(ctx);
        let focused_pid = pg.focused_pane_id(ctx);
        let focused_view_id = pg
            .terminal_view_from_pane_id(focused_pid, ctx)
            .map(|v| entity_id_to_u64(v.id()));
        for view in visible_terminal_views(pg, ctx) {
            let wire_id = entity_id_to_u64(view.id());
            if wire_id == pane_wire {
                return Some(PaneSnapshotPane {
                    id: wire_id,
                    tab_id,
                    tab_index: tab_idx,
                    title: None,
                    cwd: view.as_ref(ctx).pwd(),
                    focused: tab_idx == active_idx && focused_view_id == Some(wire_id),
                });
            }
        }
    }
    None
}

fn truncate_text(text: String, max_bytes: usize) -> (String, bool) {
    if max_bytes == 0 || text.len() <= max_bytes {
        return (text, false);
    }

    let mut start = text.len().saturating_sub(max_bytes);
    while !text.is_char_boundary(start) {
        start += 1;
    }
    (text[start..].to_string(), true)
}

struct WaitForTextOptions {
    pane: Option<u64>,
    text: String,
    regex: bool,
    timeout_ms: u64,
    mode: WaitForTextMode,
    case_insensitive: bool,
    since: WaitForTextSince,
    blocks: usize,
    block_field: WaitForTextBlockField,
    max_output_bytes: usize,
    json: bool,
}

#[derive(Default)]
struct WaitForTextBaseline {
    screen_text: Option<String>,
    block_text_by_id: HashMap<String, String>,
}

async fn handle_wait_for_text(
    options: WaitForTextOptions,
    spawner: warpui::ModelSpawner<ControlModel>,
) -> Response {
    if options.text.is_empty() {
        return Response::Error {
            message: "text must not be empty".into(),
        };
    }

    if options.regex {
        if let Err(err) = RegexBuilder::new(&options.text)
            .case_insensitive(options.case_insensitive)
            .build()
        {
            return Response::Error {
                message: format!("invalid regex: {err}"),
            };
        }
    }

    let timeout = Duration::from_millis(options.timeout_ms);
    let started = Instant::now();
    let include_screen = options.mode.includes_screen();
    let blocks = if options.mode.includes_blocks() {
        options.blocks
    } else {
        0
    };

    let baseline = if options.since == WaitForTextSince::Now {
        let pane = options.pane;
        match dispatch_snapshot_for_wait(pane, blocks, include_screen, usize::MAX, &spawner).await {
            Ok(snapshot) => WaitForTextBaseline::from_snapshot(&snapshot, options.block_field),
            Err(response) => return response,
        }
    } else {
        WaitForTextBaseline::default()
    };

    loop {
        let pane = options.pane;
        let snapshot =
            match dispatch_snapshot_for_wait(pane, blocks, include_screen, usize::MAX, &spawner)
                .await
            {
                Ok(snapshot) => snapshot,
                Err(response) => return response,
            };

        if let Some(matched) = find_text_match(&snapshot, &options, &baseline) {
            let elapsed_ms = elapsed_ms(started);
            let snapshot = options
                .json
                .then(|| truncate_snapshot(snapshot, options.max_output_bytes));
            return Response::WaitForTextMatched {
                pane: matched.pane_id,
                elapsed_ms,
                matched,
                snapshot,
                json: options.json,
            };
        }

        if started.elapsed() >= timeout {
            let pane = snapshot.pane.id;
            let snapshot = options
                .json
                .then(|| truncate_snapshot(snapshot, options.max_output_bytes));
            return Response::WaitForTextTimedOut {
                pane,
                timeout_ms: options.timeout_ms,
                elapsed_ms: elapsed_ms(started),
                snapshot,
                json: options.json,
            };
        }

        let remaining = timeout.saturating_sub(started.elapsed());
        Timer::after(remaining.min(WAIT_TEXT_POLL_INTERVAL)).await;
    }
}

async fn dispatch_snapshot_for_wait(
    pane: Option<u64>,
    blocks: usize,
    include_screen: bool,
    max_output_bytes: usize,
    spawner: &warpui::ModelSpawner<ControlModel>,
) -> Result<PaneSnapshot, Response> {
    spawner
        .spawn(move |_me, ctx| {
            collect_pane_snapshot(pane, blocks, include_screen, max_output_bytes, ctx)
        })
        .await
        .unwrap_or_else(|_| {
            Err(Response::Error {
                message: "control_server: dispatch dropped (model gone)".into(),
            })
        })
}

impl WaitForTextBaseline {
    fn from_snapshot(snapshot: &PaneSnapshot, block_field: WaitForTextBlockField) -> Self {
        Self {
            screen_text: snapshot.screen.as_ref().map(|screen| screen.text.clone()),
            block_text_by_id: snapshot
                .blocks
                .iter()
                .map(|block| (block.id.clone(), block_search_text(block, block_field)))
                .collect(),
        }
    }
}

fn elapsed_ms(started: Instant) -> u64 {
    started.elapsed().as_millis().try_into().unwrap_or(u64::MAX)
}

fn truncate_snapshot(mut snapshot: PaneSnapshot, max_output_bytes: usize) -> PaneSnapshot {
    if let Some(screen) = &mut snapshot.screen {
        let (text, text_truncated) =
            truncate_text(std::mem::take(&mut screen.text), max_output_bytes);
        screen.text = text;
        screen.text_truncated = text_truncated;
    }

    for block in &mut snapshot.blocks {
        let (output, output_truncated) =
            truncate_text(std::mem::take(&mut block.output), max_output_bytes);
        block.output = output;
        block.output_truncated = output_truncated;
    }

    snapshot
}

fn find_text_match(
    snapshot: &PaneSnapshot,
    options: &WaitForTextOptions,
    baseline: &WaitForTextBaseline,
) -> Option<TextMatch> {
    if options.mode.includes_screen() {
        if let Some(screen) = &snapshot.screen {
            let haystack =
                text_after_baseline(&screen.text, baseline.screen_text.as_deref(), options.since);
            if let Some((text, line)) = find_in_text(
                haystack,
                &options.text,
                options.regex,
                options.case_insensitive,
            ) {
                return Some(TextMatch {
                    source: TextMatchSource::Screen,
                    pane_id: snapshot.pane.id,
                    block_id: None,
                    text,
                    line,
                });
            }
        }
    }

    if options.mode.includes_blocks() {
        for block in snapshot.blocks.iter().rev() {
            let text = block_search_text(block, options.block_field);
            let haystack = text_after_baseline(
                &text,
                baseline.block_text_by_id.get(&block.id).map(String::as_str),
                options.since,
            );
            if let Some((text, line)) = find_in_text(
                haystack,
                &options.text,
                options.regex,
                options.case_insensitive,
            ) {
                return Some(TextMatch {
                    source: TextMatchSource::Block,
                    pane_id: snapshot.pane.id,
                    block_id: Some(block.id.clone()),
                    text,
                    line,
                });
            }
        }
    }

    None
}

fn block_search_text(block: &BlockEntry, field: WaitForTextBlockField) -> String {
    match field {
        WaitForTextBlockField::Output => block.output.clone(),
        WaitForTextBlockField::Command => block.command.clone().unwrap_or_default(),
        WaitForTextBlockField::Both => match &block.command {
            Some(command) if !block.output.is_empty() => {
                format!("{command}\n{}", block.output)
            }
            Some(command) => command.clone(),
            None => block.output.clone(),
        },
    }
}

fn text_after_baseline<'a>(
    text: &'a str,
    baseline: Option<&str>,
    since: WaitForTextSince,
) -> &'a str {
    if since == WaitForTextSince::Now {
        if let Some(baseline) = baseline {
            if let Some(rest) = text.strip_prefix(baseline) {
                return rest;
            }
        }
    }
    text
}

fn find_in_text(
    haystack: &str,
    needle: &str,
    regex: bool,
    case_insensitive: bool,
) -> Option<(String, Option<String>)> {
    if regex {
        let re = RegexBuilder::new(needle)
            .case_insensitive(case_insensitive)
            .build()
            .ok()?;
        let found = re.find(haystack)?;
        let matched = found.as_str().to_string();
        let line = line_containing(haystack, found.start(), found.end());
        return Some((matched, line));
    }

    if case_insensitive {
        let re = RegexBuilder::new(&regex::escape(needle))
            .case_insensitive(true)
            .build()
            .ok()?;
        let found = re.find(haystack)?;
        let matched = found.as_str().to_string();
        let line = line_containing(haystack, found.start(), found.end());
        Some((matched, line))
    } else {
        let start = haystack.find(needle)?;
        let end = start + needle.len();
        let line = line_containing(haystack, start, end);
        Some((needle.to_string(), line))
    }
}

fn line_containing(text: &str, start: usize, end: usize) -> Option<String> {
    let line_start = text[..start].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
    let line_end = text[end..]
        .find('\n')
        .map(|idx| end + idx)
        .unwrap_or(text.len());
    text.get(line_start..line_end)
        .map(str::trim_end)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
}

fn handle_share_pane(
    pane: Option<u64>,
    scrollback: ShareScrollback,
    ctx: &mut AppContext,
) -> Response {
    let (pane_wire, view_handle) = match resolve_terminal_view(pane, ctx) {
        Ok(resolved) => resolved,
        Err(response) => return response,
    };
    let scrollback_type = match scrollback {
        ShareScrollback::None => SharedSessionScrollbackType::None,
        ShareScrollback::All => SharedSessionScrollbackType::All,
    };

    view_handle.update(ctx, |view, ctx| {
        let status = view.model.lock().shared_session_status().clone();
        if status.is_viewer() {
            return Response::Error {
                message: format!("pane {pane_wire} is viewing a shared session"),
            };
        }
        if !status.is_sharer() {
            let share_source = SharedSessionSource::user(
                view.active_conversation_task_id(ctx)
                    .map(|task| task.to_string()),
            );
            view.attempt_to_share_session(
                scrollback_type,
                Some(SharedSessionActionSource::NonUser),
                share_source,
                true,
                ctx,
            );
        }
        Response::ShareStarted { pane: pane_wire }
    })
}

fn handle_share_pane_link(pane: Option<u64>, ctx: &mut AppContext) -> Response {
    let (pane_wire, view_handle) = match resolve_terminal_view(pane, ctx) {
        Ok(resolved) => resolved,
        Err(response) => return response,
    };

    if let Some(session_id) = SharedSessionManager::as_ref(ctx).session_id(&view_handle.id()) {
        return Response::ShareLink {
            pane: pane_wire,
            url: join_link(&session_id),
        };
    }

    let is_sharer = view_handle
        .as_ref(ctx)
        .model
        .lock()
        .shared_session_status()
        .is_sharer();
    if is_sharer {
        Response::SharePending { pane: pane_wire }
    } else {
        Response::Error {
            message: format!("pane {pane_wire} is not sharing; run pane share first"),
        }
    }
}

fn handle_unshare_pane(pane: Option<u64>, ctx: &mut AppContext) -> Response {
    let (pane_wire, view_handle) = match resolve_terminal_view(pane, ctx) {
        Ok(resolved) => resolved,
        Err(response) => return response,
    };

    view_handle.update(ctx, |view, ctx| {
        let is_sharer = view.model.lock().shared_session_status().is_sharer();
        if !is_sharer {
            return Response::Error {
                message: format!("pane {pane_wire} is not sharing"),
            };
        }
        view.stop_sharing_session(SharedSessionActionSource::NonUser, ctx);
        Response::ShareStopped { pane: pane_wire }
    })
}

fn block_to_entry(b: &crate::terminal::model::block::Block, pane_wire: u64) -> BlockEntry {
    let command = b.prompt_and_command_grid().contents_to_string(false, None);
    let command = if command.trim().is_empty() {
        None
    } else {
        Some(command)
    };
    let output = b.output_grid().contents_to_string(false, None);
    BlockEntry {
        id: b.id().to_string(),
        pane_id: pane_wire,
        command,
        output,
        output_truncated: false,
        exit_code: b.is_done().then(|| b.exit_code().value()),
        pwd: b.pwd().cloned(),
        started_at: b.start_ts().map(|ts| ts.to_rfc3339()),
        completed_at: b.completed_ts().map(|ts| ts.to_rfc3339()),
    }
}

fn handle_list_blocks(pane: Option<u64>, limit: usize, ctx: &mut AppContext) -> Response {
    let pane_wire = match pane.or_else(|| first_pane_wire_id(ctx)) {
        Some(p) => p,
        None => {
            return Response::Error {
                message: "no pane specified and no focused pane found".into(),
            }
        }
    };
    let Some(view_handle) = lookup_terminal_view(pane_wire, ctx) else {
        return Response::Error {
            message: format!("pane {pane_wire} not found"),
        };
    };
    let entries = view_handle.update(ctx, |view, _ctx| {
        let model = view.model.lock();
        let block_list = model.block_list();
        let all = block_list.blocks();
        let take = limit.min(all.len());
        let start = all.len().saturating_sub(take);
        all[start..]
            .iter()
            .map(|b| block_to_entry(b, pane_wire))
            .collect::<Vec<_>>()
    });
    Response::Blocks { blocks: entries }
}

fn handle_split_pane(pane: Option<u64>, direction: SplitDir, ctx: &mut AppContext) -> Response {
    let pane_wire = match pane.or_else(|| first_pane_wire_id(ctx)) {
        Some(p) => p,
        None => {
            return Response::Error {
                message: "no pane specified and no focused pane found".into(),
            }
        }
    };
    let Some(pane_group) = pane_group_for_pane(pane_wire, ctx) else {
        return Response::Error {
            message: format!("pane {pane_wire} not found"),
        };
    };
    let dir = match direction {
        SplitDir::Left => PaneDirection::Left,
        SplitDir::Right => PaneDirection::Right,
        SplitDir::Up => PaneDirection::Up,
        SplitDir::Down => PaneDirection::Down,
    };
    // `PaneGroup::add_terminal_pane` uses `focused_pane_id` as the split
    // source, so to honor `--pane <id>` we have to focus the requested pane
    // first.
    let target = EntityId::from_usize(pane_wire as usize);
    pane_group.update(ctx, |pg, ctx| {
        pg.handle_action(&PaneGroupAction::FocusTerminalView(target), ctx);
        pg.handle_action(&PaneGroupAction::Add(dir), ctx);
    });
    Response::Ok
}

fn handle_focus_tab(tab: u64, ctx: &mut AppContext) -> Response {
    let Some(workspace) = active_workspace(ctx) else {
        return Response::Error {
            message: "no active workspace".into(),
        };
    };
    let groups = workspace.as_ref(ctx).list_tab_pane_groups(ctx);
    let index = groups.iter().find_map(|tpg| {
        if entity_id_to_u64(tpg.pane_group_id) == tab || tpg.tab_idx as u64 == tab {
            Some(tpg.tab_idx)
        } else {
            None
        }
    });
    let Some(index) = index else {
        return Response::Error {
            message: format!("tab {tab} not found"),
        };
    };
    workspace.update(ctx, |ws, ctx| {
        ws.handle_action(&WorkspaceAction::ActivateTab(index), ctx);
    });
    Response::Ok
}

fn handle_focus_pane(pane: u64, ctx: &mut AppContext) -> Response {
    let Some(workspace) = active_workspace(ctx) else {
        return Response::Error {
            message: "no active workspace".into(),
        };
    };
    // Find the tab index containing this pane so we can activate the tab
    // before focusing the pane inside it. Otherwise focusing a pane in a
    // background tab just shuffles hidden state.
    let target = EntityId::from_usize(pane as usize);
    let mut owner_tab_idx: Option<usize> = None;
    let mut owner_pane_group: Option<warpui::ViewHandle<PaneGroup>> = None;
    {
        let ws = workspace.as_ref(ctx);
        for (idx, tab) in ws.tabs.iter().enumerate() {
            let pg = tab.pane_group.as_ref(ctx);
            for pid in pg.terminal_pane_ids() {
                if let Some(view) = pg.terminal_view_from_pane_id(pid, ctx) {
                    if entity_id_to_u64(view.id()) == pane {
                        owner_tab_idx = Some(idx);
                        owner_pane_group = Some(tab.pane_group.clone());
                        break;
                    }
                }
            }
            if owner_tab_idx.is_some() {
                break;
            }
        }
    }
    let Some(idx) = owner_tab_idx else {
        return Response::Error {
            message: format!("pane {pane} not found"),
        };
    };
    workspace.update(ctx, |ws, ctx| {
        ws.handle_action(&WorkspaceAction::ActivateTab(idx), ctx);
    });
    if let Some(pg) = owner_pane_group {
        pg.update(ctx, |pg, ctx| {
            pg.handle_action(&PaneGroupAction::FocusTerminalView(target), ctx);
        });
    }
    Response::Ok
}

fn handle_close_pane(pane: u64, ctx: &mut AppContext) -> Response {
    // We need the `PaneId` of the pane to close — `PaneGroup::close_pane`
    // takes a `PaneId`, not the `EntityId` we use as wire id. Find the
    // owning PaneGroup, look up the `PaneId` whose terminal view matches
    // our wire id, then close it without confirmation (control commands
    // are explicit).
    let Some(workspace) = active_workspace(ctx) else {
        return Response::Error {
            message: "no active workspace".into(),
        };
    };
    let ws = workspace.as_ref(ctx);
    let mut found: Option<(
        warpui::ViewHandle<PaneGroup>,
        crate::pane_group::pane::PaneId,
    )> = None;
    for tab in ws.tabs.iter() {
        let pg = tab.pane_group.as_ref(ctx);
        for pid in pg.terminal_pane_ids() {
            if let Some(view) = pg.terminal_view_from_pane_id(pid, ctx) {
                if entity_id_to_u64(view.id()) == pane {
                    found = Some((tab.pane_group.clone(), pid));
                    break;
                }
            }
        }
        if found.is_some() {
            break;
        }
    }
    let Some((pane_group, pid)) = found else {
        return Response::Error {
            message: format!("pane {pane} not found"),
        };
    };
    pane_group.update(ctx, |pg, ctx| {
        pg.close_pane(pid, ctx);
    });
    Response::Ok
}

fn handle_read_block(block_id: String, ctx: &mut AppContext) -> Response {
    use crate::terminal::model::BlockId;
    let Some(workspace) = active_workspace(ctx) else {
        return Response::Error {
            message: "no active workspace".into(),
        };
    };
    let needle = BlockId::from(block_id.clone());
    let ws = workspace.as_ref(ctx);
    for tab in ws.tabs.iter() {
        let pg = tab.pane_group.as_ref(ctx);
        for pid in pg.terminal_pane_ids() {
            let Some(view) = pg.terminal_view_from_pane_id(pid, ctx) else {
                continue;
            };
            let pane_wire = entity_id_to_u64(view.id());
            let entry = {
                let model = view.as_ref(ctx).model.lock();
                model
                    .block_list()
                    .block_with_id(&needle)
                    .map(|b| block_to_entry(b, pane_wire))
            };
            if let Some(entry) = entry {
                return Response::Block { block: entry };
            }
        }
    }
    Response::Error {
        message: format!("block {block_id} not found"),
    }
}

/// Find the `ViewHandle<PaneGroup>` that contains a given terminal pane (by
/// wire id == TerminalView EntityId).
fn pane_group_for_pane(
    wire_pane_id: u64,
    ctx: &AppContext,
) -> Option<warpui::ViewHandle<PaneGroup>> {
    let workspace = active_workspace(ctx)?;
    let ws = workspace.as_ref(ctx);
    for tab in ws.tabs.iter() {
        let pg = tab.pane_group.as_ref(ctx);
        for pid in pg.terminal_pane_ids() {
            if let Some(view) = pg.terminal_view_from_pane_id(pid, ctx) {
                if entity_id_to_u64(view.id()) == wire_pane_id {
                    return Some(tab.pane_group.clone());
                }
            }
        }
    }
    None
}

fn handle_write_bytes(pane: Option<u64>, bytes: Vec<u8>, ctx: &mut AppContext) -> Response {
    let pane_wire = match pane.or_else(|| first_pane_wire_id(ctx)) {
        Some(p) => p,
        None => {
            return Response::Error {
                message: "no pane specified and no focused pane found".into(),
            }
        }
    };
    let manager = match terminal_manager_for_pane(pane_wire, ctx) {
        Some(m) => m,
        None => {
            return Response::Error {
                message: format!("pane {pane_wire} not found"),
            }
        }
    };
    manager.update(ctx, |mgr, ctx| {
        mgr.write_pty_bytes(bytes, ctx);
    });
    Response::Ok
}

fn handle_keystroke(pane: Option<u64>, key: String, ctx: &mut AppContext) -> Response {
    let Some(bytes) = keystroke_to_bytes(&key) else {
        return Response::Error {
            message: format!("unknown key: {key:?}"),
        };
    };
    handle_write_bytes(pane, bytes, ctx)
}

/// Find the [`Box<dyn TerminalManager>`] handle for a pane id.
fn terminal_manager_for_pane(
    wire_pane_id: u64,
    ctx: &AppContext,
) -> Option<warpui::ModelHandle<Box<dyn crate::terminal::TerminalManager>>> {
    let workspace = active_workspace(ctx)?;
    let ws = workspace.as_ref(ctx);
    for tab in ws.tabs.iter() {
        let pg = tab.pane_group.as_ref(ctx);
        for pid in pg.terminal_pane_ids() {
            if let Some(view) = pg.terminal_view_from_pane_id(pid, ctx) {
                if entity_id_to_u64(view.id()) == wire_pane_id {
                    return pg.terminal_manager_by_id(pid, ctx);
                }
            }
        }
    }
    None
}

/// Map a key name (or `ctrl-<char>` chord) to the bytes the PTY expects.
/// Returns `None` for unknown names.
fn keystroke_to_bytes(key: &str) -> Option<Vec<u8>> {
    let lower = key.trim().to_ascii_lowercase();
    let bytes: &[u8] = match lower.as_str() {
        "enter" | "return" | "\\n" => b"\r",
        "tab" | "\\t" => b"\t",
        "esc" | "escape" => b"\x1b",
        "space" => b" ",
        "backspace" | "bs" => b"\x7f",
        "delete" | "del" => b"\x1b[3~",
        "ins" | "insert" => b"\x1b[2~",
        "up" => b"\x1b[A",
        "down" => b"\x1b[B",
        "right" => b"\x1b[C",
        "left" => b"\x1b[D",
        "home" => b"\x1b[H",
        "end" => b"\x1b[F",
        "pageup" | "pgup" => b"\x1b[5~",
        "pagedown" | "pgdn" => b"\x1b[6~",
        "f1" => b"\x1bOP",
        "f2" => b"\x1bOQ",
        "f3" => b"\x1bOR",
        "f4" => b"\x1bOS",
        "f5" => b"\x1b[15~",
        "f6" => b"\x1b[17~",
        "f7" => b"\x1b[18~",
        "f8" => b"\x1b[19~",
        "f9" => b"\x1b[20~",
        "f10" => b"\x1b[21~",
        "f11" => b"\x1b[23~",
        "f12" => b"\x1b[24~",
        _ => {
            if let Some(rest) = lower
                .strip_prefix("ctrl-")
                .or_else(|| lower.strip_prefix("c-"))
            {
                if rest.len() == 1 {
                    let c = rest.as_bytes()[0];
                    // ctrl-<letter> is the ASCII control char (0x01..=0x1A).
                    // ctrl-space → 0x00. ctrl-[ → 0x1b (esc). ctrl-] → 0x1d.
                    let code = match c {
                        b'@' | b' ' => 0u8,
                        b'a'..=b'z' => c - b'a' + 1,
                        b'A'..=b'Z' => c - b'A' + 1,
                        b'[' => 0x1b,
                        b'\\' => 0x1c,
                        b']' => 0x1d,
                        b'^' => 0x1e,
                        b'_' => 0x1f,
                        b'?' => 0x7f,
                        _ => return None,
                    };
                    return Some(vec![code]);
                }
            }
            return None;
        }
    };
    Some(bytes.to_vec())
}

fn handle_new_tab(config: Option<String>, ctx: &mut AppContext) -> Response {
    let Some(workspace) = active_workspace(ctx) else {
        return Response::Error {
            message: "no active workspace".into(),
        };
    };

    // With a config name, open the matching saved tab config (e.g. an SSH tab)
    // via the same action the new-session menu uses. Without one, open a plain
    // terminal tab.
    if let Some(name) = config {
        let tab_config = WarpConfig::as_ref(ctx)
            .tab_configs()
            .iter()
            .find(|tc| tc.name == name)
            .cloned();
        let Some(tab_config) = tab_config else {
            let available: Vec<String> = WarpConfig::as_ref(ctx)
                .tab_configs()
                .iter()
                .map(|tc| tc.name.clone())
                .collect();
            return Response::Error {
                message: format!(
                    "no tab config named {name:?}. Available: {}",
                    if available.is_empty() {
                        "(none)".to_string()
                    } else {
                        available.join(", ")
                    }
                ),
            };
        };
        workspace.update(ctx, |ws, ctx| {
            ws.handle_action(&WorkspaceAction::SelectTabConfig(tab_config), ctx);
        });
        return Response::Ok;
    }

    workspace.update(ctx, |ws, ctx| {
        ws.handle_action(
            &WorkspaceAction::AddTerminalTab {
                hide_homepage: false,
            },
            ctx,
        );
    });
    Response::Ok
}

fn handle_close_tab(tab: u64, ctx: &mut AppContext) -> Response {
    let Some(workspace) = active_workspace(ctx) else {
        return Response::Error {
            message: "no active workspace".into(),
        };
    };
    let groups = workspace.as_ref(ctx).list_tab_pane_groups(ctx);
    let index = groups.iter().find_map(|tpg| {
        if entity_id_to_u64(tpg.pane_group_id) == tab || tpg.tab_idx as u64 == tab {
            Some(tpg.tab_idx)
        } else {
            None
        }
    });
    let Some(index) = index else {
        return Response::Error {
            message: format!("tab {tab} not found"),
        };
    };
    workspace.update(ctx, |ws, ctx| {
        ws.handle_action(&WorkspaceAction::CloseTab(index), ctx);
    });
    Response::Ok
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
