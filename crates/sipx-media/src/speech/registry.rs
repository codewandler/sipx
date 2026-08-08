//! The provider registry (`docs/specs/speech-providers.md` §3).
//!
//! Registration is the only way a provider becomes selectable: nothing is discovered implicitly,
//! and no descriptor arrives because a library happened to be installed.
//!
//! Discovery is a synchronous read that probes nothing. Two properties of §3 shape the storage
//! choice: two consecutive reads of an unchanged registry must be **identical**, and the order of
//! the list must carry **no meaning**. Both are satisfied by keeping the entries ordered by
//! identity rather than by arrival, which is why `DIS-2` — permute the registration order and read
//! again — is a property of this type rather than a discipline callers have to keep.

use super::descriptor::{ProviderDescriptor, ProviderId, ProviderKind};

/// Why a registration was not accepted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum RegistrationError {
    /// §3 makes `id` unique in the registry, so a second descriptor cannot claim one.
    ///
    /// Uniqueness is across the whole registry rather than per kind: an identity that meant a
    /// recogniser in one document and a synthesiser in another would make `UnknownProvider` and
    /// "wrong kind" indistinguishable to a reader of the configuration.
    #[error("a provider is already registered as `{id}`")]
    DuplicateIdentity {
        /// The identity already taken.
        id: ProviderId,
    },
}

/// The providers an endpoint host has explicitly registered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProviderRegistry {
    /// Kept sorted by identity, so discovery is identical across reads and independent of the
    /// order registrations arrived in.
    registered: Vec<ProviderDescriptor>,
}

impl ProviderRegistry {
    /// An empty registry. Nothing is selectable until something is registered.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            registered: Vec::new(),
        }
    }

    /// Register one provider.
    ///
    /// # Errors
    ///
    /// Returns [`RegistrationError::DuplicateIdentity`] when the identity is already taken.
    pub fn register(&mut self, descriptor: ProviderDescriptor) -> Result<(), RegistrationError> {
        match self
            .registered
            .binary_search_by(|entry| entry.id().cmp(descriptor.id()))
        {
            Ok(_) => Err(RegistrationError::DuplicateIdentity {
                id: descriptor.id().clone(),
            }),
            Err(position) => {
                self.registered.insert(position, descriptor);
                Ok(())
            }
        }
    }

    /// The descriptor registered under one identity, whatever its kind.
    #[must_use]
    pub fn descriptor(&self, id: &ProviderId) -> Option<&ProviderDescriptor> {
        self.registered
            .binary_search_by(|entry| entry.id().cmp(id))
            .ok()
            .and_then(|position| self.registered.get(position))
    }

    /// §4 step 1: the descriptor registered under one identity *with the requested kind*.
    ///
    /// A registered identity of the other kind resolves to `None` and therefore refuses as
    /// `UnknownProvider`, which is the step's stated reason — a synthesis document naming a
    /// recogniser has not found a provider it can use.
    #[must_use]
    pub fn resolve(&self, id: &ProviderId, kind: ProviderKind) -> Option<&ProviderDescriptor> {
        self.descriptor(id)
            .filter(|descriptor| descriptor.kind() == kind)
    }

    /// Every registered descriptor.
    ///
    /// Identical across repeated reads of an unchanged registry, and in an order that carries no
    /// meaning: selection never consults it.
    #[must_use]
    pub fn discover(&self) -> &[ProviderDescriptor] {
        &self.registered
    }

    /// How many providers are registered.
    #[must_use]
    pub fn len(&self) -> usize {
        self.registered.len()
    }

    /// Whether nothing is registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.registered.is_empty()
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::speech::descriptor::ProviderDescriptor;

    fn descriptor(token: &str) -> ProviderDescriptor {
        ProviderDescriptor::recognition(ProviderId::new(token).unwrap(), "0").build()
    }

    /// DIS-2: registration order is not discovery order, so permuting it changes nothing.
    #[test]
    fn discovery_is_independent_of_registration_order() {
        let mut forwards = ProviderRegistry::new();
        for token in ["a", "b", "c"] {
            forwards.register(descriptor(token)).unwrap();
        }
        let mut backwards = ProviderRegistry::new();
        for token in ["c", "b", "a"] {
            backwards.register(descriptor(token)).unwrap();
        }
        assert_eq!(forwards.discover(), backwards.discover());
        assert_eq!(forwards.discover(), forwards.discover());
    }

    /// §3: an identity is unique in the registry.
    #[test]
    fn a_second_registration_of_one_identity_is_refused() {
        let mut registry = ProviderRegistry::new();
        registry.register(descriptor("inert")).unwrap();
        assert_eq!(
            registry.register(descriptor("inert")),
            Err(RegistrationError::DuplicateIdentity {
                id: ProviderId::new("inert").unwrap()
            })
        );
        assert_eq!(registry.len(), 1);
    }

    /// §4 step 1: resolution is by identity *and* kind.
    #[test]
    fn resolution_requires_the_requested_kind() {
        let mut registry = ProviderRegistry::new();
        registry.register(descriptor("inert")).unwrap();
        let id = ProviderId::new("inert").unwrap();
        assert!(registry.resolve(&id, ProviderKind::Recognition).is_some());
        assert!(registry.resolve(&id, ProviderKind::Synthesis).is_none());
    }
}
