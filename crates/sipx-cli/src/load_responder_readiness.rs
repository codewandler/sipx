//! Machine-readable start barrier for the bounded load responder.

use std::io::Write as _;

use sipx_transport::Handle;

pub(crate) fn emit(
    endpoint: &Handle,
    max_active: usize,
    queued_events: usize,
) -> std::io::Result<()> {
    let ready = serde_json::json!({
        "schema": "sipx.comparative-load.ready.v1",
        "role": "responder",
        "pid": std::process::id(),
        "address": endpoint.local_addr().to_string(),
        "transport": "udp",
        "limits": {
            "active": max_active,
            "events": queued_events,
            "stdout_bytes": 16 * 1024 * 1024,
            "stderr_bytes": 16 * 1024 * 1024,
        }
    });
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{ready}")?;
    stdout.flush()
}
