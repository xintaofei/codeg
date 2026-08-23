use std::collections::{HashMap, VecDeque};
use std::fs;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};
use url::Url;

use super::{validate_connection_id, NativeBrowserError, INITIAL_URL};

const MAX_INTERCEPTED_REQUESTS: usize = 4_096;
const MAX_REQUEST_ID_BYTES: usize = 256;
const MAX_HEADER_NAME_BYTES: usize = 256;
const MAX_HEADER_VALUE_BYTES: usize = 64 * 1024;
const DNS_VALIDATION_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(super) struct NativeRequestKey {
    connection_id: String,
    tab_id: String,
    generation: u64,
    request_id: String,
}

impl NativeRequestKey {
    pub(super) fn new(
        connection_id: &str,
        tab_id: &str,
        generation: u64,
        request_id: &str,
    ) -> Self {
        Self {
            connection_id: connection_id.to_string(),
            tab_id: tab_id.to_string(),
            generation,
            request_id: request_id.to_string(),
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct RequestUrlTracker {
    urls: HashMap<NativeRequestKey, String>,
    order: VecDeque<NativeRequestKey>,
}

impl RequestUrlTracker {
    pub(super) fn get(&self, key: &NativeRequestKey) -> Option<&str> {
        self.urls.get(key).map(String::as_str)
    }

    pub(super) fn len(&self) -> usize {
        self.urls.len()
    }

    pub(super) fn remember(&mut self, key: NativeRequestKey, url: String) {
        if !self.urls.contains_key(&key) {
            self.order.push_back(key.clone());
        }
        self.urls.insert(key, url);
        while self.order.len() > MAX_INTERCEPTED_REQUESTS {
            if let Some(expired) = self.order.pop_front() {
                self.urls.remove(&expired);
            }
        }
    }

    pub(super) fn remove_tab(&mut self, connection_id: &str, tab_id: &str) {
        self.urls
            .retain(|key, _| key.connection_id != connection_id || key.tab_id != tab_id);
        self.order
            .retain(|key| key.connection_id != connection_id || key.tab_id != tab_id);
    }

    pub(super) fn remove_session(&mut self, connection_id: &str) {
        self.urls
            .retain(|key, _| key.connection_id != connection_id);
        self.order.retain(|key| key.connection_id != connection_id);
    }

    pub(super) fn clear(&mut self) {
        self.urls.clear();
        self.order.clear();
    }
}

#[derive(Debug, Clone)]
struct ParsedNetworkTarget {
    canonical_url: String,
    scheme: String,
    hostname: String,
    port: u16,
    literal_address: Option<IpAddr>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeIpClassification {
    Public,
    Private,
    Special,
}

pub(super) fn validate_navigation_url(value: &str) -> Result<String, NativeBrowserError> {
    if value == INITIAL_URL {
        return Ok(INITIAL_URL.to_string());
    }
    let target = parse_network_target(value)?;
    if target
        .literal_address
        .is_some_and(|address| classify_ip_address(address) != NativeIpClassification::Public)
    {
        return Err(NativeBrowserError::NavigationBlocked);
    }
    Ok(target.canonical_url)
}

pub(super) async fn validate_network_request_url(
    value: &str,
) -> Result<String, NativeBrowserError> {
    let target = parse_network_target(value)?;
    if let Some(address) = target.literal_address {
        return (classify_ip_address(address) == NativeIpClassification::Public)
            .then_some(target.canonical_url)
            .ok_or(NativeBrowserError::NavigationBlocked);
    }

    let answers = tokio::time::timeout(
        DNS_VALIDATION_TIMEOUT,
        tokio::net::lookup_host((target.hostname.as_str(), target.port)),
    )
    .await
    .map_err(|_| NativeBrowserError::NavigationBlocked)?
    .map_err(|_| NativeBrowserError::NavigationBlocked)?;
    let addresses = answers.map(|answer| answer.ip()).collect::<Vec<_>>();
    addresses_are_public(&addresses)
        .then_some(target.canonical_url)
        .ok_or(NativeBrowserError::NavigationBlocked)
}

pub(super) fn sanitize_redirect_headers(
    previous_url: &str,
    next_url: &str,
    headers: &HashMap<String, String>,
) -> Result<Option<Vec<Value>>, NativeBrowserError> {
    if request_origin(previous_url)? == request_origin(next_url)? {
        return Ok(None);
    }
    let mut headers = headers
        .iter()
        .filter(|(name, _)| {
            !matches!(
                name.to_ascii_lowercase().as_str(),
                "authorization" | "proxy-authorization" | "cookie" | "x-api-key"
            )
        })
        .map(|(name, value)| {
            if name.is_empty()
                || name.len() > MAX_HEADER_NAME_BYTES
                || value.len() > MAX_HEADER_VALUE_BYTES
                || name.contains('\r')
                || name.contains('\n')
                || value.contains('\r')
                || value.contains('\n')
            {
                return Err(NativeBrowserError::NavigationBlocked);
            }
            Ok(json!({ "name": name, "value": value }))
        })
        .collect::<Result<Vec<_>, _>>()?;
    headers.sort_by(|left, right| {
        left["name"]
            .as_str()
            .unwrap_or_default()
            .cmp(right["name"].as_str().unwrap_or_default())
    });
    Ok(Some(headers))
}

pub(super) fn valid_request_id(request_id: &str) -> bool {
    !request_id.is_empty() && request_id.len() <= MAX_REQUEST_ID_BYTES
}

pub(super) fn intercepted_request_id(raw: &str, max_event_bytes: usize) -> Option<String> {
    if raw.len() > max_event_bytes {
        return None;
    }
    serde_json::from_str::<Value>(raw)
        .ok()?
        .get("requestId")?
        .as_str()
        .filter(|value| valid_request_id(value))
        .map(str::to_string)
}

pub(super) fn managed_download_path_for_root(
    download_dir: &Path,
    connection_id: &str,
    proposed: &Path,
) -> Result<PathBuf, NativeBrowserError> {
    validate_connection_id(connection_id)?;
    fs::create_dir_all(download_dir).map_err(|_| NativeBrowserError::ControllerFailed)?;
    let root = fs::canonicalize(download_dir).map_err(|_| NativeBrowserError::ControllerFailed)?;
    let session_dir = download_dir.join(connection_id);
    fs::create_dir_all(&session_dir).map_err(|_| NativeBrowserError::ControllerFailed)?;
    let canonical_session =
        fs::canonicalize(&session_dir).map_err(|_| NativeBrowserError::ControllerFailed)?;
    if !canonical_session.starts_with(&root) {
        return Err(NativeBrowserError::ControllerFailed);
    }
    let extension = proposed
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 10
                && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
        });
    let mut name = uuid::Uuid::new_v4().simple().to_string();
    if let Some(extension) = extension {
        name.push('.');
        name.push_str(extension);
    }
    Ok(canonical_session.join(name))
}

pub(super) fn completed_download_path_for_root(
    download_dir: &Path,
    connection_id: &str,
    candidate: &Path,
) -> Result<String, NativeBrowserError> {
    validate_connection_id(connection_id)?;
    let root = fs::canonicalize(download_dir).map_err(|_| NativeBrowserError::ControllerFailed)?;
    let session = fs::canonicalize(download_dir.join(connection_id))
        .map_err(|_| NativeBrowserError::ControllerFailed)?;
    let completed =
        fs::canonicalize(candidate).map_err(|_| NativeBrowserError::ControllerFailed)?;
    if !session.starts_with(&root) || !completed.starts_with(&session) {
        return Err(NativeBrowserError::ControllerFailed);
    }
    Ok(completed.to_string_lossy().into_owned())
}

fn parse_network_target(value: &str) -> Result<ParsedNetworkTarget, NativeBrowserError> {
    let url = value
        .parse::<Url>()
        .map_err(|_| NativeBrowserError::NavigationBlocked)?;
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(NativeBrowserError::NavigationBlocked);
    }
    let hostname = url
        .host_str()
        .map(|value| value.trim_matches(['[', ']']).to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .ok_or(NativeBrowserError::NavigationBlocked)?;
    if hostname_is_locally_scoped(&hostname) {
        return Err(NativeBrowserError::NavigationBlocked);
    }
    let port = url
        .port_or_known_default()
        .ok_or(NativeBrowserError::NavigationBlocked)?;
    let literal_address = hostname.parse::<IpAddr>().ok();
    Ok(ParsedNetworkTarget {
        canonical_url: url.to_string(),
        scheme: url.scheme().to_string(),
        hostname,
        port,
        literal_address,
    })
}

fn request_origin(value: &str) -> Result<(String, String, u16), NativeBrowserError> {
    let target = parse_network_target(value)?;
    Ok((target.scheme, target.hostname, target.port))
}

fn hostname_is_locally_scoped(hostname: &str) -> bool {
    let normalized = hostname.to_ascii_lowercase();
    let normalized = normalized.strip_suffix('.').unwrap_or(&normalized);
    normalized == "localhost"
        || normalized.ends_with(".localhost")
        || normalized.ends_with(".local")
        || normalized.ends_with(".internal")
        || normalized.ends_with(".home.arpa")
}

fn addresses_are_public(addresses: &[IpAddr]) -> bool {
    !addresses.is_empty()
        && addresses
            .iter()
            .all(|address| classify_ip_address(*address) == NativeIpClassification::Public)
}

fn classify_ip_address(address: IpAddr) -> NativeIpClassification {
    match address {
        IpAddr::V4(address) => classify_ipv4_address(address),
        IpAddr::V6(address) => address
            .to_ipv4_mapped()
            .map(classify_ipv4_address)
            .unwrap_or_else(|| classify_ipv6_address(address)),
    }
}

fn classify_ipv4_address(address: Ipv4Addr) -> NativeIpClassification {
    const PRIVATE: &[([u8; 4], u8)] = &[
        ([169, 254, 0, 0], 16),
        ([127, 0, 0, 0], 8),
        ([100, 64, 0, 0], 10),
        ([10, 0, 0, 0], 8),
        ([172, 16, 0, 0], 12),
        ([192, 168, 0, 0], 16),
    ];
    const SPECIAL: &[([u8; 4], u8)] = &[
        ([0, 0, 0, 0], 8),
        ([255, 255, 255, 255], 32),
        ([224, 0, 0, 0], 4),
        ([192, 0, 0, 0], 24),
        ([192, 0, 2, 0], 24),
        ([192, 88, 99, 0], 24),
        ([198, 18, 0, 0], 15),
        ([198, 51, 100, 0], 24),
        ([203, 0, 113, 0], 24),
        ([240, 0, 0, 0], 4),
        ([192, 175, 48, 0], 24),
        ([192, 31, 196, 0], 24),
        ([192, 52, 193, 0], 24),
    ];
    if PRIVATE
        .iter()
        .any(|(network, prefix)| ipv4_in_prefix(address, *network, *prefix))
    {
        NativeIpClassification::Private
    } else if SPECIAL
        .iter()
        .any(|(network, prefix)| ipv4_in_prefix(address, *network, *prefix))
    {
        NativeIpClassification::Special
    } else {
        NativeIpClassification::Public
    }
}

fn classify_ipv6_address(address: Ipv6Addr) -> NativeIpClassification {
    const PRIVATE: &[(Ipv6Addr, u8)] = &[
        (Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0), 10),
        (Ipv6Addr::LOCALHOST, 128),
        (Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 0), 7),
    ];
    const SPECIAL: &[(Ipv6Addr, u8)] = &[
        (Ipv6Addr::UNSPECIFIED, 128),
        (Ipv6Addr::new(0xff00, 0, 0, 0, 0, 0, 0, 0), 8),
        (Ipv6Addr::new(0xfec0, 0, 0, 0, 0, 0, 0, 0), 10),
        (Ipv6Addr::new(0x0100, 0, 0, 0, 0, 0, 0, 0), 64),
        (Ipv6Addr::new(0, 0, 0, 0, 0xffff, 0, 0, 0), 96),
        (Ipv6Addr::new(0x0064, 0xff9b, 0, 0, 0, 0, 0, 0), 96),
        (Ipv6Addr::new(0x0064, 0xff9b, 1, 0, 0, 0, 0, 0), 48),
        (Ipv6Addr::new(0x2002, 0, 0, 0, 0, 0, 0, 0), 16),
        (Ipv6Addr::new(0x2001, 0, 0, 0, 0, 0, 0, 0), 32),
        (Ipv6Addr::new(0x2001, 2, 0, 0, 0, 0, 0, 0), 48),
        (Ipv6Addr::new(0x2001, 3, 0, 0, 0, 0, 0, 0), 32),
        (Ipv6Addr::new(0x2001, 4, 0x0112, 0, 0, 0, 0, 0), 48),
        (Ipv6Addr::new(0x2620, 0x004f, 0x8000, 0, 0, 0, 0, 0), 48),
        (Ipv6Addr::new(0x2001, 0x0010, 0, 0, 0, 0, 0, 0), 28),
        (Ipv6Addr::new(0x2001, 0x0020, 0, 0, 0, 0, 0, 0), 28),
        (Ipv6Addr::new(0x2001, 0x0030, 0, 0, 0, 0, 0, 0), 28),
        (Ipv6Addr::new(0x5f00, 0, 0, 0, 0, 0, 0, 0), 16),
        (Ipv6Addr::new(0x2001, 0, 0, 0, 0, 0, 0, 0), 23),
        (Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 0), 32),
        (Ipv6Addr::new(0x3fff, 0, 0, 0, 0, 0, 0, 0), 20),
    ];
    if PRIVATE
        .iter()
        .any(|(network, prefix)| ipv6_in_prefix(address, *network, *prefix))
    {
        NativeIpClassification::Private
    } else if SPECIAL
        .iter()
        .any(|(network, prefix)| ipv6_in_prefix(address, *network, *prefix))
    {
        NativeIpClassification::Special
    } else {
        NativeIpClassification::Public
    }
}

