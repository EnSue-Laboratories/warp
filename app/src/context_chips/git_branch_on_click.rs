//! Click-action payload encoding + filtering for the branch / worktree chips.
//!
//! Each chip has its own pill in the prompt toolbar:
//!
//! - **Branch chip** (`ContextChipKind::ShellGitBranch`) lists checkout-able
//!   branches. Linked-worktree entries (those with a `+` marker in `git
//!   branch`) are dropped because you cannot `git checkout` them while
//!   they're checked out elsewhere — they're surfaced in the worktree chip
//!   instead.
//! - **Worktree chip** (`ContextChipKind::ShellGitWorktree`) lists every
//!   worktree (including the main one), parsed from `git worktree list
//!   --porcelain`. Clicking one runs `cd <path>` in the active pane.
//!
//! The two chips run their own shell commands (see `builtins.rs`), so each
//! filter here parses the output of exactly one git invocation.

const ENCODED_VALUE_SEPARATOR: char = '\u{1f}';
const WORKTREE_BRANCH_TAG: &str = "branch";
const GIT_BRANCH_REF_PREFIX: &str = "refs/heads/";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GitBranchOnClickValue {
    pub(crate) branch_name: String,
}

impl GitBranchOnClickValue {
    pub(crate) fn new(branch_name: String) -> Self {
        Self { branch_name }
    }

    pub(crate) fn encode(&self) -> String {
        self.branch_name.clone()
    }

    pub(crate) fn decode(value: &str) -> Self {
        // For forward-compat with payloads emitted by older builds that
        // bundled worktree metadata into the branch chip, drop anything
        // after the encoded-value separator.
        let branch_name = value
            .split(ENCODED_VALUE_SEPARATOR)
            .next()
            .unwrap_or_default()
            .to_string();
        Self::new(branch_name)
    }
}

/// Click-action payload for the worktree chip. The path is what `cd`
/// targets; the branch (if any) and short label are used to render the
/// menu item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GitWorktreeOnClickValue {
    pub(crate) path: String,
    pub(crate) branch_name: Option<String>,
}

impl GitWorktreeOnClickValue {
    pub(crate) fn new(path: String, branch_name: Option<String>) -> Self {
        Self { path, branch_name }
    }

    /// Last segment of the worktree path (or the full path if it has none).
    /// Mirrors what users see in their shell prompt's `pwd`.
    pub(crate) fn display_name(&self) -> &str {
        self.path
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or(&self.path)
    }

    pub(crate) fn encode(&self) -> String {
        match &self.branch_name {
            Some(branch) => format!(
                "{}{ENCODED_VALUE_SEPARATOR}{WORKTREE_BRANCH_TAG}{ENCODED_VALUE_SEPARATOR}{branch}",
                self.path
            ),
            None => self.path.clone(),
        }
    }

    pub(crate) fn decode(value: &str) -> Self {
        let mut parts = value.splitn(3, ENCODED_VALUE_SEPARATOR);
        let path = parts.next().unwrap_or_default().to_string();
        let branch_name = match parts.next() {
            Some(WORKTREE_BRANCH_TAG) => parts.next().filter(|s| !s.is_empty()).map(str::to_string),
            _ => None,
        };
        Self::new(path, branch_name)
    }
}

struct ParsedGitBranchLine {
    branch_name: String,
    is_current: bool,
    is_linked_worktree: bool,
}

/// Filter the output of `git --no-optional-locks branch --no-color
/// --sort=-committerdate` into the encoded click values shown in the
/// branch chip's dropdown. Linked-worktree entries (marked with `+`) are
/// dropped — they cannot be `git checkout`'d, and the worktree chip
/// surfaces them by path.
pub(crate) fn filter_git_branch_on_click_values(
    values_opt: Option<Vec<String>>,
) -> Option<Vec<String>> {
    values_opt.map(|values| {
        let branches: Vec<ParsedGitBranchLine> = values
            .iter()
            .filter_map(|line| parse_git_branch_line(line))
            .filter(|branch| !branch.is_linked_worktree)
            .collect();

        // Keep the current branch first (denoted by *), preserving relative order
        // for the remaining branches.
        let (current_branches, other_branches): (Vec<_>, Vec<_>) =
            branches.into_iter().partition(|branch| branch.is_current);

        current_branches
            .into_iter()
            .chain(other_branches)
            .map(|branch| GitBranchOnClickValue::new(branch.branch_name).encode())
            .collect()
    })
}

