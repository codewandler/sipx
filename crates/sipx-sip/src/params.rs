//! Ordered parameter lists: the `;name=value` tails on URIs and header values.
//!
//! Order and duplicates are preserved. Forwarding must not reorder a parameter list, and a
//! duplicate is a fact about the message a proxy has no business erasing — the layer that
//! cares can decide what a repeat means.

use bytes::Bytes;

use crate::escape;

/// One `name` or `name=value` parameter.
#[derive(Debug, Clone)]
pub struct Param {
    name: Bytes,
    value: Option<Bytes>,
}

impl Param {
    /// A parameter with no value, as in `;lr`.
    #[must_use]
    pub fn flag(name: impl Into<Bytes>) -> Self {
        Self {
            name: name.into(),
            value: None,
        }
    }

    /// A parameter with a value, as in `;transport=tcp`.
    #[must_use]
    pub fn new(name: impl Into<Bytes>, value: impl Into<Bytes>) -> Self {
        Self {
            name: name.into(),
            value: Some(value.into()),
        }
    }

    /// The parameter name, as it appeared.
    #[must_use]
    pub fn name(&self) -> &[u8] {
        &self.name
    }

    /// The parameter value, as it appeared, still percent-encoded.
    #[must_use]
    pub fn value(&self) -> Option<&[u8]> {
        self.value.as_deref()
    }

    /// Whether this parameter's name matches, case-insensitively.
    ///
    /// Escapes of unreserved characters fold before the comparison: `pname` is built from
    /// `paramchar`, which includes `escaped`, so `%74ransport` is a legal spelling of
    /// `transport` (RFC 3261 §19.1.4).
    #[must_use]
    pub fn has_name(&self, name: &str) -> bool {
        names_equivalent(&self.name, name.as_bytes())
    }
}

/// Whether two parameter names are the same name under RFC 3261 §19.1.4: case-insensitive,
/// with escapes of unreserved characters folded into the characters themselves.
#[must_use]
pub(crate) fn names_equivalent(a: &[u8], b: &[u8]) -> bool {
    escape::eq_ignore_ascii_case(
        &escape::normalize_for_comparison(a),
        &escape::normalize_for_comparison(b),
    )
}

/// An ordered list of parameters.
#[derive(Debug, Clone, Default)]
pub struct Params {
    entries: Vec<Param>,
}

impl Params {
    /// An empty list.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many parameters are present, counting duplicates.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the list is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Append a parameter, keeping any existing one of the same name.
    pub fn push(&mut self, param: Param) {
        self.entries.push(param);
    }

    /// Remove every parameter with this name, and say whether any was there.
    ///
    /// Every one, not the first: the list preserves duplicates because a message may contain them,
    /// and leaving a second copy behind after removing the first would be a silent change of
    /// meaning rather than a removal.
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|p| !p.has_name(name));
        self.entries.len() != before
    }

    /// Every parameter, in wire order.
    pub fn iter(&self) -> impl Iterator<Item = &Param> {
        self.entries.iter()
    }

    /// The first parameter with this name, if any. Names compare case-insensitively.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Param> {
        self.entries.iter().find(|p| p.has_name(name))
    }

    /// The value of the first parameter with this name.
    ///
    /// Returns `None` both when the parameter is absent and when it is present without a
    /// value; use [`Params::contains`] to tell those apart, because `;lr` and no `lr` at all
    /// mean different things.
    #[must_use]
    pub fn value(&self, name: &str) -> Option<&[u8]> {
        self.get(name).and_then(Param::value)
    }

    /// Whether a parameter with this name is present, with or without a value.
    #[must_use]
    pub fn contains(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// Whether two parameter values are equivalent under RFC 3261 §19.1.4: order is
    /// insignificant, comparison is case-insensitive, and escapes of unreserved characters
    /// fold into the characters themselves.
    #[must_use]
    fn values_equivalent(a: Option<&[u8]>, b: Option<&[u8]>) -> bool {
        match (a, b) {
            (None, None) => true,
            (Some(x), Some(y)) => escape::eq_ignore_ascii_case(
                &escape::normalize_for_comparison(x),
                &escape::normalize_for_comparison(y),
            ),
            _ => false,
        }
    }

    /// Whether a named parameter is equivalent in both lists, treating absence as a value.
    #[must_use]
    pub(crate) fn param_equivalent(&self, other: &Self, name: &str) -> bool {
        match (self.get(name), other.get(name)) {
            (None, None) => true,
            (Some(x), Some(y)) => Self::values_equivalent(x.value(), y.value()),
            _ => false,
        }
    }

    /// Whether every parameter present in *both* lists agrees.
    ///
    /// Parameters present in only one are ignored — the caller applies the special cases
    /// (RFC 3261 §19.1.4 names `user`, `ttl`, `method`, `maddr` and `transport`) with
    /// [`Params::param_equivalent`].
    #[must_use]
    pub(crate) fn common_params_agree(&self, other: &Self) -> bool {
        self.entries.iter().all(|p| {
            other
                .entries
                .iter()
                .filter(|q| names_equivalent(q.name(), p.name()))
                .all(|q| Self::values_equivalent(p.value(), q.value()))
        })
    }

    /// Serialize as `;name=value` pairs, in order.
    pub fn write_to(&self, out: &mut Vec<u8>, separator: u8) {
        for (i, p) in self.entries.iter().enumerate() {
            out.push(if i == 0 && separator == b'?' {
                b'?'
            } else if separator == b'?' {
                b'&'
            } else {
                separator
            });
            out.extend_from_slice(&p.name);
            if let Some(v) = &p.value {
                out.push(b'=');
                out.extend_from_slice(v);
            }
        }
    }
}

