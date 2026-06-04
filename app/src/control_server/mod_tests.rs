use std::collections::HashMap;

use super::wire::{
    BlockEntry, PaneScreenSnapshot, PaneSnapshot, PaneSnapshotPane, TextMatchSource,
    WaitForTextBlockField, WaitForTextMode, WaitForTextSince,
};
use super::*;

fn block(id: &str, command: Option<&str>, output: &str) -> BlockEntry {
    BlockEntry {
        id: id.to_string(),
        pane_id: 7,
        command: command.map(ToOwned::to_owned),
        output: output.to_string(),
        output_truncated: false,
        exit_code: None,
        pwd: None,
        started_at: None,
        completed_at: None,
    }
}

fn snapshot(screen_text: Option<&str>, blocks: Vec<BlockEntry>) -> PaneSnapshot {
    PaneSnapshot {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        captured_at: "2026-06-03T00:00:00.000Z".to_string(),
        pane: PaneSnapshotPane {
            id: 7,
            tab_id: 70,
            tab_index: 0,
            title: None,
            cwd: Some("/tmp".to_string()),
            focused: true,
        },
        screen: screen_text.map(|text| PaneScreenSnapshot {
            alt_screen: false,
            text: text.to_string(),
            text_truncated: false,
        }),
        blocks,
    }
}

fn wait_options(text: &str) -> WaitForTextOptions {
    WaitForTextOptions {
        pane: None,
        text: text.to_string(),
        regex: false,
        timeout_ms: 30_000,
        mode: WaitForTextMode::Both,
        case_insensitive: false,
        since: WaitForTextSince::All,
        blocks: 10,
        block_field: WaitForTextBlockField::Output,
        max_output_bytes: 65_536,
        json: false,
    }
}

#[test]
fn case_insensitive_literal_match_uses_original_unicode_offsets() {
    let (matched, line) = find_in_text("İ prefix\nstatus: Ready\n", "ready", false, true).unwrap();

    assert_eq!(matched, "Ready");
    assert_eq!(line.as_deref(), Some("status: Ready"));
}

#[test]
fn since_now_ignores_text_present_in_the_baseline_prefix() {
    let snapshot = snapshot(
        Some("old prompt\nready now"),
        vec![block(
            "block-1",
            Some("echo old"),
            "old output\nready block",
        )],
    );
    let baseline = WaitForTextBaseline {
        screen_text: Some("old prompt\n".to_string()),
        block_text_by_id: HashMap::from([("block-1".to_string(), "old output".to_string())]),
    };

    let mut options = wait_options("old");
    options.since = WaitForTextSince::Now;
    assert!(find_text_match(&snapshot, &options, &baseline).is_none());

    options.text = "ready".to_string();
    let matched = find_text_match(&snapshot, &options, &baseline).unwrap();
    assert_eq!(matched.source, TextMatchSource::Screen);
    assert_eq!(matched.text, "ready");
    assert_eq!(matched.line.as_deref(), Some("ready now"));
}

#[test]
fn block_wait_ignores_command_text_by_default() {
    let snapshot = snapshot(
        None,
        vec![block(
            "block-1",
            Some("printf '__DONE_SENTINEL__:%s\\n' \"$status\""),
            "",
        )],
    );
    let baseline = WaitForTextBaseline::default();

    let mut options = wait_options("__DONE_SENTINEL__");
    options.mode = WaitForTextMode::Blocks;
    assert!(find_text_match(&snapshot, &options, &baseline).is_none());

    options.block_field = WaitForTextBlockField::Command;
    let matched = find_text_match(&snapshot, &options, &baseline).unwrap();
    assert_eq!(matched.source, TextMatchSource::Block);
    assert_eq!(matched.text, "__DONE_SENTINEL__");
}

#[test]
fn truncate_text_preserves_utf8_boundaries_at_the_tail() {
    let (text, truncated) = truncate_text("abcédef".to_string(), 4);

    assert_eq!(text, "def");
    assert_eq!(truncated, true);
}
