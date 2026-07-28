//! Media sessions: the runtime half of the media stack.
//!
//! Binds RTP and RTCP sockets, applies the result of SDP offer/answer from [`sipx_sdp`],
//! drives the packet machinery in [`sipx_rtp`], and handles the parts of real networks the
//! RFCs leave open — symmetric RTP for NAT traversal, and address learning.
