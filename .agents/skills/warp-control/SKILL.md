---
name: warp-control
description: |
  Drive a running Warp instance from the shell via `warp-oss control …` — list
  tabs, send commands to specific panes, read block output back, capture
  structured pane snapshots, wait for text, open/close tabs, split panes, and
  start/stop native Warp session sharing. Use when you
  want to run a command in a particular Warp tab/pane (not your own shell),
  inspect what's on screen in another Warp tab, monitor or coordinate work
  across panes, share a pane to a phone/browser, or open/close/split tabs from
  outside Warp. Requires a running Warp build with control-socket support
  (current fork/master or `feat/control-cli`, socket at
  `~/Library/Application Support/dev.warp.WarpOss/control.sock`).
---

# warp-control — Drive Warp from the shell

## When to use this skill

Use the `warp-oss control` CLI whenever you need to **read or write inside a running Warp window** without taking over the user's keyboard. Concretely:

- "Run X in tab Y" — `pane send`
- "What's on the active tab" / "What did that command print" — `tab list` + `pane read`
- "Give me agent-readable pane state" — `pane snapshot --json`
- "Wait until the prompt/output says X" — `pane wait-for-text`
- "Open a fresh tab for the deploy" — `tab new`
- "Split this pane to the left" — `pane split`
- "Share this pane to my phone" — `pane share` + `pane share-link`
- "Close that scratch tab" — `tab close`

If the user is just asking to run a one-off command and doesn't care which terminal it lands in, **don't** route through this skill — just use Bash. Use this skill when the destination terminal matters (a specific tab, a specific shell, an SSH session the user is in, etc.).

## Iron laws

1. **Check Warp is running first.** `pgrep -lf warp-oss` should show at least the parent process. If not, the CLI will fail with `could not connect to Warp control socket … — is Warp running?`. Don't try to start Warp from this skill; ask the user.
2. **Always start with `tab list` and `pane list`.** Don't guess IDs. The active/focused markers in those tables drive every other call.
3. **Use `--wait` for one-shot shell commands.** `pane send --wait [--timeout <secs>]` blocks on the exact block it submitted and prints the output plus exit code. Plain `pane send` still returns `ok` as soon as the command is accepted/queued; reserve no-wait mode for dev servers, tails, long-running commands, and TUI launches.
4. **Distinguish "active" from "focused".** A tab is *active* if it's the foreground tab in the workspace. A pane within a tab is *focused* if it's the one that would receive keystrokes if the user typed. When `--pane` is omitted, the CLI targets the focused pane of the active tab.
5. **Don't restart Warp casually.** Killing `warp-oss` wipes all live shells (SSH sessions especially). Tabs are restored from disk on relaunch; their PTYs are not.

## CLI surface

The binary is the same `warp-oss` that runs the GUI; the `control` subcommand is a fast-path client that only talks to the socket.

```
warp-oss control tab   list
warp-oss control tab   new
warp-oss control tab   close <id>
warp-oss control tab   focus <id>

warp-oss control pane  list      [--tab <id>]
warp-oss control pane  send      [--pane <id>] [--wait] [--timeout <secs>] "<command>"  # block exec; pane is a --FLAG
warp-oss control pane  write     [--pane <id>] "<text>"       # raw bytes to PTY, no \n
warp-oss control pane  keystroke [--pane <id>] <key>          # named key / ctrl-<char>
warp-oss control pane  read      [--pane <id>] [--blocks N]   # default N=10
warp-oss control pane  screen    [--pane <id>]                # live screen grid
warp-oss control pane  snapshot  [--pane <id>] [--blocks N] [--no-screen] [--json]
warp-oss control pane  wait-for-text [--pane <id>] [--mode screen|blocks|both] [--block-field output|command|both] [--since all|now] [--regex] [--timeout <secs>] [--json] "<text>"
warp-oss control pane  focus     <id>                         # also activates the owning tab
warp-oss control pane  close     <id>
warp-oss control pane  split     [--pane <id>] --direction <left|right|up|down>

warp-oss control pane  share      [--pane <id>] [--scrollback none|all]  # start sharing
warp-oss control pane  share-link [--pane <id>]               # print the watch URL
warp-oss control pane  unshare    [--pane <id>]               # stop sharing

warp-oss control block list [--pane <id>] [--limit N]
warp-oss control block read  <id>                             # id from `block list`
```

