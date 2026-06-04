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
//!   input, read scrollback, focus, split, close, share.
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
    /// Open a new tab. With `--config`, opens a saved tab config by name
    /// (e.g. an SSH tab); otherwise opens a plain terminal tab.
    New(TabNewArgs),
    /// Close a tab by id.
    Close(TabIdArg),
    /// Focus a tab by id.
    Focus(TabIdArg),
}

#[derive(Debug, Clone, Args)]
pub struct TabNewArgs {
    /// Open a saved tab config by name (matched against the config's `name`
    /// field, from your `tab_configs/` directory), e.g. `--config "SSH:
    /// claude-code"`. When omitted, opens a plain terminal tab.
    #[arg(long)]
    pub config: Option<String>,
}

#[derive(Debug, Clone, Subcommand)]
pub enum PaneCommand {
    /// List panes, optionally filtered by tab.
    List(PaneListArgs),

    /// Send a command to a pane (block-submission path). A trailing
    /// newline is implicit. For TUI applications (vim, fzf, etc.), use
    /// `write` or `keystroke` instead.
    Send(SendInputArgs),

    /// Write raw bytes directly to a pane's PTY (no newline, no execute).
    /// This is the input path for TUI applications.
    Write(WriteBytesArgs),

    /// Send a named keystroke or chord to a pane's PTY.
    ///
    /// Supported names: enter, return, esc, escape, tab, backspace, space,
    /// up, down, left, right, home, end, pageup, pagedown, delete, ins,
    /// f1..f12, and ctrl-<char> chords (e.g. `ctrl-c`, `ctrl-d`, `ctrl-z`).
    Keystroke(KeystrokeArgs),

    /// Read a pane's recent output (scrollback summary).
    Read(PaneReadArgs),

    /// Capture a pane's current screen as text. Unlike `read` (which dumps
    /// command blocks), this renders the live screen grid — so it can see
    /// inside full-screen/TUI apps like vim, tmux, and less.
    Screen(PaneScreenArgs),

    /// Capture a structured pane snapshot with screen text and recent blocks.
    #[command(alias = "snap")]
    Snapshot(PaneSnapshotArgs),

    /// Wait until text appears in a pane's screen or recent command blocks.
    #[command(alias = "wait")]
    WaitForText(WaitForTextArgs),

    /// Start sharing a pane's session and return immediately while setup is pending.
    Share(PaneShareArgs),

    /// Print the watch link for a shared pane, once sharing has finished setup.
    ShareLink(PaneTargetArgs),

    /// Stop sharing a pane's session.
    Unshare(PaneTargetArgs),

    /// Focus a pane by id.
    Focus(PaneIdArg),

    /// Split a pane to create a new sibling pane next to it.
    Split(SplitArgs),

    /// Close a pane by id.
    Close(PaneIdArg),
}

#[derive(Debug, Clone, Args)]
pub struct WriteBytesArgs {
    /// Pane id (defaults to the focused pane).
    #[arg(long)]
    pub pane: Option<String>,

    /// Text to write to the PTY. Bytes are written verbatim (UTF-8).
    pub text: String,
}

#[derive(Debug, Clone, Args)]
pub struct KeystrokeArgs {
    /// Pane id (defaults to the focused pane).
    #[arg(long)]
    pub pane: Option<String>,

    /// Key name (e.g. `enter`, `esc`, `up`) or a `ctrl-<char>` chord.
    pub key: String,
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
    /// Pane id (defaults to the focused pane). Matches the convention used
    /// by `pane write`, `pane keystroke`, `pane read`, and `pane split`.
    #[arg(long)]
    pub pane: Option<String>,

    /// Wait for the submitted command block to finish and print its output.
    #[arg(long, short = 'w')]
    pub wait: bool,

    /// Maximum seconds to wait for completion. Defaults to 120 seconds when
    /// `--wait` is set. The command keeps running if this timeout is reached.
    #[arg(long, value_name = "SECS", requires = "wait")]
    pub timeout: Option<u64>,

