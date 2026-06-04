use clap::Parser;

use crate::{Args, CliCommand, Command};

use super::{ControlCommand, PaneCommand, SendInputArgs};

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