### `send` vs `write` vs `keystroke` — when to use which

| Goal | Use | Notes |
|---|---|---|
| Run a shell command and capture its output as a Warp block | `pane send --wait` | The normal agent case. Goes through Warp's command-block submission and waits for completion. |
| Start a long-running command, dev server, tail, or TUI | `pane send` without `--wait` | Fire-and-forget. Follow with `pane read`, `pane screen`, or raw input calls as appropriate. |
| Drive a TUI app (vim, fzf, less, claude, htop, ssh password prompts) | `pane write` + `pane keystroke` | Bytes go straight to the PTY. No newline appended unless you ask for one. |
| **Read what a TUI app is showing** (vim, tmux, less, claude, htop) | `pane screen` | Captures the live screen grid. `pane read` only sees command *blocks*, so it's blind inside full-screen apps — use `pane screen` there. Output is tagged `(alt-screen)` when a TUI is active, `(primary)` otherwise. |
| Get machine-readable pane state for an agent | `pane snapshot --json` | Returns schema-versioned JSON with pane metadata, captured_at, screen text, recent blocks, and truncation flags. Alias: `pane snap`. |
| Wait for a prompt, server readiness line, or TUI text | `pane wait-for-text` | Polls screen and/or recent block output by default. Use `--since now` for new output, `--mode screen` for TUI-only waits, `--regex` for patterns, `--block-field command|both` when command text should count, and `--json` for match + final snapshot. Alias: `pane wait`. |
| Send a special key (Enter, Esc, arrows, Tab, Backspace, function keys, ctrl-c…) | `pane keystroke` | Recognized names: `enter` `return` `tab` `esc` `escape` `space` `backspace` `delete` `ins` `up` `down` `left` `right` `home` `end` `pageup` `pagedown` `f1`–`f12`. Chords: `ctrl-<char>` or `c-<char>` (e.g. `ctrl-c`, `ctrl-d`, `ctrl-[` = Esc, `ctrl-?` = Backspace). |

**Mixing the two paths is fine.** A common pattern: `pane send --pane <id> vim file.txt` to launch the TUI through the shell, then switch to `pane write` / `pane keystroke` for everything inside vim.

`pane send` accepts the command as trailing args, so you can usually skip quoting (`pane send --pane 1990 ls -la /tmp` works). Quote only when the command contains shell operators you want the *target* pane's shell to interpret (pipes, redirects, `&&`, etc.).

> ⚠️ **GOTCHA — `--pane` is a FLAG, not positional.** For `send`, the pane id must be `--pane <id>`. If you write `pane send 1990 "cmd"`, the `1990` gets swallowed into the trailing-args command (it runs `1990 cmd` → `command not found: 1990`) and `--pane` defaults to the *focused* pane — so it silently targets the wrong pane with garbage. `write`/`keystroke`/`read`/`screen` also take `--pane`. Always use the flag.

End-to-end vim example, fully driven from outside:

```bash
WARP=… ; P=2794
"$WARP" control pane send      --pane $P vim /tmp/scratch.txt ;  sleep 1
"$WARP" control pane write     --pane $P "i"                  ;  sleep 0.2  # insert mode
"$WARP" control pane write     --pane $P "hello"
"$WARP" control pane keystroke --pane $P esc                  ;  sleep 0.2
"$WARP" control pane write     --pane $P ":wq"
"$WARP" control pane keystroke --pane $P enter
cat /tmp/scratch.txt   # → hello
```

Where the binary lives (use the Applications bundle by default):

