//! Cluster API fallback for names ordinary SIP resolution could not answer.
//!
//! This stays in the command crate because reading kubeconfig and making HTTPS requests are
//! deployment I/O. The transport resolver remains ordinary RFC 3263 resolution.

use std::collections::HashSet;
use std::future::Future;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use serde::Deserialize;
use sipx_sip::{Host, Uri, UriTransport};
use sipx_transport::destination::Error;
use sipx_transport::dns::ResolutionError as DnsError;
use sipx_transport::{Target, TransportKind};

const MAX_RESPONSE: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClusterName {
    service: String,
    namespace: String,
}

/// Let a cluster API supply targets only after an authoritative empty ordinary resolution.
pub(crate) async fn after_ordinary(
    ordinary: Result<Vec<Target>, Error>,
    uri: &Uri,
    next_hop: Option<&str>,
    transport: Option<TransportKind>,
    suffix: &str,
    budget: Option<Duration>,
) -> Result<Vec<Target>, Error> {
    after_with(
        ordinary,
        uri,
        next_hop,
        transport,
        suffix,
        |name, kind| async move {
            if budget.is_some_and(|duration| duration.is_zero()) {
                Err(Error::Resolution(DnsError::ResolutionTimeout))
            } else {
                api_targets(&name, kind, budget).await
            }
        },
    )
    .await
}

async fn after_with<F, Fut>(
    ordinary: Result<Vec<Target>, Error>,
    uri: &Uri,
    next_hop: Option<&str>,
    transport: Option<TransportKind>,
    suffix: &str,
    fetch: F,
) -> Result<Vec<Target>, Error>
where
    F: FnOnce(ClusterName, TransportKind) -> Fut,
    Fut: Future<Output = Result<Vec<Target>, Error>>,
{
    match ordinary {
        Ok(candidates) => Ok(candidates),
        Err(error) => {
            if next_hop.is_some() || !is_no_candidate(&error) {
                return Err(error);
            }
            let Some(name) = cluster_name(uri, suffix) else {
                return Err(error);
            };
            let kind = selected_transport(uri, transport)?;
            fetch(name, kind).await
        }
    }
}

fn is_no_candidate(error: &Error) -> bool {
    matches!(
        error,
        Error::Resolution(DnsError::Selection(
            sipx_transport::ResolutionError::NoUsableCandidate { .. }
        ))
    )
}

fn cluster_name(uri: &Uri, suffix: &str) -> Option<ClusterName> {
    let Host::Name(host) = uri.host()? else {
        return None;
    };
    let host = String::from_utf8_lossy(host.as_bytes())
        .trim_end_matches('.')
        .to_ascii_lowercase();
    let suffix = suffix.trim_end_matches('.').to_ascii_lowercase();
    let labels = host.split('.').collect::<Vec<_>>();
    let suffix_labels = suffix.split('.').collect::<Vec<_>>();
    if suffix.is_empty()
        || suffix_labels.iter().any(|label| label.is_empty())
        || labels.len() != suffix_labels.len().saturating_add(3)
        || labels.get(2).copied() != Some("svc")
        || labels.get(3..) != Some(suffix_labels.as_slice())
    {
        return None;
    }
    let service = labels.first()?.to_string();
    let namespace = labels.get(1)?.to_string();
    if service.is_empty() || namespace.is_empty() {
        return None;
    }
    Some(ClusterName { service, namespace })
}

