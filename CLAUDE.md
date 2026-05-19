# CLAUDE.md

Notes for Claude Code sessions. For general engineering guidance see `WARP.md`.

## Project intent

This fork is being shaped for **the maintainer's personal use of Warp**, not for upstream contribution. Bias toward customizations, integrations, and ergonomic tweaks for one user. Don't worry about preserving features that don't matter to them; do worry about not breaking what they actually use day-to-day.

## Shipped: `warp-oss control …` (branch `feat/control-cli`)

A shell-invokable CLI that **controls the running Warp instance** over a local Unix-domain socket. Outside processes (other shells, scripts, AI agents, Raycast) drive Warp through it. This is the inversion of Warp's existing MCP support (`app/src/ai/mcp/parsing.rs`), which only lets Warp's own agent *consume* MCP servers. It's a terminal-automation surface, not an AI feature, so BYOK / `SoloUserByok` gating does **not** apply.

There is a dedicated skill at `.agents/skills/warp-control/SKILL.md` that documents the day-to-day usage; load it whenever you (Claude) want to interact with a running Warp instance.

### What's wired today

| Subcommand | Status |
|---|---|
| `tab list` / `tab new` / `tab close <id>` / `tab focus <id>` | ✅ |
| `pane list [--tab <id>]` / `pane send <id> "<cmd>"` / `pane read [--pane <id>] [--blocks N]` | ✅ |
| `pane focus <id>` / `pane close <id>` / `pane split [--pane <id>] --direction <left\|right\|up\|down>` | ✅ |
| `block list [--pane <id>] [--limit N]` / `block read <id>` | ✅ |

The CLI reports `active` for the focused tab and `focused` for the focused pane (`Workspace::active_tab_index` + `PaneGroup::focused_pane_id`). When `--pane` is omitted, requests default to the focused pane of the active tab. `pane focus <id>` automatically activates the owning tab so cross-tab focus actually moves the user.

**Target workspace selection.** Requests target the frontmost Warp window via `ctx.windows().active_window()` → `WorkspaceRegistry::get`. When no Warp window is frontmost (typical when invoking the CLI from another terminal), the server falls back to any registered workspace — unambiguous with one Warp instance, "first registered" with multiple (focus the desired window first to disambiguate).

**Hidden-for-close panes.** `PaneGroup` keeps closed panes in `pane_contents` for undo-close. The list/lookup helpers filter them via `PaneGroup::is_pane_hidden_for_close` so responses match what the user sees.

**Platform gating.** The control surface is `#[cfg(unix)]`. On Windows/WASM, `warp control …` returns a clear "only available on Unix targets" error. The Unix-only `std::os::unix::net::Unix*` types are never reached on non-Unix builds.

### Architecture (where the code lives)

