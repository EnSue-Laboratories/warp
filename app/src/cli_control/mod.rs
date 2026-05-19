//! Client-side handler for `warp-oss control …` subcommands.
//!
//! These connect to the control socket exposed by a running Warp GUI instance
//! (see `crate::control_server`) and proxy the request. When no Warp instance
//! is running, the handler errors out without spawning the GUI.
//!
//! Status: scaffolding. The transport, socket discovery, and RPCs are not yet
//! implemented — each handler currently returns a clear `not yet implemented`
//! error so the CLI surface exists for shape/discoverability while the
//! server side is being built out.

use anyhow::{bail, Result};
use warp_cli::control::{
    BlockCommand, ControlCommand, PaneCommand, ReadBlockArgs, SendInputArgs, SessionCommand,
};
use warp_cli::GlobalOptions;
use warpui::AppContext;

/// Dispatch `warp control …`.
pub fn run(
    _ctx: &mut AppContext,
    _global_options: GlobalOptions,
    command: ControlCommand,
) -> Result<()> {
    match command {
        ControlCommand::Session(SessionCommand::List) => session_list(),
        ControlCommand::Pane(PaneCommand::List) => pane_list(),
        ControlCommand::Pane(PaneCommand::Send(args)) => pane_send(args),
        ControlCommand::Block(BlockCommand::Read(args)) => block_read(args),
    }
}

fn session_list() -> Result<()> {
    bail!("`warp control session list` is not yet implemented; control socket pending");
}

fn pane_list() -> Result<()> {
    bail!("`warp control pane list` is not yet implemented; control socket pending");
}

fn pane_send(_args: SendInputArgs) -> Result<()> {
    bail!("`warp control pane send` is not yet implemented; control socket pending");
}

fn block_read(_args: ReadBlockArgs) -> Result<()> {
    bail!("`warp control block read` is not yet implemented; control socket pending");
}
