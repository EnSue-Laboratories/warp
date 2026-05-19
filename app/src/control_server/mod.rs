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

use std::path::PathBuf;

use futures::io::{BufReader, BufWriter};
use futures::AsyncReadExt as _;
use warpui::{AppContext, Entity, EntityId, SingletonEntity, ViewHandle};

use crate::terminal::view::TerminalView;
use warpui::TypedActionView;

use crate::pane_group::{PaneGroup, PaneGroupAction};
use crate::pane_group::tree::Direction as PaneDirection;
use crate::workspace::action::WorkspaceAction;
use crate::workspace::registry::WorkspaceRegistry;
use crate::workspace::view::Workspace;
use wire::{BlockEntry, PaneSummary, Request, Response, SplitDir, TabSummary};

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

    let response = spawner
        .spawn(move |_me, ctx| dispatch(request, ctx))
        .await
        .unwrap_or_else(|_| Response::Error {
            message: "control_server: dispatch dropped (model gone)".into(),
        });

    if let Err(e) = framing::write_frame(&mut writer, &response).await {
        log::warn!("control_server: write response failed: {e}");
    }
}

fn dispatch(request: Request, ctx: &mut AppContext) -> Response {
    match request {
        Request::Ping => Response::Pong,
        Request::ListTabs => handle_list_tabs(ctx),
        Request::ListPanes { tab } => handle_list_panes(tab, ctx),
        Request::SendInput {
            pane,
            text,
            newline,
        } => handle_send_input(pane, text, newline, ctx),
        Request::ReadPane { pane, blocks } => handle_read_pane(pane, blocks, ctx),
        Request::NewTab => handle_new_tab(ctx),
        Request::CloseTab { tab } => handle_close_tab(tab, ctx),
        Request::ListBlocks { pane, limit } => handle_list_blocks(pane, limit, ctx),
        Request::SplitPane { pane, direction } => handle_split_pane(pane, direction, ctx),
        Request::FocusTab { .. }
        | Request::FocusPane { .. }
        | Request::ClosePane { .. }
        | Request::ReadBlock { .. } => Response::Error {
            message: "not implemented in v0".into(),
        },
    }
}

// -------- helpers ----------------------------------------------------------

fn entity_id_to_u64(id: EntityId) -> u64 {
    // EntityId implements Display as its inner usize — round-trip via string.
    id.to_string().parse::<u64>().unwrap_or(0)
}

/// Return the Workspace for the currently active (frontmost) Warp window.
///
/// `WorkspaceRegistry::all_workspaces` is HashMap-backed and yields windows in
/// arbitrary order, so picking `.next()` would route control commands to a
/// random workspace whenever multiple Warp windows are open. Mirror the
/// `active_workspace` pattern from `crate::root_view` instead.
fn active_workspace(ctx: &AppContext) -> Option<ViewHandle<Workspace>> {
    let window_id = ctx.windows().active_window()?;
    WorkspaceRegistry::as_ref(ctx).get(window_id, ctx)
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
    let groups = ws.list_tab_pane_groups(ctx);
    let tabs = groups
        .into_iter()
        .map(|tpg| TabSummary {
            id: entity_id_to_u64(tpg.pane_group_id),
            index: tpg.tab_idx,
            title: None,
            active: tpg.tab_idx == active_idx,
            pane_ids: tpg
                .terminal_ids
                .iter()
                .map(|eid| entity_id_to_u64(*eid))
                .collect(),
        })
        .collect();
    Response::Tabs { tabs }
}

fn handle_list_panes(filter_tab: Option<u64>, ctx: &mut AppContext) -> Response {
    let Some(workspace) = active_workspace(ctx) else {
        return Response::Panes { panes: vec![] };
    };
    let ws = workspace.as_ref(ctx);
    let active_idx = ws.active_tab_index();
    let groups = ws.list_tab_pane_groups(ctx);
    let mut panes = Vec::new();
    for tpg in groups {
        let tab_id = entity_id_to_u64(tpg.pane_group_id);
        if let Some(want) = filter_tab {
            if want != tab_id && want != tpg.tab_idx as u64 {
                continue;
            }
        }
        // The focused pane id is meaningful per pane group; consider a pane
        // "focused" only when its tab is also active.
        let focused_pid = ws
            .tabs
            .get(tpg.tab_idx)
            .map(|tab| tab.pane_group.as_ref(ctx).focused_pane_id(ctx));
        for term_id in &tpg.terminal_ids {
            let view = lookup_terminal_view(entity_id_to_u64(*term_id), ctx);
            let cwd = view.as_ref().and_then(|v| v.as_ref(ctx).pwd());
            // Match the pane id we'd return to whatever PaneGroup considers focused.
            let is_focused = if tpg.tab_idx == active_idx {
                match (focused_pid, &view) {
                    (Some(pid), Some(v)) => {
                        // The focused PaneId corresponds to a TerminalView with this EntityId.
                        let v_id = entity_id_to_u64(v.id());
                        let pg = ws.tabs[tpg.tab_idx].pane_group.as_ref(ctx);
                        pg.terminal_view_from_pane_id(pid, ctx)
                            .map_or(false, |fv| entity_id_to_u64(fv.id()) == v_id)
                    }
                    _ => false,
                }
            } else {
                false
            };
            panes.push(PaneSummary {
                id: entity_id_to_u64(*term_id),
                tab_id,
                tab_index: tpg.tab_idx,
                title: None,
                cwd,
                focused: is_focused,
            });
        }
    }
    Response::Panes { panes }
}

fn handle_send_input(
    pane: Option<u64>,
    text: String,
    _newline: bool,
    ctx: &mut AppContext,
) -> Response {
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
    view_handle.update(ctx, |view, ctx| {
        view.execute_command_or_set_pending(&text, ctx);
    });
    Response::Ok
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

fn block_to_entry(b: &crate::terminal::model::block::Block, pane_wire: u64) -> BlockEntry {
    let command = b
        .prompt_and_command_grid()
        .contents_to_string(false, None);
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
        exit_code: Some(b.exit_code().value()),
        pwd: b.pwd().cloned(),
        started_at: None,
        completed_at: None,
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

fn handle_split_pane(
    pane: Option<u64>,
    direction: SplitDir,
    ctx: &mut AppContext,
) -> Response {
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
    pane_group.update(ctx, |pg, ctx| {
        pg.handle_action(&PaneGroupAction::Add(dir), ctx);
    });
    Response::Ok
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

fn handle_new_tab(ctx: &mut AppContext) -> Response {
    let Some(workspace) = active_workspace(ctx) else {
        return Response::Error {
            message: "no active workspace".into(),
        };
    };
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