- **Wire protocol** — `app/src/control_server/wire.rs`: `Request` / `Response` enums with `serde(tag = "kind", rename_all = "snake_case")`. Block ids are `String` (matches Warp's `BlockId(String)`); tab/pane ids are `u64` (`EntityId` as a number).
- **Framing** — `app/src/control_server/framing.rs`: 4-byte big-endian length prefix + JSON bytes, with both async (server) and sync (CLI client) helpers.
- **Server** — `app/src/control_server/mod.rs`: binds `~/Library/Application Support/dev.warp.WarpOss/control.sock` (perm `0o600`) on `LaunchMode::App` startup. `ControlModel` singleton owns the accept loop; each connection is a one-shot request → response. Handlers hop onto the main thread via `ModelSpawner` to read/write the live Entity model.
- **CLI client** — `app/src/cli_control/mod.rs`: synchronous Unix-socket client, pretty-prints responses to stdout.
- **Fast path** — `app/src/lib.rs`: `Control` subcommands short-circuit before `run_internal`, so a CLI invocation is purely a client. Without this, every call would spin up a full GUI app context (incl. a `terminal-server` child) and collide on the socket.

### Important quirks

- **Active-tab dispatch.** `NewTab` / `CloseTab` call `Workspace::handle_action(&WorkspaceAction::…, ctx)` directly. Earlier attempts used `dispatch_typed_action` and silently no-op'd ("no view handled it") because the singleton-model background task isn't in any focus chain.
- **`pane split`** uses the same pattern: locate the owning `PaneGroup` view and call `handle_action(&PaneGroupAction::Add(Direction::…), ctx)`. That's the same code path keybindings use, so splits inherit shell-selection / focus behavior.
- **exFAT codesign trap (build).** When the `target/` directory is a symlink to an external exFAT drive, `codesign` chokes on AppleDouble `._*` files. Workaround: copy the bundled `.app` to APFS, then `codesign --force --deep` there. See "macOS local-build gotchas" below.
- **Restart resets sessions.** Killing/relaunching `warp-oss` wipes live shells (SSH sessions especially). The tab structure is restored from disk but PTYs aren't.

### Design context (kept for reference)

Architectural seams that look promising:
- `warp-oss terminal-server --parent-pid=...` subprocess — Warp already separates the UI from the terminal/PTY backend. That separation implies an internal IPC that could be exposed or proxied.
- `with_local_server` feature + `SERVER_ROOT_URL` env var — there's already plumbing for a local server context.
- "Ambient agent" / "remote session" infra in recent commits suggests programmatic session control is on the roadmap internally.

Design — CLI only, no MCP layer:
1. The running Warp app opens a Unix-domain socket (e.g. `~/Library/Application Support/dev.warp.WarpOss/control-<pid>.sock`) and serves a small JSON-RPC protocol.
2. The same `warp-oss` binary grows new CLI subcommands (`warp session/pane/block/run`) that connect to that socket and proxy the request.
3. **No MCP server.** Agents that want to drive Warp shell out to the CLI directly — Claude Code, Cursor, Codex, and friends all support running bash. MCP would just be an extra hop.

Reference designs to borrow from:
- `tmux send-keys` / `tmux capture-pane` / `tmux -C` control mode — closest analogue.
- `code` CLI for VS Code (`code --diff`, etc.).
- iTerm2's Python API / WebSocket daemon.

Open questions before committing:
- How much session/block state is reachable without rebuilding the data model? Need to map the `warpui` ↔ terminal-server IPC.
- Sandboxing/entitlements (`script/Debug-Entitlements.plist`) — Unix socket access has to be allowed.
- Stable identifiers for blocks across sessions for outside callers to reference.
- Auth/scoping: should the socket gate by Unix permissions only, or add a per-call token? Local-only by default seems fine.

#### Investigation findings (the seam to cut)

The architecture is more favorable than I first thought. **The biggest surprise: Warp already has a multi-subcommand CLI** (`warp-oss <subcmd>`) and **a near-identical Unix-socket daemon pattern** we can copy almost verbatim.

Key facts from the code:

- **`warp-oss terminal-server` is NOT the session brain.** It's a minimal forked subprocess whose only job is to spawn shell PTYs in a clean state. After spawn, the UI process talks **directly to the PTY** (Unix domain sockets pass the PTY fd back to the UI). Session/block state lives in the UI process. See `app/src/terminal/local_tty/server/mod.rs` (the leading doc comment is explicit about this).

- **The UI process owns everything we care about:**
  - `Block` struct at `app/src/terminal/model/block.rs:286` (id, header/output grids, state, exit code, timestamps, session_id, …).
  - `BlockList` at `app/src/terminal/model/blocks.rs:239` (holds `block_id_to_block_index: HashMap<BlockId, BlockIndex>`).
  - `TerminalModel` at `app/src/terminal/model/terminal_model.rs:453`.
  - Panes identified by `TerminalPaneId(EntityId)` and `PaneUuid(Vec<u8>)` (`app/src/app_state.rs:36, 99`).
  - All live inside the `warpui` Entity/AppContext system, so access requires `ctx.read(entity, |…|)`-style hops on the UI thread (or via `spawner` from a background task).

- **CLI plumbing is already established.** `crates/warp_cli/src/lib.rs` defines a `Command` enum that flattens `WorkerCommand` (hidden subprocess subcommands) and `CliCommand` (user-facing CLI surface for "scripting Warp functionality" — the doc comment says so verbatim). Existing user-facing subcommands: `Agent`, `Environment`, `MCP`, `Run`/`Task`, `Model`, `Login`, `Logout`, `Whoami`, `Provider`, `Integration`, `Schedule`, `Secret`, `Federate`, `Artifact`, `ApiKey`. None of them currently talk to a running GUI — they're all stateless or cloud-bound. A new `Session` / `Block` / `Pane` subcommand family would fit cleanly.

- **There's an existing Unix-socket daemon to copy.** `RemoteServerDaemon` (`app/src/remote_server/unix/mod.rs:44`) is exactly the pattern needed:
  ```
  let listener = UnixListener::bind(&socket_path)?;
  std::fs::set_permissions(&socket_path, Permissions::from_mode(0o600))?;
  ctx.add_singleton_model(move |ctx| {
      ctx.background_executor().spawn(async move {
          let listener = async_io::Async::new(listener)?;
          loop {
              let (stream, _) = listener.accept().await?;
              background_executor.spawn(handle_connection(stream, spawner.clone())).detach();
          }
      }).detach();
      ServerModel::new(ctx)
  });
  ```
  The per-connection handler splits a reader task + writer loop with a per-connection mpsc channel. The reader hands decoded requests to the `ServerModel` via `spawner`, which is the bridge back onto the UI thread for Entity reads/writes. We'd do the same with a new `ControlModel`.

- **Two-process model is already in CLI shape.** `WorkerCommand::RemoteServerProxy` is a "thin byte bridge (stdin/stdout ↔ Unix socket)" subcommand. Our agent-bridge equivalent would be `warp session list` connecting to `~/Library/Application Support/dev.warp.WarpOss/control.sock`, sending one request, printing the response. Trivial.

**Concrete seam:**

1. **New CLI subcommands** under `CliCommand` in `crates/warp_cli/src/lib.rs`:
   - `Session(SessionCommand::{List, Focus, Spawn})`
   - `Pane(PaneCommand::{List, Focus, Read, Send})`
   - `Block(BlockCommand::{List, Read, Tail, Wait})`
   - `Run { command: String, pane: Option<String>, wait: bool }` (sugar = `pane send` + `block wait`)
   
   These resolve to client code that opens `control.sock` and sends a JSON-RPC request.

2. **New worker / launch-mode-agnostic singleton** in the main GUI app:
   - Add `app/src/control_server/mod.rs` modeled on `app/src/remote_server/unix/mod.rs`.
   - Hook it into `run_internal(LaunchMode::App { .. })` in `app/src/lib.rs` so the socket only listens when the GUI is running (not for `RemoteServerProxy`, `TerminalServer`, etc.). Search for `LaunchMode::App { .. }` matches around lines 347, 366, 416 to find the right `add_singleton_model` chain (lines 1038–1328).
   - Socket path: per-instance, scoped by PID + app bundle id, e.g. `$XDG_RUNTIME_DIR/dev.warp.WarpOss/control-<pid>.sock` (macOS: `~/Library/Application Support/dev.warp.WarpOss/control-<pid>.sock`). Permissions `0o600`.
   - On startup, also write a discovery file (`active.sock` symlink or `instances.json`) so a CLI invocation with no `--pane` flag knows which Warp to talk to. If there's exactly one instance, default to it.

3. **`ControlModel` singleton** that holds:
   - Access to the entity graph (via `AppContext` snapshots passed by `spawner`).
   - A registry of subscriptions (for `block tail` long-lived requests).
   - A small dispatcher mapping `Request::{ListSessions, ListBlocks { pane }, ReadBlock { id }, SendInput { pane, text }, FocusPane { pane }, SubscribeOutput { pane }}` to entity reads/writes.

4. **Wire format:** length-prefixed JSON frames (same 4-byte framing the remote-server daemon already uses — `app/src/remote_server/unix/proxy.rs:32` referenced "the existing 4-byte length-prefixed frame format"). Re-use that framing module so we don't reinvent it.

5. **Stable IDs:**
   - `BlockId` already exists and is stable within a session.
   - `PaneUuid(Vec<u8>)` for cross-session pane references.
   - `SessionId` for top-level sessions.
   - Surface all three in API responses; let the CLI accept either UUID or short prefix.

**Estimated effort for a usable v0** (list/read/send/focus, no streaming):
- ~1 day to copy the daemon socket-server scaffold into a new `control_server` module and wire it into `LaunchMode::App` startup.
- ~2 days to implement the first four RPCs against the Entity model (`ListSessions`, `ListPanes`, `ReadBlock`, `SendInput`).
- ~1 day to add the matching `warp session/pane/block` CLI subcommands and the framing client.

Streaming (`block tail`, `subscribe_output`) is another 2-3 days because it needs event subscriptions on the Entity side, not just one-shot reads.

#### Current state

Implemented and merged to `feat/control-cli` (pushed to the fork). See the
table at the top of this file for what's wired vs. stubbed.

**Risks:**
- Entity reads on background tasks must go through `spawner.spawn_in_main`-style indirection — can't just lock the model. Adds latency (single-digit ms per RPC).
- For `SendInput`, need to find the right action/event type that the existing UI uses for keystroke injection. Look at how `app/src/terminal/input.rs` dispatches keystrokes; reuse that path so we don't bypass any state machine the UI relies on.
- macOS sandbox/Gatekeeper: the dev codesign entitlements file may need updating if it disallows `bind()` on Unix sockets. (`script/Debug-Entitlements.plist` — currently inspected only briefly during this run; verify before shipping.)

## macOS local-build gotchas (discovered the hard way)

These are not covered by `./script/bootstrap` but bite anyone running `./script/run`:

### 1. PATH trap: Anaconda ships its own `codesign`

`./script/run` calls plain `codesign` (no absolute path). If your `$PATH` contains an Anaconda or similar Python environment before `/usr/bin`, that `codesign` shim wins and the signing step fails with:

```
The following arguments were not expected: --options --deep
Run with --help for more information.
```

Workaround for the invocation:

```bash
PATH="/usr/bin:$PATH" ./script/run
```

`which -a codesign` will reveal a shadowing binary if this happens.

### 2. exFAT + `codesign` is broken

If `target/` lives on an exFAT volume (e.g. a symlink to an external drive to save internal disk space), `codesign` fails with:

```
target/debug/bundle/osx/WarpOss.app: Operation not permitted
In subcomponent: .../Contents/._Info.plist
```

macOS writes xattrs to AppleDouble `._*` files on non-APFS/HFS+ filesystems, and `codesign` treats them as unsigned sub-bundles. `codesign` itself writes new xattrs so the problem is self-perpetuating — you cannot just `dot_clean` once.

Recovery: keep `target/` on exFAT for the compile (it works fine for cargo), but copy the bundle to APFS before signing:

```bash
SRC="target/debug/bundle/osx/WarpOss.app"
DEST="$HOME/Library/Application Support/WarpOss-local-build/WarpOss.app"
mkdir -p "$(dirname "$DEST")"
rm -rf "$DEST"
ditto "$SRC" "$DEST"
dot_clean -m "$DEST"
CERT="$(security find-identity -p codesigning -v | grep "Apple Development" | awk '{print $2}' | head -1)"
/usr/bin/codesign --force --deep --options runtime --sign "${CERT:--}" "$DEST" --entitlements script/Debug-Entitlements.plist
"$DEST/Contents/MacOS/warp-oss"
```

### 3. Metal Toolchain is required, not always installed

`crates/warpui/build.rs` invokes `xcrun -sdk macosx metal -c …` to compile shaders. Plain Xcode does NOT ship the Metal Toolchain by default. The build fails with:

```
error compiling metal shaders to .air; error: cannot execute tool 'metal' due to missing Metal Toolchain;
use: xcodebuild -downloadComponent MetalToolchain
```

Fix: `xcodebuild -downloadComponent MetalToolchain` (~700 MB download). `./script/bootstrap` calls `./script/macos/install_build_deps` which does this, but `./script/run` does not — so if you skipped bootstrap, run this manually.

The Apple CDN occasionally fails the streaming extractor partway through. Just retry.

### 4. Disk-space rule of thumb

A debug `target/` for the full workspace reaches **~25 GB**. Plan for it. If the internal volume is tight, a symlink to an external drive works (cargo doesn't care about the filesystem; only the codesign step does — see #2).

## Recommended OSS-contributor invocation

```bash
PATH="/usr/bin:$PATH" \
WARP_SKIP_COMMON_SKILLS_INSTALL=1 \
./script/run
```

- `WARP_SKIP_COMMON_SKILLS_INSTALL=1` avoids the interactive prompt about installing common agent skills.
- `./script/install_channel_config` failing with "no SSH access" is expected for external contributors — the script handles it gracefully and falls back to the `oss` channel (binary becomes `warp-oss`).

## Incremental builds

After the first cold compile, `./script/run` rebuilds in tens of seconds for no-op runs, or under a minute for single-crate edits. The slow steps that always re-run regardless: `cargo bundle` (repackages the .app), `codesign --force --deep`, `prepare_bundled_resources`, `compile_icon`. Together those add ~10–20s.

A full recompile is only triggered by: `cargo update`, Cargo.lock changes, `rust-toolchain.toml` channel bump, or `cargo clean`.

## BYOK / custom AI — important to know before debugging the AI layer

The OSS client cannot call LLM providers directly. BYOK keys and custom OpenAI-compatible endpoints are bundled into the request to Warp's backend (`crates/ai/src/api_keys.rs:241-289` — `custom_model_providers_for_request` ships keys + URL in `warp_multi_agent_api::request::settings::CustomModelProviders`). Warp's server proxies the actual provider call. BYOK is also server-gated (`SoloUserByok` feature flag, workspace billing metadata, anonymous users blocked at `app/src/workspaces/user_workspaces.rs:491`), so a successful local build does not unlock BYOK on its own.
