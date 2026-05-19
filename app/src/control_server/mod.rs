//! In-process control surface for a running Warp GUI instance.
//!
//! A future `launch()` will bind a per-instance Unix domain socket
//! (e.g. `~/Library/Application Support/dev.warp.WarpOss/control-<pid>.sock`)
//! and serve a small JSON-RPC protocol that mirrors the `warp control …`
//! CLI surface (`warp_cli::control`).
//!
//! Modeled on `crate::remote_server::unix::launch_daemon`. See `CLAUDE.md`
//! ("Warp-as-CLI" section) for the seam, RPC list, and rationale.
//!
//! Status: stub. `launch()` is a no-op so wiring it into the app's startup
//! singleton chain is safe ahead of the real implementation landing.

use warpui::AppContext;

/// Bind the control socket and start serving requests.
///
/// Currently a no-op. Will be filled in once the socket path scheme,
/// framing, and RPC dispatch are in place.
pub fn launch(_ctx: &mut AppContext) {
    log::debug!("control_server::launch — not yet implemented (scaffolding)");
}
