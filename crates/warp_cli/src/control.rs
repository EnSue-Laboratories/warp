//! `warp-oss control …` subcommands.
//!
//! These talk to a running Warp instance's control socket (see
//! `app/src/control_server/` for the server side and `app/src/cli_control/`
//! for the client-side handler). When no Warp instance is running, the
//! handler errors out cleanly instead of launching the GUI.

use clap::{Args, Subcommand};

/// Interact with a running Warp instance.
#[derive(Debug, Clone, Subcommand)]
pub enum ControlCommand {
    /// Operate on sessions.
    #[command(subcommand)]
    Session(SessionCommand),

    /// Operate on panes (terminal views) within sessions.
    #[command(subcommand)]
    Pane(PaneCommand),

    /// Operate on blocks (executed commands and their output).
    #[command(subcommand)]
    Block(BlockCommand),
}

#[derive(Debug, Clone, Subcommand)]
pub enum SessionCommand {
    /// List all open sessions in the running Warp instance.
    List,
}

#[derive(Debug, Clone, Subcommand)]
pub enum PaneCommand {
    /// List all open panes across all sessions.
    List,

    /// Send a command to a pane. A trailing newline is appended.
    Send(SendInputArgs),
}

#[derive(Debug, Clone, Args)]
pub struct SendInputArgs {
    /// Pane UUID (or unambiguous prefix). Defaults to the focused pane.
    #[arg(long)]
    pub pane: Option<String>,

    /// The command text to send.
    pub command: String,
}

#[derive(Debug, Clone, Subcommand)]
pub enum BlockCommand {
    /// Print a block's command and output.
    Read(ReadBlockArgs),
}

#[derive(Debug, Clone, Args)]
pub struct ReadBlockArgs {
    /// Block id.
    pub id: String,
}