/// Filter the output of `git --no-optional-locks worktree list
/// --porcelain` into encoded click values for the worktree chip.
/// Worktrees are returned in the order git emits them (main first).
pub(crate) fn filter_git_worktree_on_click_values(
    values_opt: Option<Vec<String>>,
) -> Option<Vec<String>> {
    values_opt.map(|values| {
        let mut entries: Vec<GitWorktreeOnClickValue> = Vec::new();
        let mut current_path: Option<String> = None;
        let mut current_branch: Option<String> = None;

        let mut flush = |path: &mut Option<String>, branch: &mut Option<String>| {
            if let Some(path) = path.take() {
                entries.push(GitWorktreeOnClickValue::new(path, branch.take()));
            } else {
                // Discard the branch if we never saw a path (shouldn't happen
                // for well-formed porcelain output, but be defensive).
                branch.take();
            }
        };

        for line in values {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                flush(&mut current_path, &mut current_branch);
                continue;
            }
            if let Some(path) = trimmed.strip_prefix("worktree ") {
                flush(&mut current_path, &mut current_branch);
                current_path = Some(path.to_string());
            } else if let Some(branch_ref) = trimmed.strip_prefix("branch ") {
                current_branch = branch_ref
                    .strip_prefix(GIT_BRANCH_REF_PREFIX)
                    .map(str::to_string);
            }
            // Other porcelain attributes (HEAD, detached, bare, locked, …)
            // are not needed for the click action.
        }
        flush(&mut current_path, &mut current_branch);

        entries.into_iter().map(|entry| entry.encode()).collect()
    })
}

fn parse_git_branch_line(line: &str) -> Option<ParsedGitBranchLine> {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return None;
    }

    let status_marker = ['*', '+'].into_iter().find_map(|marker| {
        trimmed.strip_prefix(marker).and_then(|rest| {
            rest.chars()
                .next()
                .filter(|c| c.is_whitespace())
                .map(|_| marker)
        })
    });

    let branch_name = match status_marker {
        Some(marker) => trimmed
            .strip_prefix(marker)
            .map(str::trim)
            .unwrap_or(trimmed),
        None => trimmed,
    };

    if branch_name.is_empty() {
        return None;
    }

    Some(ParsedGitBranchLine {
        branch_name: branch_name.to_string(),
        is_current: status_marker == Some('*'),
        is_linked_worktree: status_marker == Some('+'),
    })
}

/// Returns `true` when `name` looks like a plausible git branch name that can
/// be created via `git checkout -b`.
///
/// We err on the side of letting git itself reject borderline cases: this
/// helper only filters out the most obviously broken inputs so that the
/// "Create new branch …" affordance does not appear for clearly invalid
/// queries (e.g. an empty string after the user backspaces, or whitespace).
/// Anything we accept here may still be rejected by `git check-ref-format`,
/// in which case the user sees the failure in the terminal.
pub(crate) fn is_plausible_new_branch_name(name: &str) -> bool {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return false;
    }
    // git rejects names beginning with `-` outright, and they would also be
    // ambiguous with `git checkout -b` flags, so don't offer the affordance.
    if trimmed.starts_with('-') {
        return false;
    }
    // git refuses whitespace (other than as a separator) inside refs.
    if trimmed.chars().any(char::is_whitespace) {
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
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
        let value = format!(
            "feature-a{ENCODED_VALUE_SEPARATOR}worktree{ENCODED_VALUE_SEPARATOR}/repo/feature-a"
        );
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
                // parsed as a worktree marker — it stays as a branch.
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
}
