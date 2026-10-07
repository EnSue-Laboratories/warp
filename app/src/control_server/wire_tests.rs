use super::{PaneStatus, Request, Response};

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
    assert!(!wait);
    assert_eq!(timeout_ms, None);
}

#[test]
fn list_panes_defaults_to_no_preview_or_json_for_old_clients() {
    let request: Request = serde_json::from_value(serde_json::json!({
        "kind": "list_panes",
        "tab": 42
    }))
    .expect("old list_panes frame should deserialize");

    let Request::ListPanes {
        tab,
        include_preview,
        json,
    } = request
    else {
        panic!("expected list_panes request");
    };

    assert_eq!(tab, Some(42));
    assert!(!include_preview);
    assert!(!json);
}

#[test]
fn pane_summary_activity_fields_default_for_old_responses() {
    let response: Response = serde_json::from_value(serde_json::json!({
        "kind": "panes",
        "panes": [{
            "id": 7,
            "tab_id": 70,
            "tab_index": 0,
            "title": null,
            "cwd": "/tmp",
            "focused": true
        }]
    }))
    .expect("old panes response should deserialize");

    let Response::Panes {
        panes,
        include_preview,
        json,
    } = response
    else {
        panic!("expected panes response");
    };

    assert!(!include_preview);
    assert!(!json);
    assert_eq!(panes.len(), 1);
    assert_eq!(panes[0].status, PaneStatus::Idle);
    assert!(!panes[0].running);
    assert_eq!(panes[0].foreground_process, None);
    assert_eq!(panes[0].preview, None);
}