fn selected_transport(uri: &Uri, requested: Option<TransportKind>) -> Result<TransportKind, Error> {
    if let Some(requested) = requested {
        return Ok(requested);
    }
    match uri.selected_transport() {
        Ok(UriTransport::Udp) => Ok(TransportKind::Udp),
        Ok(UriTransport::Tcp) => Ok(TransportKind::Tcp),
        Ok(UriTransport::Tls) => Ok(TransportKind::Tls),
        Ok(UriTransport::Ws) => Ok(TransportKind::Ws),
        Ok(UriTransport::Wss) => Ok(TransportKind::Wss),
        Ok(UriTransport::Quic) => Ok(TransportKind::Quic),
        Ok(_) => Err(Error::Input {
            message: "unsupported transport for cluster target".to_owned(),
        }),
        Err(error) => Err(Error::Input {
            message: format!("unsupported transport for cluster target: {error}"),
        }),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
struct Kubeconfig {
    current_context: String,
    #[serde(default)]
    contexts: Vec<NamedContext>,
    #[serde(default)]
    clusters: Vec<NamedCluster>,
    #[serde(default)]
    users: Vec<NamedUser>,
}

#[derive(Deserialize)]
struct NamedContext {
    name: String,
    context: Context,
}

#[derive(Deserialize)]
struct Context {
    cluster: String,
    user: String,
}

#[derive(Deserialize)]
struct NamedCluster {
    name: String,
    cluster: Cluster,
}

#[derive(Deserialize)]
#[serde(rename_all = "kebab-case")]
struct Cluster {
    server: String,
    certificate_authority: Option<PathBuf>,
    certificate_authority_data: Option<String>,
}

#[derive(Deserialize)]
struct NamedUser {
    name: String,
    user: User,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct User {
    token: Option<String>,
    token_file: Option<PathBuf>,
}

struct Access {
    server: String,
    token: String,
    authority: Option<Vec<u8>>,
}

impl Kubeconfig {
    fn access(self, directory: &Path) -> Result<Access, Error> {
        let context = self
            .contexts
            .into_iter()
            .find(|entry| entry.name == self.current_context)
            .ok_or_else(|| cluster_error("kubeconfig current context was not found"))?
            .context;
        let cluster = self
            .clusters
            .into_iter()
            .find(|entry| entry.name == context.cluster)
            .ok_or_else(|| cluster_error("kubeconfig current cluster was not found"))?
            .cluster;
        let user = self
            .users
            .into_iter()
            .find(|entry| entry.name == context.user)
            .ok_or_else(|| cluster_error("kubeconfig current user was not found"))?
            .user;
        let token = match (user.token, user.token_file) {
            (Some(token), _) if !token.is_empty() => token,
            (_, Some(path)) => std::fs::read_to_string(relative(directory, &path))
                .map_err(|source| cluster_error(&format!("cannot read bearer token: {source}")))?
                .trim()
                .to_owned(),
            _ => return Err(cluster_error("kubeconfig current user has no bearer token")),
        };
        if token.is_empty() {
            return Err(cluster_error("kubeconfig bearer token is empty"));
        }
        let authority = match (
            cluster.certificate_authority_data,
            cluster.certificate_authority,
        ) {
            (Some(data), _) => Some(
                base64::engine::general_purpose::STANDARD
                    .decode(data.trim())
                    .map_err(|_| {
                        cluster_error("kubeconfig certificate authority data is invalid")
                    })?,
            ),
            (_, Some(path)) => {
                Some(std::fs::read(relative(directory, &path)).map_err(|source| {
                    cluster_error(&format!("cannot read certificate authority: {source}"))
                })?)
            }
            _ => None,
        };
        Ok(Access {
            server: cluster.server,
            token,
            authority,
        })
    }
}

fn relative(directory: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        directory.join(path)
    }
}

fn kubeconfig_path() -> Result<PathBuf, Error> {
    if let Some(paths) = std::env::var_os("KUBECONFIG") {
        return std::env::split_paths(&paths)
            .next()
            .filter(|path| !path.as_os_str().is_empty())
            .ok_or_else(|| cluster_error("KUBECONFIG names no file"));
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| cluster_error("HOME is unavailable and KUBECONFIG is not set"))?;
    Ok(home.join(".kube").join("config"))
}

async fn api_targets(
    name: &ClusterName,
    transport: TransportKind,
    budget: Option<Duration>,
) -> Result<Vec<Target>, Error> {
    let path = kubeconfig_path()?;
    let bytes = std::fs::read(&path)
        .map_err(|source| cluster_error(&format!("cannot read {}: {source}", path.display())))?;
    let config: Kubeconfig = serde_saphyr::from_slice(&bytes)
        .map_err(|source| cluster_error(&format!("cannot parse {}: {source}", path.display())))?;
    let directory = path.parent().unwrap_or_else(|| Path::new("."));
    let access = config.access(directory)?;
    let mut client = reqwest::Client::builder()
        .timeout(budget.unwrap_or(Duration::from_secs(8)))
        // One dial performs one credential-bearing request. A redirect is another authority, not
        // an EndpointSlice answer, and must never receive the kubeconfig bearer token.
        .redirect(reqwest::redirect::Policy::none());
    if let Some(authority) = access.authority {
        let certificates = reqwest::Certificate::from_pem_bundle(&authority)
            .map_err(|source| cluster_error(&format!("invalid certificate authority: {source}")))?;
        for certificate in certificates {
            client = client.add_root_certificate(certificate);
        }
    }
    let client = client
        .build()
        .map_err(|source| cluster_error(&format!("cannot build cluster HTTPS client: {source}")))?;
    let url = endpoint_slice_url(&access.server, name)?;
    let mut response = client
        .get(url)
        .bearer_auth(access.token)
        .send()
        .await
        .map_err(|source| cluster_error(&format!("cluster API request failed: {source}")))?;
    if !response.status().is_success() {
        return Err(cluster_error(&format!(
            "cluster API returned HTTP {}",
            response.status()
        )));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|source| cluster_error(&format!("cluster API response failed: {source}")))?
    {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE {
            return Err(cluster_error("cluster API response exceeds 1 MiB"));
        }
        body.extend_from_slice(&chunk);
    }
    let slices: EndpointSliceList = serde_json::from_slice(&body)
        .map_err(|source| cluster_error(&format!("invalid EndpointSlice response: {source}")))?;
    let candidates = slices.targets(transport);
    if candidates.is_empty() {
        return Err(cluster_error("cluster API returned no ready SIP endpoint"));
    }
    Ok(candidates)
}

fn endpoint_slice_url(server: &str, name: &ClusterName) -> Result<reqwest::Url, Error> {
    let mut url = reqwest::Url::parse(server)
        .map_err(|source| cluster_error(&format!("invalid cluster API URL: {source}")))?;
    if url.scheme() != "https" {
        return Err(cluster_error("cluster API URL must use HTTPS"));
    }
    if !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(cluster_error(
            "cluster API URL must not contain credentials, a query, or a fragment",
        ));
    }
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|()| cluster_error("cluster API URL cannot be a path base"))?;
        segments.pop_if_empty().extend([
            "apis",
            "discovery.k8s.io",
            "v1",
            "namespaces",
            &name.namespace,
            "endpointslices",
        ]);
    }
    url.query_pairs_mut().append_pair(
        "labelSelector",
        &format!("kubernetes.io/service-name={}", name.service),
    );
    Ok(url)
}