fn ipv4_in_prefix(address: Ipv4Addr, network: [u8; 4], prefix: u8) -> bool {
    let address = u32::from(address);
    let network = u32::from_be_bytes(network);
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - prefix)
    };
    address & mask == network & mask
}

fn ipv6_in_prefix(address: Ipv6Addr, network: Ipv6Addr, prefix: u8) -> bool {
    let address = u128::from_be_bytes(address.octets());
    let network = u128::from_be_bytes(network.octets());
    let mask = if prefix == 0 {
        0
    } else {
        u128::MAX << (128 - prefix)
    };
    address & mask == network & mask
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn navigation_syntax_blocks_unsafe_schemes_credentials_and_local_targets() {
        assert_eq!(validate_navigation_url(INITIAL_URL).unwrap(), INITIAL_URL);
        assert_eq!(
            validate_navigation_url("https://8.8.8.8").unwrap(),
            "https://8.8.8.8/"
        );
        for blocked in [
            "file:///C:/Windows/System32/drivers/etc/hosts",
            "javascript:alert(1)",
            "https://user:secret@example.com/",
            "http://localhost/",
            "http://service.internal/",
            "http://127.0.0.1/",
            "http://[::1]/",
            "http://169.254.169.254/latest/meta-data/",
        ] {
            assert_eq!(
                validate_navigation_url(blocked),
                Err(NativeBrowserError::NavigationBlocked),
                "{blocked}"
            );
        }
    }

    #[test]
    fn ip_classification_matches_the_sidecar_ssrf_contract() {
        for (address, expected) in [
            ("8.8.8.8", NativeIpClassification::Public),
            ("2606:4700:4700::1111", NativeIpClassification::Public),
            ("10.0.0.1", NativeIpClassification::Private),
            ("100.64.0.1", NativeIpClassification::Private),
            ("169.254.169.254", NativeIpClassification::Private),
            ("192.0.2.1", NativeIpClassification::Special),
            ("224.0.0.1", NativeIpClassification::Special),
            ("fc00::1", NativeIpClassification::Private),
            ("fe80::1", NativeIpClassification::Private),
            ("2001:db8::1", NativeIpClassification::Special),
            ("64:ff9b::808:808", NativeIpClassification::Special),
            ("::ffff:8.8.8.8", NativeIpClassification::Public),
            ("::ffff:127.0.0.1", NativeIpClassification::Private),
        ] {
            assert_eq!(
                classify_ip_address(address.parse().unwrap()),
                expected,
                "{address}"
            );
        }
        assert!(addresses_are_public(&[
            "8.8.8.8".parse().unwrap(),
            "1.1.1.1".parse().unwrap(),
        ]));
        assert!(!addresses_are_public(&[
            "8.8.8.8".parse().unwrap(),
            "127.0.0.1".parse().unwrap(),
        ]));
        assert!(!addresses_are_public(&[]));
    }

    #[test]
    fn cross_origin_redirects_strip_sensitive_headers_and_same_origin_keeps_them() {
        let headers = HashMap::from([
            ("Authorization".to_string(), "Bearer secret".to_string()),
            ("Cookie".to_string(), "session=secret".to_string()),
            ("X-Api-Key".to_string(), "secret".to_string()),
            ("Accept".to_string(), "text/html".to_string()),
        ]);
        assert!(sanitize_redirect_headers(
            "https://example.com/start",
            "https://example.com/next",
            &headers,
        )
        .unwrap()
        .is_none());

        let sanitized = sanitize_redirect_headers(
            "https://example.com/start",
            "https://other.example/next",
            &headers,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            sanitized,
            vec![json!({ "name": "Accept", "value": "text/html" })]
        );
    }

    #[test]
    fn redirect_tracking_is_bounded_and_removed_with_its_owner() {
        let mut tracker = RequestUrlTracker::default();
        let key = NativeRequestKey::new("connection-a", "tab-a", 7, "request-a");
        tracker.remember(key.clone(), "https://example.com/".to_string());
        assert_eq!(tracker.get(&key), Some("https://example.com/"));
        tracker.remove_tab("connection-a", "tab-a");
        assert_eq!(tracker.get(&key), None);
        assert_eq!(tracker.len(), 0);

        for index in 0..=MAX_INTERCEPTED_REQUESTS {
            tracker.remember(
                NativeRequestKey::new("connection-b", "tab-b", 8, &index.to_string()),
                format!("https://example.com/{index}"),
            );
        }
        assert_eq!(tracker.len(), MAX_INTERCEPTED_REQUESTS);
        assert!(tracker
            .get(&NativeRequestKey::new("connection-b", "tab-b", 8, "0"))
            .is_none());
        tracker.remove_session("connection-b");
        assert_eq!(tracker.len(), 0);
    }

    #[test]
    fn download_paths_are_randomized_scoped_and_preserve_existing_data() {
        let temp = tempfile::tempdir().unwrap();
        let download_root = temp.path().join("downloads");
        fs::create_dir_all(&download_root).unwrap();
        let sentinel = download_root.join("keep.txt");
        fs::write(&sentinel, "keep").unwrap();

        let managed = managed_download_path_for_root(
            &download_root,
            "connection-a",
            Path::new("..\\suggested.exe"),
        )
        .unwrap();
        let session = fs::canonicalize(download_root.join("connection-a")).unwrap();
        assert!(managed.starts_with(&session));
        assert_eq!(
            managed.extension().and_then(|value| value.to_str()),
            Some("exe")
        );
        assert_ne!(
            managed.file_name().and_then(|value| value.to_str()),
            Some("suggested.exe")
        );
        fs::write(&managed, "download").unwrap();
        assert_eq!(
            completed_download_path_for_root(&download_root, "connection-a", &managed).unwrap(),
            fs::canonicalize(&managed)
                .unwrap()
                .to_string_lossy()
                .into_owned()
        );

        let outside = temp.path().join("outside.exe");
        fs::write(&outside, "outside").unwrap();
        assert_eq!(
            completed_download_path_for_root(&download_root, "connection-a", &outside),
            Err(NativeBrowserError::ControllerFailed)
        );
        assert_eq!(fs::read_to_string(sentinel).unwrap(), "keep");
    }

    #[test]
    fn intercepted_event_parsing_accepts_only_bounded_request_ids() {
        assert_eq!(
            intercepted_request_id(
                r#"{"requestId":"request-a","request":{"url":"https://example.com/?token=secret"}}"#,
                256 * 1024,
            )
            .as_deref(),
            Some("request-a")
        );
        assert!(intercepted_request_id(
            r#"{"request":{"url":"https://example.com/"}}"#,
            256 * 1024,
        )
        .is_none());
        assert!(intercepted_request_id(
            &json!({ "requestId": "x".repeat(MAX_REQUEST_ID_BYTES + 1) }).to_string(),
            256 * 1024,
        )
        .is_none());
    }
}
