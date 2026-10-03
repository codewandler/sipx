//! ABI error codes (`docs/specs/browser-sdk.md` §4.10).
//!
//! These report **host-contract violations**, never protocol outcomes. Malformed SIP arriving in
//! [`crate::Abi::input_bytes`] returns `0`: hostile network input is a value, handled inside the
//! kernel with typed errors and counters, exactly as the native stack handles it. A SIP request
//! that fails is reported through an event, not through a return code.

macro_rules! define_errors {
    ($($name:ident = $code:expr => $token:literal;)*) => {
        /// Stable host-contract violation codes from the browser wire declaration.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
        #[repr(i32)]
        #[non_exhaustive]
        pub enum Error { $(#[doc = $token] $name = $code,)* }
        impl Error {
            /// Stable wire token, safe for diagnostics.
            #[must_use]
            pub fn token(self) -> &'static str { match self { $(Self::$name => $token,)* } }
            pub(crate) const ALL: [Self; 12] = [$(Self::$name,)*];
        }
    };
}
browser_errors!(define_errors);

impl Error {
    /// The wire value: the negative integer an entry point returns.
    #[must_use]
    pub fn code(self) -> i32 {
        self as i32
    }

    /// The magnitude, for the packed-buffer error encoding in §4.2.
    #[must_use]
    pub fn magnitude(self) -> u32 {
        self.code().unsigned_abs()
    }
}

/// The result of an entry point that reports through §4.10.
pub type Result<T> = core::result::Result<T, Error>;