impl<'a> IntoIterator for &'a Params {
    type Item = &'a Param;
    type IntoIter = std::slice::Iter<'a, Param>;

    fn into_iter(self) -> Self::IntoIter {
        self.entries.iter()
    }
}

impl FromIterator<Param> for Params {
    fn from_iter<T: IntoIterator<Item = Param>>(iter: T) -> Self {
        Self {
            entries: iter.into_iter().collect(),
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn params(pairs: &[(&str, Option<&str>)]) -> Params {
        pairs
            .iter()
            .map(|(n, v)| match v {
                Some(v) => Param::new(Bytes::from((*n).to_owned()), Bytes::from((*v).to_owned())),
                None => Param::flag(Bytes::from((*n).to_owned())),
            })
            .collect()
    }

    #[test]
    fn preserves_order_and_duplicates() {
        let p = params(&[("b", Some("2")), ("a", Some("1")), ("b", Some("3"))]);
        assert_eq!(p.len(), 3);
        let names: Vec<_> = p.iter().map(|x| x.name().to_vec()).collect();
        assert_eq!(names, vec![b"b".to_vec(), b"a".to_vec(), b"b".to_vec()]);
        // get() returns the first occurrence, not the last.
        assert_eq!(p.value("b"), Some(&b"2"[..]));
    }

    #[test]
    fn flag_is_not_the_same_as_absent() {
        let p = params(&[("lr", None)]);
        assert!(p.contains("lr"));
        assert_eq!(p.value("lr"), None);
        assert!(!p.contains("nope"));
    }

    #[test]
    fn names_and_values_compare_case_insensitively() {
        let a = params(&[("Transport", Some("TCP"))]);
        let b = params(&[("transport", Some("tcp"))]);
        assert!(a.param_equivalent(&b, "transport"));
    }

    /// RFC 3261 §19.1.4: `pname` includes `escaped`, so an escaped spelling of a name is
    /// the same name.
    #[test]
    fn escaped_names_fold_when_looked_up() {
        let p = params(&[("%74ransport", Some("udp"))]);
        assert!(p.get("transport").is_some());
        assert_eq!(p.value("transport"), Some(&b"udp"[..]));

        let plain = params(&[("transport", Some("udp"))]);
        assert!(p.param_equivalent(&plain, "transport"));
        assert!(p.common_params_agree(&plain) && plain.common_params_agree(&p));

        let other = params(&[("transport", Some("tcp"))]);
        assert!(!p.common_params_agree(&other));
    }

    #[test]
    fn writes_uri_and_header_separators() {
        let p = params(&[("a", Some("1")), ("b", None)]);
        let mut out = Vec::new();
        p.write_to(&mut out, b';');
        assert_eq!(out, b";a=1;b");

        let mut out = Vec::new();
        p.write_to(&mut out, b'?');
        assert_eq!(out, b"?a=1&b");
    }
}