    /// The command text to send. Multiple args are joined with single spaces,
    /// so `pane send --pane <id> ls -la /tmp` works without shell quoting. The
    /// command is executed as a whole block (Warp's command-block model), so
    /// a trailing newline is implicit.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
    pub command: Vec<String>,
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
pub struct PaneScreenArgs {
    /// Pane id. Defaults to the focused pane if omitted.
    #[arg(long)]
    pub pane: Option<String>,
}

#[derive(Debug, Clone, Args)]
pub struct PaneSnapshotArgs {
    /// Pane id. Defaults to the focused pane if omitted.
    #[arg(long)]
    pub pane: Option<String>,

    /// Number of most-recent blocks to include.
    #[arg(long, default_value_t = 5)]
    pub blocks: usize,

    /// Omit the live screen grid from the snapshot.
    #[arg(long)]
    pub no_screen: bool,

    /// Maximum bytes of text to include per block output/screen field.
    #[arg(long, default_value_t = 65_536)]
    pub max_output_bytes: usize,

    /// Print the snapshot as structured JSON.
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Clone, Args)]
pub struct WaitForTextArgs {
    /// Pane id. Defaults to the focused pane if omitted.
    #[arg(long)]
    pub pane: Option<String>,

    /// Treat the text argument as a regular expression.
    #[arg(long)]
    pub regex: bool,

    /// Maximum seconds to wait before returning a timeout error.
    #[arg(long, default_value_t = 30)]
    pub timeout: u64,

    /// Where to search for the text.
    #[arg(long, value_enum, default_value_t = WaitForTextMode::Both)]
    pub mode: WaitForTextMode,

    /// Ignore ASCII/Unicode case while matching.
    #[arg(long)]
    pub case_insensitive: bool,

    /// Whether to match existing text or only text that appears after the wait starts.
    #[arg(long, value_enum, default_value_t = WaitForTextSince::All)]
    pub since: WaitForTextSince,

    /// Number of most-recent blocks to search when block matching is enabled.
    #[arg(long, default_value_t = 10)]
    pub blocks: usize,

    /// Which part of recent blocks to search.
    #[arg(long, value_enum, default_value_t = WaitForTextBlockField::Output)]
    pub block_field: WaitForTextBlockField,

    /// Maximum bytes of text to include per field in JSON match/timeout snapshots.
    #[arg(long, default_value_t = 65_536)]
    pub max_output_bytes: usize,

    /// Print match/timeout details as structured JSON.
    #[arg(long)]
    pub json: bool,

    /// Literal text or regular expression to wait for.
    pub text: String,
}

#[derive(Debug, Clone, Args)]
pub struct PaneShareArgs {
    /// Pane id. Defaults to the focused pane if omitted.
    #[arg(long)]
    pub pane: Option<String>,

    /// How much scrollback to include in the shared session.
    #[arg(long, value_enum, default_value_t = ShareScrollback::None)]
    pub scrollback: ShareScrollback,
}

#[derive(Debug, Clone, Args)]
pub struct PaneTargetArgs {
    /// Pane id. Defaults to the focused pane if omitted.
    #[arg(long)]
    pub pane: Option<String>,
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

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ShareScrollback {
    /// Do not include prior scrollback.
    None,
    /// Include all shareable prior scrollback.
    All,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum WaitForTextMode {
    /// Search the live screen grid only.
    Screen,
    /// Search recent command blocks only.
    Blocks,
    /// Search both live screen and recent command blocks.
    Both,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum WaitForTextSince {
    /// Match existing text and future text.
    All,
    /// Only match text that appears after the wait starts.
    Now,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum WaitForTextBlockField {
    /// Search block output only.
    Output,
    /// Search submitted command text only.
    Command,
    /// Search command text followed by output.
    Both,
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

#[cfg(test)]
#[path = "control_tests.rs"]
mod tests;
