//! Wire protocol shared between the in-app control server
//! (`crate::control_server`) and the CLI client (`crate::cli_control`).
//!
//! Single request → single response, length-prefixed JSON.

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Request {
    Ping,
    ListTabs,
    ListPanes {
        tab: Option<u64>,
    },
    SendInput {
        pane: Option<u64>,
        text: String,
        #[serde(default)]
        wait: bool,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    ReadPane {
        pane: Option<u64>,
        blocks: usize,
    },
    /// Render a pane's current screen to text. Captures the alternate-screen
    /// grid for TUI apps (vim/tmux/less/…) that `ReadPane`'s block model can't
    /// see.
    ReadScreen {
        pane: Option<u64>,
    },
    /// Capture a structured pane snapshot for agent inspection.
    SnapshotPane {
        pane: Option<u64>,
        blocks: usize,
        include_screen: bool,
        max_output_bytes: usize,
        json: bool,
    },
    /// Wait for text or a regex to appear in a pane's screen/blocks.
    WaitForText {
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
    },
    /// Start sharing a pane's session. Session setup completes asynchronously.
    SharePane {
        pane: Option<u64>,
        scrollback: ShareScrollback,
    },
    /// Return the share link for a pane once the session id is available.
    SharePaneLink {
        pane: Option<u64>,
    },
    /// Stop sharing a pane's session.
    UnsharePane {
        pane: Option<u64>,
    },
    /// Open a new tab. With `config`, opens the saved tab config whose `name`
    /// matches (e.g. an SSH tab); otherwise opens a plain terminal tab.
    NewTab {
        config: Option<String>,
    },
    CloseTab {
        tab: u64,
    },
    FocusTab {
        tab: u64,
    },
    FocusPane {
        pane: u64,
    },
    SplitPane {
        pane: Option<u64>,
        direction: SplitDir,
    },
    ClosePane {
        pane: u64,
    },
    ListBlocks {
        pane: Option<u64>,
        limit: usize,
    },
    ReadBlock {
        block: String,
    },
    /// Write raw bytes verbatim to a pane's PTY. No newline appended, no
    /// command-block submission semantics. Useful for driving TUI apps.
    WriteBytes {
        pane: Option<u64>,
        bytes: Vec<u8>,
    },
    /// Send a named key (or a chord like "ctrl-c") to a pane's PTY.
    Keystroke {
        pane: Option<u64>,
        key: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitDir {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShareScrollback {
    None,
    All,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Response {
    Pong,
    Ok,
    Tabs {
        tabs: Vec<TabSummary>,
    },
    Panes {
        panes: Vec<PaneSummary>,
    },
    PaneOutput {
        pane: u64,
        blocks: Vec<BlockEntry>,
    },
    /// Rendered screen text for a pane. `alt_screen` is true when the text came
    /// from a TUI/full-screen app's alternate screen.
    Screen {
        pane: u64,
        alt_screen: bool,
        text: String,
    },
    PaneSnapshot {
        snapshot: PaneSnapshot,
        json: bool,
    },
    WaitForTextMatched {
        pane: u64,
        elapsed_ms: u64,
        matched: TextMatch,
        snapshot: Option<PaneSnapshot>,
        json: bool,
    },
    WaitForTextTimedOut {
        pane: u64,
        timeout_ms: u64,
        elapsed_ms: u64,
        snapshot: Option<PaneSnapshot>,
        json: bool,
    },
    ShareStarted {
        pane: u64,
    },
    ShareLink {
        pane: u64,
        url: String,
    },
    SharePending {
        pane: u64,
    },
    ShareStopped {
        pane: u64,
    },
    Blocks {
        blocks: Vec<BlockEntry>,
    },
    Block {
        block: BlockEntry,
    },
    SendTimedOut {
        pane: u64,
        timeout_ms: u64,
        block: BlockEntry,
    },
    Error {
        message: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TabSummary {
    pub id: u64,
    pub index: usize,
    pub title: Option<String>,
    pub active: bool,
    pub pane_ids: Vec<u64>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PaneSummary {
    pub id: u64,
    pub tab_id: u64,
    pub tab_index: usize,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub focused: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BlockEntry {
    pub id: String,
    pub pane_id: u64,
    pub command: Option<String>,
    pub output: String,
    #[serde(default)]
    pub output_truncated: bool,
    pub exit_code: Option<i32>,
    pub pwd: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitForTextMode {
    Screen,
    Blocks,
    Both,
}

impl WaitForTextMode {
    pub fn includes_screen(self) -> bool {
        matches!(self, Self::Screen | Self::Both)
    }

    pub fn includes_blocks(self) -> bool {
        matches!(self, Self::Blocks | Self::Both)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WaitForTextSince {
    All,
    Now,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WaitForTextBlockField {
    Output,
    Command,
    Both,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PaneSnapshot {
    pub schema_version: u32,
    pub captured_at: String,
    pub pane: PaneSnapshotPane,
    pub screen: Option<PaneScreenSnapshot>,
    pub blocks: Vec<BlockEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PaneSnapshotPane {
    pub id: u64,
    pub tab_id: u64,
    pub tab_index: usize,
    pub title: Option<String>,
    pub cwd: Option<String>,
    pub focused: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PaneScreenSnapshot {
    pub alt_screen: bool,
    pub text: String,
    #[serde(default)]
    pub text_truncated: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TextMatchSource {
    Screen,
    Block,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TextMatch {
    pub source: TextMatchSource,
    pub pane_id: u64,
    pub block_id: Option<String>,
    pub text: String,
    pub line: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::Request;

    #[test]
    fn send_input_defaults_to_no_wait_for_old_clients() {
        let request: Request = serde_json::from_value(serde_json::json!({
            "kind": "send_input",
            "pane": 42,
            "text": "echo hi"
        }))
        .expect("old send_input frame should deserialize");

        let Request::SendInput {
            pane,
            text,
            wait,
            timeout_ms,
        } = request
        else {
            panic!("expected send_input request");
        };

        assert_eq!(pane, Some(42));
        assert_eq!(text, "echo hi");
        assert_eq!(wait, false);
        assert_eq!(timeout_ms, None);
    }
}
