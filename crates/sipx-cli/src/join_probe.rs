//! Asking whether a command left anything of itself running — the test half of the join barrier.
//!
//! Every long-running command binds one signalling endpoint at the local address it was given, and
//! the endpoint's driver owns that socket for as long as it is running. So "did this invocation
//! finish before it reported" has a cheap and exact answer: bind the same address again.
//!
//! What makes it an observation rather than a race is *when* it is taken — with no `.await`
//! between the command returning and the probe. A command that joined has already released the
//! socket, because [`sipx_transport::Handle::shutdown`] returns only after the driver's last
//! observable action. A command that did not join has not yielded since, so nothing it left
//! running can have released anything in the meantime, and the address is still taken. Both
//! answers are therefore decided before the probe runs, whichever way the command behaves.

use std::net::{SocketAddr, UdpSocket};

/// A loopback address nothing is using, for the command under test to bind.
pub(crate) fn free_local() -> SocketAddr {
    let reserved = UdpSocket::bind("127.0.0.1:0").expect("reserves a loopback port");
    let address = reserved.local_addr().expect("the reserved address");
    drop(reserved);
    address
}

/// Fail unless `local` is free again, naming the exit class that failed to join.
///
/// Called immediately after the command returns and before anything is awaited.
#[track_caller]
pub(crate) fn assert_released(local: SocketAddr, exit_class: &str) {
    if let Err(error) = UdpSocket::bind(local) {
        panic!(
            "{exit_class}: the signalling endpoint still holds {local} after the terminal record \
             ({error}) — the command reported before its own work was finished"
        );
    }
}
