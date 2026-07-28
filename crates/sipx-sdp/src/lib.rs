//! SDP session descriptions (RFC 8866) and offer/answer negotiation (RFC 3264).
//!
//! Session descriptions are parsed into a typed AST rather than a bag of lines, and
//! negotiation is a pure function of local capabilities and the remote description — no
//! sockets, no shared mutable session object.
