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
    ListPanes { tab: Option<u64> },
    SendInput { pane: Option<u64>, text: String, newline: bool },
    ReadPane { pane: Option<u64>, blocks: usize },
    NewTab,
    CloseTab { tab: u64 },
    FocusTab { tab: u64 },
    FocusPane { pane: u64 },
    SplitPane { pane: Option<u64>, direction: SplitDir },
    ClosePane { pane: u64 },
    ListBlocks { pane: Option<u64>, limit: usize },
    ReadBlock { block: String },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitDir {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Response {
    Pong,
    Ok,
    Tabs { tabs: Vec<TabSummary> },
    Panes { panes: Vec<PaneSummary> },
    PaneOutput { pane: u64, blocks: Vec<BlockEntry> },
    Blocks { blocks: Vec<BlockEntry> },
    Block { block: BlockEntry },
    Error { message: String },
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
    pub exit_code: Option<i32>,
    pub pwd: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
}
