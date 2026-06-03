use super::*;

#[test]
fn test_git_branch_on_click_value_round_trips_through_encode_decode() {
    let value = GitBranchOnClickValue::new("feature-a".to_string());
    assert_eq!(GitBranchOnClickValue::decode(&value.encode()), value);
}

#[test]
fn test_git_branch_on_click_value_decode_strips_legacy_worktree_metadata() {
    // Older builds packed worktree metadata after the encoded-value
    // separator. New code stores worktrees in a separate chip; if we
    // see a legacy payload we should still produce a valid branch name.
    let value =
        format!("feature-a{ENCODED_VALUE_SEPARATOR}worktree{ENCODED_VALUE_SEPARATOR}/repo/feature-a");
    assert_eq!(
        GitBranchOnClickValue::decode(&value),
        GitBranchOnClickValue::new("feature-a".to_string())
    );
}

#[test]
fn test_filter_git_branch_on_click_values_drops_linked_worktrees() {
    // `+` marks a branch that's checked out in another worktree.
    // It should not appear in the branch chip; the worktree chip
    // surfaces it instead.
    let values = Some(vec![
        "  feature-a".to_string(),
        "+ linked-worktree".to_string(),
        "* main".to_string(),
        "  +literal-plus".to_string(),
    ]);
    let values = filter_git_branch_on_click_values(values).unwrap();
    let values: Vec<_> = values
        .iter()
        .map(|value| GitBranchOnClickValue::decode(value))
        .collect();
    assert_eq!(
        values,
        vec![
            GitBranchOnClickValue::new("main".to_string()),
            GitBranchOnClickValue::new("feature-a".to_string()),
            // `+literal-plus` has no whitespace after `+`, so it's not
            // parsed as a worktree marker and stays as a branch.
            GitBranchOnClickValue::new("+literal-plus".to_string()),
        ]
    );
}

#[test]
fn test_filter_git_worktree_on_click_values_parses_porcelain() {
    let values = Some(vec![
        "worktree /repo".to_string(),
        "HEAD abcd1234".to_string(),
        "branch refs/heads/main".to_string(),
        "".to_string(),
        "worktree /repo/.worktrees/feature".to_string(),
        "HEAD 5678efff".to_string(),
        "branch refs/heads/feature".to_string(),
        "".to_string(),
        "worktree /repo/.worktrees/detached".to_string(),
        "HEAD 99999999".to_string(),
        "detached".to_string(),
    ]);
    let values = filter_git_worktree_on_click_values(values).unwrap();
    let values: Vec<_> = values
        .iter()
        .map(|value| GitWorktreeOnClickValue::decode(value))
        .collect();
    assert_eq!(
        values,
        vec![
            GitWorktreeOnClickValue::new("/repo".to_string(), Some("main".to_string())),
            GitWorktreeOnClickValue::new(
                "/repo/.worktrees/feature".to_string(),
                Some("feature".to_string())
            ),
            // Detached HEAD worktree: no branch ref.
            GitWorktreeOnClickValue::new("/repo/.worktrees/detached".to_string(), None),
        ]
    );
}

#[test]
fn test_git_worktree_on_click_value_round_trips_with_and_without_branch() {
    for value in [
        GitWorktreeOnClickValue::new("/repo".to_string(), Some("main".to_string())),
        GitWorktreeOnClickValue::new("/tmp/detached".to_string(), None),
    ] {
        assert_eq!(GitWorktreeOnClickValue::decode(&value.encode()), value);
    }
}

#[test]
fn test_git_worktree_display_name_uses_basename() {
    let with_trailing = GitWorktreeOnClickValue::new("/repo/feature/".to_string(), None);
    assert_eq!(with_trailing.display_name(), "feature");
    let root = GitWorktreeOnClickValue::new("/".to_string(), None);
    assert_eq!(root.display_name(), "/");
}

#[test]
fn test_is_plausible_new_branch_name_accepts_typical_names() {
    for name in [
        "feature/xyz",
        "fix-123",
        "release/v1.2.3",
        "user/alice/work",
        "main",
    ] {
        assert!(
            is_plausible_new_branch_name(name),
            "expected {name:?} to be accepted",
        );
    }
}

#[test]
fn test_is_plausible_new_branch_name_rejects_empty_or_whitespace() {
    for name in ["", "   ", "\t\n"] {
        assert!(
            !is_plausible_new_branch_name(name),
            "expected {name:?} to be rejected",
        );
    }
}

#[test]
fn test_is_plausible_new_branch_name_rejects_leading_dash() {
    assert!(!is_plausible_new_branch_name("-foo"));
    assert!(!is_plausible_new_branch_name("--all"));
}

#[test]
fn test_is_plausible_new_branch_name_rejects_internal_whitespace() {
    assert!(!is_plausible_new_branch_name("my branch"));
    assert!(!is_plausible_new_branch_name("foo\tbar"));
}
