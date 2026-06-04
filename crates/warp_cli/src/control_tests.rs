use clap::Parser;

use crate::{Args, CliCommand, Command};

use super::{
    ControlCommand, PaneCommand, PaneSnapshotArgs, SendInputArgs, WaitForTextArgs, WaitForTextMode,
    WaitForTextSince,
};

fn parse_pane_send<const N: usize>(args: [&str; N]) -> SendInputArgs {
    let args = Args::try_parse_from(args).expect("control pane send should parse");
    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("expected command-line command");
    };
    let CliCommand::Control(ControlCommand::Pane(PaneCommand::Send(send_args))) =
        boxed_cmd.as_ref()
    else {
        panic!("expected control pane send command");
    };
    send_args.clone()
}

fn parse_pane_snapshot<const N: usize>(args: [&str; N]) -> PaneSnapshotArgs {
    let args = Args::try_parse_from(args).expect("control pane snapshot should parse");
    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("expected command-line command");
    };
    let CliCommand::Control(ControlCommand::Pane(PaneCommand::Snapshot(snapshot_args))) =
        boxed_cmd.as_ref()
    else {
        panic!("expected control pane snapshot command");
    };
    snapshot_args.clone()
}

fn parse_pane_wait_for_text<const N: usize>(args: [&str; N]) -> WaitForTextArgs {
    let args = Args::try_parse_from(args).expect("control pane wait-for-text should parse");
    let Some(Command::CommandLine(boxed_cmd)) = args.command else {
        panic!("expected command-line command");
    };
    let CliCommand::Control(ControlCommand::Pane(PaneCommand::WaitForText(wait_args))) =
        boxed_cmd.as_ref()
    else {
        panic!("expected control pane wait-for-text command");
    };
    wait_args.clone()
}

#[test]
fn pane_send_defaults_to_no_wait() {
    let args = parse_pane_send(["warp", "control", "pane", "send", "echo", "hi"]);

    assert_eq!(args.pane, None);
    assert_eq!(args.wait, false);
    assert_eq!(args.timeout, None);
    assert_eq!(args.command, vec!["echo", "hi"]);
}

#[test]
fn pane_send_accepts_wait_and_timeout() {
    let args = parse_pane_send([
        "warp",
        "control",
        "pane",
        "send",
        "--pane",
        "123",
        "--wait",
        "--timeout",
        "5",
        "echo",
        "hi",
    ]);

    assert_eq!(args.pane.as_deref(), Some("123"));
    assert_eq!(args.wait, true);
    assert_eq!(args.timeout, Some(5));
    assert_eq!(args.command, vec!["echo", "hi"]);
}

#[test]
fn pane_send_accepts_short_wait_flag() {
    let args = parse_pane_send(["warp", "control", "pane", "send", "-w", "pwd"]);

    assert_eq!(args.wait, true);
    assert_eq!(args.command, vec!["pwd"]);
}

#[test]
fn pane_send_rejects_timeout_without_wait() {
    let err = Args::try_parse_from([
        "warp",
        "control",
        "pane",
        "send",
        "--timeout",
        "5",
        "echo",
        "hi",
    ])
    .expect_err("--timeout should require --wait");

    assert!(
        err.to_string().contains("--wait"),
        "expected error to mention --wait, got: {err}"
    );
}

#[test]
fn pane_snapshot_defaults_to_screen_json_off_and_five_blocks() {
    let args = parse_pane_snapshot(["warp", "control", "pane", "snapshot"]);

    assert_eq!(args.pane, None);
    assert_eq!(args.blocks, 5);
    assert_eq!(args.no_screen, false);
    assert_eq!(args.max_output_bytes, 65_536);
    assert_eq!(args.json, false);
}

#[test]
fn pane_snapshot_accepts_json_and_tuning_flags() {
    let args = parse_pane_snapshot([
        "warp",
        "control",
        "pane",
        "snap",
        "--pane",
        "123",
        "--blocks",
        "2",
        "--no-screen",
        "--max-output-bytes",
        "99",
        "--json",
    ]);

    assert_eq!(args.pane.as_deref(), Some("123"));
    assert_eq!(args.blocks, 2);
    assert_eq!(args.no_screen, true);
    assert_eq!(args.max_output_bytes, 99);
    assert_eq!(args.json, true);
}

#[test]
fn pane_wait_for_text_defaults_to_both_existing_text() {
    let args = parse_pane_wait_for_text(["warp", "control", "pane", "wait-for-text", "ready"]);

    assert_eq!(args.pane, None);
    assert_eq!(args.regex, false);
    assert_eq!(args.timeout, 30);
    assert!(matches!(args.mode, WaitForTextMode::Both));
    assert_eq!(args.case_insensitive, false);
    assert!(matches!(args.since, WaitForTextSince::All));
    assert_eq!(args.blocks, 10);
    assert_eq!(args.max_output_bytes, 65_536);
    assert_eq!(args.json, false);
    assert_eq!(args.text, "ready");
}

#[test]
fn pane_wait_for_text_accepts_regex_alias_and_tuning_flags() {
    let args = parse_pane_wait_for_text([
        "warp",
        "control",
        "pane",
        "wait",
        "--pane",
        "123",
        "--regex",
        "--timeout",
        "7",
        "--mode",
        "screen",
        "--case-insensitive",
        "--since",
        "now",
        "--blocks",
        "3",
        "--max-output-bytes",
        "99",
        "--json",
        "READY|DONE",
    ]);

    assert_eq!(args.pane.as_deref(), Some("123"));
    assert_eq!(args.regex, true);
    assert_eq!(args.timeout, 7);
    assert!(matches!(args.mode, WaitForTextMode::Screen));
    assert_eq!(args.case_insensitive, true);
    assert!(matches!(args.since, WaitForTextSince::Now));
    assert_eq!(args.blocks, 3);
    assert_eq!(args.max_output_bytes, 99);
    assert_eq!(args.json, true);
    assert_eq!(args.text, "READY|DONE");
}
