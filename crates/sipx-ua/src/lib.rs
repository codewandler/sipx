//! SIP user agent.
//!
//! Ties the sans-IO core and the transport layer into the roles applications actually use:
//! a client that issues requests, a server that dispatches them by method, dialogs (RFC
//! 3261 §12) as typed state machines, digest authentication (RFC 7616), and registration.
