---
name: warp-control
description: |
  Drive a running Warp instance from the shell via `warp-oss control …` — list
  tabs, send commands to specific panes, read block output back, open/close
  tabs, split panes. Use when you want to run a command in a particular Warp
  tab/pane (not your own shell), inspect what's on screen in another Warp tab,
  monitor or coordinate work across panes, or open/close/split tabs from
  outside Warp. Requires a running Warp build with control-socket support
  (`feat/control-cli` branch, socket at
  `~/Library/Application Support/dev.warp.WarpOss/control.sock`).
---

# warp-control — Drive Warp from the shell

## When to use this skill

Use the `warp-oss control` CLI whenever you need to **read or write inside a running Warp window** without taking over the user's keyboard. Concretely:

- "Run X in tab Y" — `pane send`
- "What's on the active tab" / "What did that command print" — `tab list` + `pane read`
- "Open a fresh tab for the deploy" — `tab new`
- "Split this pane to the left" — `pane split`
- "Close that scratch tab" — `tab close`

If the user is just asking to run a one-off command and doesn't care which terminal it lands in, **don't** route through this skill — just use Bash. Use this skill when the destination terminal matters (a specific tab, a specific shell, an SSH session the user is in, etc.).

## Iron laws

1. **Check Warp is running first.** `pgrep -lf warp-oss` should show at least the parent process. If not, the CLI will fail with `could not connect to Warp control socket … — is Warp running?`. Don't try to start Warp from this skill; ask the user.
2. **Always start with `tab list` and `pane list`.** Don't guess IDs. The active/focused markers in those tables drive every other call.
3. **Read before assuming a command finished.** `pane send` returns `ok` as soon as the command is *queued*, not when it completes. Sleep 1–3s (longer for slow commands), then `pane read --pane <id> --blocks 2` to capture the result block.
4. **Distinguish "active" from "focused".** A tab is *active* if it's the foreground tab in the workspace. A pane within a tab is *focused* if it's the one that would receive keystrokes if the user typed. When `--pane` is omitted, the CLI targets the focused pane of the active tab.
5. **Don't restart Warp casually.** Killing `warp-oss` wipes all live shells (SSH sessions especially). Tabs are restored from disk on relaunch; their PTYs are not.

## CLI surface

The binary is the same `warp-oss` that runs the GUI; the `control` subcommand is a fast-path client that only talks to the socket.

```
warp-oss control tab   list
warp-oss control tab   new
warp-oss control tab   close <id>
warp-oss control tab   focus <id>

warp-oss control pane  list  [--tab <id>]
warp-oss control pane  send  <id> "<command>"             # executes as a block
warp-oss control pane  read  [--pane <id>] [--blocks N]   # default N=10
warp-oss control pane  focus <id>                         # also activates the owning tab
warp-oss control pane  close <id>
warp-oss control pane  split [--pane <id>] --direction <left|right|up|down>

warp-oss control block list [--pane <id>] [--limit N]
warp-oss control block read  <id>                         # id from `block list`
```

Where the binary lives in the maintainer's setup (use whichever exists):

- `/Volumes/ThinkPlus/warp-target/debug/warp-oss` (fresh local builds — preferred for testing)
- `~/Library/Application Support/WarpOss-local-build/WarpOss.app/Contents/MacOS/warp-oss` (codesigned bundle copy)

In a normal install it would be the bundled binary inside `Warp.app`.

## Standard workflow

```bash
WARP=/Volumes/ThinkPlus/warp-target/debug/warp-oss   # adjust to your setup

# 1) Survey state — identify which tab/pane you want to talk to.
"$WARP" control tab list
"$WARP" control pane list

# 2) Send the command. The ID column from `pane list` is what `pane send` takes.
"$WARP" control pane send 1990 "ls -la && pwd"

# 3) Wait briefly for the shell to execute, then read the result.
sleep 2
"$WARP" control pane read --pane 1990 --blocks 2
```

The block immediately above the trailing precmd block is the one your command produced — it contains `command`, `output`, `exit_code`, and `pwd`.

## Reading output

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
"$WARP" control pane send 1990 "<cmd>"
sleep 2
"$WARP" control pane read --pane 1990 --blocks 2
```

**Tail a long-running command** — re-read every few seconds:
```bash
"$WARP" control pane send 1990 "cargo test --workspace 2>&1"
while sleep 5; do
  "$WARP" control pane read --pane 1990 --blocks 1
done
```

**Open a fresh tab for SSH:**
```bash
"$WARP" control tab new
sleep 1
# Newest tab is the now-active one — `tab list` shows it, the new pane is
# also the focused one, so `--pane` can be omitted.
"$WARP" control pane send "$("$WARP" control pane list | awk '/yes/ {print $1; exit}')" \
  "ssh user@host"
```

**Split for diff-style side-by-side work:**
```bash
"$WARP" control pane split --direction right
sleep 1
"$WARP" control pane list   # the new pane appears in the same tab
```

**Drive multiple panes independently** — they're separate `SessionId`s with independent BlockLists, no cross-talk:
```bash
"$WARP" control pane send 3106 "tail -f /var/log/foo.log"
"$WARP" control pane send 2415 "tail -f /var/log/bar.log"
```

## Failure modes you'll see

| Symptom | Meaning | Fix |
|---|---|---|
| `could not connect to Warp control socket … — is Warp running?` | No Warp instance, or one is mid-shutdown. | `pgrep -lf warp-oss`; if missing, ask the user to launch it. |
| `pane <id> not found` | Stale id from before a tab was closed / app restarted. | Re-run `pane list` and use the fresh id. |
| `tab <id> not found` (for `tab close`) | Same as above. | Re-run `tab list`. Note `tab close` accepts either the tab id OR the index. |
| `pane send` returns `ok` but `pane read` shows nothing | Shell hasn't run yet, or you're reading too few blocks. | `sleep 2; pane read --blocks 5`. Long commands need more time. |

## Don't

- **Don't try to read another user's input mid-typing.** The CLI sees committed blocks (post-Enter); it does not stream raw keystrokes.
- **Don't `pane send` arbitrary shell escapes when the user is mid-task.** Anything you send gets executed in their live shell. Treat it like `cat > /dev/$tty` — confirm before injecting anything destructive.
- **Don't restart Warp to "pick up" CLI changes.** The control surface is part of the running GUI; a restart loses every shell. Only restart when you've built a new binary that needs to be loaded.