fn cluster_error(message: &str) -> Error {
    Error::Setup {
        message: format!("cluster resolution failed: {message}"),
    }
}

#[derive(Debug, Deserialize)]
struct EndpointSliceList {
    #[serde(default)]
    items: Vec<EndpointSlice>,
}

#[derive(Debug, Deserialize)]
struct EndpointSlice {
    #[serde(default)]
    endpoints: Vec<Endpoint>,
    #[serde(default)]
    ports: Vec<EndpointPort>,
}

#[derive(Debug, Deserialize)]
struct Endpoint {
    #[serde(default)]
    addresses: Vec<String>,
    #[serde(default)]
    conditions: Conditions,
}

#[derive(Debug, Default, Deserialize)]
struct Conditions {
    ready: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct EndpointPort {
    protocol: Option<String>,
    port: Option<u16>,
}

impl EndpointSliceList {
    fn targets(self, transport: TransportKind) -> Vec<Target> {
        let mut seen = HashSet::new();
        let mut targets = Vec::new();
        for slice in self.items {
            let Some(port) = endpoint_port(&slice.ports, transport) else {
                continue;
            };
            for endpoint in slice.endpoints {
                if endpoint.conditions.ready == Some(false) {
                    continue;
                }
                for address in endpoint.addresses {
                    let Ok(address) = address.parse::<IpAddr>() else {
                        continue;
                    };
                    let socket = SocketAddr::new(address, port);
                    if !seen.insert(socket) {
                        continue;
                    }
                    targets.push(Target::new(socket, transport));
                }
            }
        }
        targets
    }
}

fn endpoint_port(ports: &[EndpointPort], transport: TransportKind) -> Option<u16> {
    if ports.is_empty() {
        return Some(transport.default_port());
    }
    let port = ports
        .iter()
        .find(|port| protocol_matches(port.protocol.as_deref(), transport))?;
    match port.port {
        Some(0) => None,
        Some(port) => Some(port),
        None => Some(transport.default_port()),
    }
}

fn protocol_matches(protocol: Option<&str>, transport: TransportKind) -> bool {
    let expected = match transport {
        TransportKind::Udp | TransportKind::Quic => Some("UDP"),
        TransportKind::Tcp | TransportKind::Tls | TransportKind::Ws | TransportKind::Wss => {
            Some("TCP")
        }
        _ => None,
    };
    expected.is_some_and(|expected| {
        protocol.map_or(expected == "TCP", |protocol| {
            protocol.eq_ignore_ascii_case(expected)
        })
    })
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn uri(value: &str) -> Uri {
        Uri::parse(bytes::Bytes::copy_from_slice(value.as_bytes())).expect("valid URI")
    }

    fn no_candidate(authority: &str) -> Error {
        Error::Resolution(DnsError::Selection(
            sipx_transport::ResolutionError::NoUsableCandidate {
                authority: authority.to_owned(),
            },
        ))
    }

    #[tokio::test]
    async fn an_ordinary_answer_never_reaches_the_cluster_api() {
        let expected = Target::udp("192.0.2.10:5060".parse().unwrap());
        let actual = after_with(
            Ok(vec![expected.clone()]),
            &uri("sip:echo@echo.voice.svc.cluster.local"),
            None,
            None,
            "cluster.local",
            |_name, _transport| async { panic!("cluster API must not be called") },
        )
        .await
        .unwrap();
        assert_eq!(actual, vec![expected]);
    }

    #[tokio::test]
    async fn an_exhausted_cluster_budget_does_not_discard_an_ordinary_answer() {
        let expected = Target::udp("192.0.2.10:5060".parse().unwrap());
        let actual = after_ordinary(
            Ok(vec![expected.clone()]),
            &uri("sip:echo@echo.voice.svc.cluster.local"),
            None,
            None,
            "cluster.local",
            Some(Duration::ZERO),
        )
        .await
        .unwrap();
        assert_eq!(actual, vec![expected]);
    }

    #[tokio::test]
    async fn a_two_label_short_name_never_reaches_the_cluster_api() {
        let original = no_candidate("echo.voice");
        let error = after_with(
            Err(original.clone()),
            &uri("sip:echo@echo.voice"),
            None,
            None,
            "cluster.local",
            |_name, _transport| async { panic!("cluster API must not be called") },
        )
        .await
        .unwrap_err();
        assert_eq!(error, original);
    }

    #[tokio::test]
    async fn a_matching_name_uses_the_fallback_after_no_dns_candidate() {
        let actual = after_with(
            Err(no_candidate("echo.voice.svc.cluster.local")),
            &uri("sip:echo@echo.voice.svc.cluster.local"),
            None,
            None,
            "cluster.local",
            |name, transport| async move {
                assert_eq!(
                    name,
                    ClusterName {
                        service: "echo".to_owned(),
                        namespace: "voice".to_owned(),
                    }
                );
                assert_eq!(transport, TransportKind::Udp);
                Ok(vec![Target::udp("10.20.0.4:5060".parse().unwrap())])
            },
        )
        .await
        .unwrap();
        assert_eq!(actual[0].addr, "10.20.0.4:5060".parse().unwrap());
    }

    #[test]
    fn endpoint_slices_preserve_ready_address_order() {
        let fixture: EndpointSliceList = serde_json::from_str(
            r#"{
              "items": [{
                "ports": [{"name":"sip","protocol":"UDP","port":5070}],
                "endpoints": [
                  {"addresses":["10.20.0.4"],"conditions":{"ready":true}},
                  {"addresses":["10.20.0.5"],"conditions":{"ready":false}},
                  {"addresses":["2001:db8::4","10.20.0.6"],"conditions":{}}
                ]
              }]
            }"#,
        )
        .unwrap();
        let targets = fixture.targets(TransportKind::Udp);
        assert_eq!(
            targets.iter().map(|target| target.addr).collect::<Vec<_>>(),
            vec![
                "10.20.0.4:5070".parse().unwrap(),
                "[2001:db8::4]:5070".parse().unwrap(),
                "10.20.0.6:5070".parse().unwrap(),
            ]
        );
    }