- `/Applications/WarpOss.app/Contents/Resources/bin/warp-oss` ← **preferred shell wrapper**
- `/Applications/WarpOss.app/Contents/MacOS/warp-oss` ← canonical app binary
- `/Volumes/ThinkPlus/warp-target/debug/warp-oss` (fresh local build — only when testing an unreleased build)

The `warp` shell alias points at the same binary. The `control` command is just a *client* of the running Warp GUI (the server) over the shared socket, so which binary you invoke doesn't change behavior — default to the `/Applications` one.

## Standard workflow

```bash
WARP=/Applications/WarpOss.app/Contents/Resources/bin/warp-oss   # canonical install wrapper

# 1) Survey state — identify which tab/pane you want to talk to.
"$WARP" control tab list
"$WARP" control pane list

# 2) Send the command and wait for the submitted block. `--pane` is optional and defaults to the focused pane —
#    omit it when you want to talk to whatever the user is currently looking at.
"$WARP" control pane send --pane 1990 --wait --timeout 120 "ls -la && pwd"

# 3) Inspect the printed block. It contains command, output, exit_code, and pwd.
```

For no-wait commands, use `pane read --pane <id> --blocks 2` after the command has had time to emit a block.

## Reading output

`pane send --wait` prints the submitted block in the same shape as `pane read`. Use `pane read` for follow-up polling, no-wait commands, TUI launches, and long-running processes.

For automation, prefer `pane snapshot --json` over scraping `pane read` or `pane screen`. It includes the live screen plus recent blocks in one response:

```bash
"$WARP" control pane snapshot --pane 1990 --blocks 3 --json
```

When sequencing against text that may appear later, prefer `wait-for-text` to manual sleep loops:

```bash
"$WARP" control pane wait-for-text --pane 1990 --since now --timeout 30 "Ready"
"$WARP" control pane wait-for-text --pane 1990 --mode screen --regex "claude.*>" --case-insensitive
```

For block waits, `--block-field output` is the default. That avoids false positives when a completion sentinel or target phrase appears in the submitted command itself. Use `--block-field command` only for command-submission checks, and `--block-field both` when either command text or output is intentionally acceptable.

`pane read` prints the last N blocks, each formatted as:

```
--- block <id> (pane <pane-id>) ---
pwd: /Users/kira-chan
$ <command>
<output...>
(exit <code>)
```

Trailing `precmd-…` blocks with no `$` line are idle shell prompts — skip them when looking for command output. Your `echo X` output is in the block whose `$` line shows `echo X`.

## Common patterns

**Run a command and capture its output in one go:**
```bash
"$WARP" control pane send --pane 1990 --wait "<cmd>"
```

**Tail a long-running command** — re-read every few seconds:
```bash
"$WARP" control pane send --pane 1990 "cargo test --workspace 2>&1"  # no --wait: this may run for a while
while sleep 5; do
  "$WARP" control pane read --pane 1990 --blocks 1
done
```

**Open a fresh tab for SSH:**
```bash
"$WARP" control tab new
sleep 2
# Newest tab is the now-active one — `tab list` shows it, the new pane is
# also the focused one, so `--pane` can be omitted.
"$WARP" control pane send ssh user@host
```

**Open a saved tab config by name (e.g. an SSH tab):**
```bash
# Opens the tab config whose `name` field matches, via the same path the
# new-session menu uses. SSH configs auto-run `ssh <host>` on launch.
"$WARP" control tab new --config "SSH: claude-code"
# Unknown name → error listing the available config names.
```

**Split for diff-style side-by-side work:**
```bash
"$WARP" control pane split --direction right
sleep 1
"$WARP" control pane list   # the new pane appears in the same tab
```

**Drive multiple panes independently** — they're separate `SessionId`s with independent BlockLists, no cross-talk:
```bash
"$WARP" control pane send --pane 3106 "tail -f /var/log/foo.log"
"$WARP" control pane send --pane 2415 "tail -f /var/log/bar.log"
```

