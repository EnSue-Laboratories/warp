//! `warp-oss control …` subcommands.
//!
//! These talk to a running Warp instance's control socket (see
//! `app/src/control_server/` for the server side and `app/src/cli_control/`
//! for the client-side handler).
//!
//! Noun set:
//! - **tab** — UI tab container (holds a PaneGroup). Operations: list, new,
//!   close, focus.
//! - **pane** — one shell process / PTY within a tab. Operations: list, send
//!   input, read scrollback, focus, split, close.
//! - **block** — one executed command and its output. Operations: list, read.

use clap::{Args, Subcommand, ValueEnum};

/// Interact with a running Warp instance.
#[derive(Debug, Clone, Subcommand)]
pub enum ControlCommand {
    /// Operate on UI tabs.
    #[command(subcommand)]
    Tab(TabCommand),

    /// Operate on panes (one shell process per pane).
    #[command(subcommand)]
    Pane(PaneCommand),

    /// Operate on blocks (executed commands and their output).
    #[command(subcommand)]
    Block(BlockCommand),
}

#[derive(Debug, Clone, Subcommand)]
pub enum TabCommand {
    /// List all open tabs.
    List,
    /// Open a new tab.
    New,
    /// Close a tab by id.
    Close(TabIdArg),
    /// Focus a tab by id.
    Focus(TabIdArg),
}

#[derive(Debug, Clone, Subcommand)]
pub enum PaneCommand {
    /// List panes, optionally filtered by tab.
    List(PaneListArgs),

    /// Send a command to a pane. A trailing newline is appended.
    Send(SendInputArgs),

    /// Read a pane's recent output (scrollback summary).
    Read(PaneReadArgs),

    /// Focus a pane by id.
    Focus(PaneIdArg),

    /// Split a pane to create a new sibling pane next to it.
    Split(SplitArgs),

    /// Close a pane by id.
    Close(PaneIdArg),
}

#[derive(Debug, Clone, Subcommand)]
pub enum BlockCommand {
    /// List blocks in a pane (most recent last).
    List(BlockListArgs),

    /// Read a block's command + output by id.
    Read(BlockIdArg),
}

#[derive(Debug, Clone, Args)]
pub struct TabIdArg {
    /// Tab id (as returned by `tab list`).
    pub id: String,
}

#[derive(Debug, Clone, Args)]
pub struct PaneIdArg {
    /// Pane id (as returned by `pane list`).
    pub id: String,
}

#[derive(Debug, Clone, Args)]
pub struct BlockIdArg {
    /// Block id (as returned by `block list`).
    pub id: String,
}

#[derive(Debug, Clone, Args, Default)]
pub struct PaneListArgs {
    /// Restrict to panes in this tab.
    #[arg(long)]
    pub tab: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct SendInputArgs {
    /// Pane id (as returned by `pane list`).
    pub pane: String,

    /// The command text to send. A trailing newline is appended by default.
    pub command: String,

    /// Suppress the trailing newline (send the bytes verbatim).
    #[arg(long)]
    pub no_newline: bool,
}

#[derive(Debug, Clone, Args)]
pub struct PaneReadArgs {
    /// Pane id. Defaults to the focused pane if omitted.
    #[arg(long)]
    pub pane: Option<String>,

    /// Number of most-recent blocks to include in the dump.
    #[arg(long, default_value_t = 10)]
    pub blocks: usize,
}

#[derive(Debug, Clone, Args)]
pub struct SplitArgs {
    /// Pane to split. Defaults to the focused pane.
    #[arg(long)]
    pub pane: Option<String>,

    /// Split direction.
    #[arg(long, value_enum, default_value_t = SplitDirection::Right)]
    pub direction: SplitDirection,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum SplitDirection {
    /// Open the new pane to the left of the source.
    Left,
    /// Open the new pane to the right of the source.
    Right,
    /// Open the new pane above the source.
    Up,
    /// Open the new pane below the source.
    Down,
}

#[derive(Debug, Clone, Args)]
pub struct BlockListArgs {
    /// Pane to list blocks for. Defaults to the focused pane.
    #[arg(long)]
    pub pane: Option<String>,

    /// Cap the number of blocks returned (most recent).
    #[arg(long, default_value_t = 50)]
    pub limit: usize,
}