    #[test]
    fn endpoint_slice_ports_follow_the_selected_transport() {
        let no_ports = || {
            serde_json::from_str::<EndpointSliceList>(
                r#"{"items":[{"endpoints":[{"addresses":["10.20.0.4"]}]}]}"#,
            )
            .unwrap()
        };
        assert_eq!(
            no_ports().targets(TransportKind::Ws)[0].addr.port(),
            TransportKind::Ws.default_port()
        );
        assert_eq!(
            no_ports().targets(TransportKind::Wss)[0].addr.port(),
            TransportKind::Wss.default_port()
        );

        let tcp_only: EndpointSliceList = serde_json::from_str(
            r#"{
              "items": [{
                "ports": [{"protocol":"TCP","port":5070}],
                "endpoints": [{"addresses":["10.20.0.4"]}]
              }]
            }"#,
        )
        .unwrap();
        assert!(tcp_only.targets(TransportKind::Udp).is_empty());
    }

    #[test]
    fn endpoint_slice_url_preserves_the_api_prefix_and_encodes_the_selector() {
        let url = endpoint_slice_url(
            "https://api.example.test/proxy/",
            &ClusterName {
                service: "echo".to_owned(),
                namespace: "voice".to_owned(),
            },
        )
        .unwrap();
        assert_eq!(
            url.as_str(),
            concat!(
                "https://api.example.test/proxy/apis/discovery.k8s.io/v1/namespaces/voice/",
                "endpointslices?labelSelector=kubernetes.io%2Fservice-name%3Decho"
            )
        );

        for server in [
            "http://api.example.test",
            "https://user@api.example.test",
            "https://api.example.test?other=value",
            "https://api.example.test#fragment",
        ] {
            assert!(
                endpoint_slice_url(
                    server,
                    &ClusterName {
                        service: "echo".to_owned(),
                        namespace: "voice".to_owned(),
                    },
                )
                .is_err(),
                "{server}"
            );
        }
    }

    #[tokio::test]
    async fn a_secure_uri_maps_ws_to_wss_for_the_cluster_candidate() {
        let actual = after_with(
            Err(no_candidate("echo.voice.svc.cluster.local")),
            &uri("sips:echo@echo.voice.svc.cluster.local;transport=ws"),
            None,
            None,
            "cluster.local",
            |_name, transport| async move {
                assert_eq!(transport, TransportKind::Wss);
                Ok(vec![Target::new(
                    "10.20.0.4:443".parse().unwrap(),
                    transport,
                )])
            },
        )
        .await
        .unwrap();
        assert_eq!(actual[0].transport, TransportKind::Wss);
    }

    #[test]
    fn configured_suffix_is_exact_and_case_insensitive() {
        let matched = cluster_name(
            &uri("sip:echo@Echo.Voice.SVC.Internal.Example"),
            "internal.example",
        )
        .unwrap();
        assert_eq!(matched.service, "echo");
        assert_eq!(matched.namespace, "voice");
        assert!(cluster_name(&uri("sip:echo@echo.voice"), "cluster.local").is_none());
        assert!(
            cluster_name(&uri("sip:echo@echo.voice.svc.cluster.local"), "other.local").is_none()
        );
    }
}