**Share a pane and get a watch-on-any-device link** — exposes Warp's native session sharing. Sharing requires Warp to be **logged into a Warp account** (it connects to Warp's sharing server). Session setup is **async**, so `share` returns immediately as *pending* and you poll `share-link` until the URL appears:
```bash
"$WARP" control pane share --pane 65777            # → "sharing started for pane 65777 (pending)"
# poll until the link is ready (usually a second or two):
until URL=$("$WARP" control pane share-link --pane 65777 2>/dev/null) && [ -n "$URL" ]; do sleep 1; done
echo "$URL"                                         # → https://app.warp.dev/session/<id>  (open on any device)
"$WARP" control pane unshare --pane 65777           # stop sharing when done
```
`pane share-link` prints the bare URL on success (script-friendly), `sharing pending …` while setup is still in flight, and errors if the pane isn't sharing. `--scrollback all` includes prior scrollback in the shared view; default `none` shares only from now on. Viewers join read-only or as executor depending on the role Warp grants them. Current caveat: sharing setup is async, so backend failures such as quota/plan limits may only appear in the Warp UI/logs after `pane share` has already printed `pending`.

## Failure modes you'll see

| Symptom | Meaning | Fix |
|---|---|---|
| `could not connect to Warp control socket … — is Warp running?` | No Warp instance, or one is mid-shutdown. | `pgrep -lf warp-oss`; if missing, ask the user to launch it. |
| `pane <id> not found` | Stale id from before a tab was closed / app restarted. | Re-run `pane list` and use the fresh id. |
| `tab <id> not found` (for `tab close`) | Same as above. | Re-run `tab list`. Note `tab close` accepts either the tab id OR the index. |
| Plain `pane send` returns `ok` but `pane read` shows nothing | No-wait mode accepted/queued the command before output was available, or you're reading too few blocks. | Prefer `pane send --wait` for one-shot commands. Otherwise `sleep 2; pane read --blocks 5`; long commands need more time. |
| `pane send` returns `pane cannot execute commands while another command is already running` or `shell bootstrapping is still in progress` | The pane is not executable right now. PR #16 fixed the old silent false-`Ok` for most of these cases. | Wait for the pane to become idle/bootstrapped, then retry. For interactive programs, use `pane write` + `pane keystroke`. |
| Immediate back-to-back no-wait `pane send` calls both return `ok`, but the second command fuses into the first output/prompt | Remaining race before Warp marks the first command as running. | Use `--wait` for sequencing. For no-wait commands, wait briefly or poll `pane read`/`pane screen` before sending another shell command. |
| Fresh `tab new` followed immediately by `pane send --wait` returns a bootstrapping error but the command later runs anyway | Known side-effect-on-error path: the command can be left pending during shell startup. | After `tab new`, wait a couple seconds and re-run `pane list`/`pane read` before the first `pane send`. |
| `pane wait-for-text` times out | The requested text/regex did not appear in the selected search surface before `--timeout`. | Re-check the target pane id and `--mode`; use `--json` to inspect the final snapshot returned with the timeout. |
| `pane share` prints `pending`, then `pane share-link` says the pane is not sharing | Async share setup failed after the CLI returned. Today the CLI does not surface the backend reason. | Check the Warp UI/logs for quota/auth/network errors; retry after fixing the account/plan issue. |
| `--output-format json` / `WARP_OUTPUT_FORMAT=json` still prints pretty tables | Global output-format plumbing is accepted but not wired for most control responses yet. | Use explicit JSON-capable commands: `pane snapshot --json` and `pane wait-for-text --json`. |

## Don't

- **Don't try to read another user's input mid-typing.** The CLI sees committed blocks (post-Enter); it does not stream raw keystrokes.
- **Don't `pane send` arbitrary shell escapes when the user is mid-task.** Anything you send gets executed in their live shell. Treat it like `cat > /dev/$tty` — confirm before injecting anything destructive.
- **Don't restart Warp to "pick up" CLI changes.** The control surface is part of the running GUI; a restart loses every shell. Only restart when you've built a new binary that needs to be loaded.
