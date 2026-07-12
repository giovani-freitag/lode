use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::{Client, RequestBuilder, Response};
use serde::de::DeserializeOwned;

/// A generous ceiling for a single download when the expected size isn't known ahead of time.
/// Mod jars and loader installers are far below this; it exists only so a hostile or misbehaving
/// endpoint can't stream an unbounded body straight into memory and OOM the process.
pub const DEFAULT_MAX_DOWNLOAD: u64 = 1024 * 1024 * 1024;

/// Slack added to a known content size before it becomes the cap, so a legitimate file that is a
/// few bytes larger than the recorded size (re-compression, metadata) still downloads.
pub const SIZE_MARGIN: u64 = 64 * 1024;

/// Ceiling for a sibling checksum file (`<url>.sha256`). A checksum line is a few dozen bytes; the
/// cap only exists so a hostile host can't stream an unbounded body in place of a checksum.
pub const MAX_CHECKSUM: u64 = 64 * 1024;

/// Ceiling for a metadata/API JSON body. Provider catalogs and release manifests are small; the cap
/// keeps a hostile or misbehaving host from streaming an unbounded body into a `.json()` decode.
const MAX_JSON: u64 = 8 * 1024 * 1024;

/// The most redirect hops a download will follow. Matches reqwest's own default; re-declared here
/// because the SSRF-guarding custom policy replaces the built-in redirect limit.
const MAX_REDIRECTS: usize = 10;

/// The one User-Agent every lode request sends. Providers key rate limits and etiquette off it,
/// so it names the tool and its repo; kept in one place so every request stays in lockstep.
pub const USER_AGENT: &str = concat!(
    "lode/",
    env!("CARGO_PKG_VERSION"),
    " (github.com/giovani-freitag/lode)"
);

/// How long to wait for a TCP+TLS connection before giving up — the same for every client.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// Overall deadline for a metadata/API request. These bodies are tiny JSON, so a whole-request
/// cap is the right guard: a provider that stalls can't hang the command forever.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Overall deadline for a streamed download. Far more generous than the metadata timeout so a
/// large jar or loader installer over a slow link isn't cut off, while a truly hung transfer
/// still eventually aborts instead of blocking forever.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);

/// The shared client for metadata/API calls (resolve, release lookups, version catalogs). Bounds
/// both the connect and the overall request so a stalled provider can't hang the command.
pub fn client() -> Result<Client> {
    Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(REQUEST_TIMEOUT)
        .redirect(ssrf_redirect_policy())
        .build()
        .context("building HTTP client")
}

/// The shared client for streamed downloads (jars, installers, pack archives). Uses a much longer
/// overall deadline than metadata calls so a large file over a slow link isn't cut off, while the
/// connect timeout still fails fast on a dead host.
pub fn download_client() -> Result<Client> {
    Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(CONNECT_TIMEOUT)
        .timeout(DOWNLOAD_TIMEOUT)
        .redirect(ssrf_redirect_policy())
        .build()
        .context("building HTTP client")
}

/// The single SSRF choke point. A download destination must be `https` and must not resolve to a
/// non-routable or internal address — loopback, link-local (incl. the `169.254.169.254` cloud
/// metadata endpoint), private, CGNAT, or the IPv6 equivalents. Applied to the initial URL of every
/// download (see `download_once`) and re-applied to each redirect hop here, so neither an
/// attacker-authored URL nor a public host that 30x-redirects inward can reach an internal service.
/// No host allowlist: self-hosted mavens and `--from-url` are first-class, so the guard is by
/// address class, not by name.
fn guard_download_url(url: &reqwest::Url) -> Result<()> {
    if url.scheme() != "https" {
        bail!(
            "refusing to fetch {url} — downloads must use https, not '{}'",
            url.scheme()
        );
    }
    let host = url
        .host_str()
        .ok_or_else(|| anyhow!("refusing to fetch {url} — it has no host"))?;
    // A literal IP is checked as-is; a name is resolved and *every* address it yields is checked, so
    // a hostname that points at an internal address is caught too (best-effort against rebinding).
    if let Some(ip) = parse_host_ip(host) {
        reject_blocked_ip(ip, url)?;
    } else {
        let port = url.port_or_known_default().unwrap_or(443);
        // An unresolvable host isn't a bypass: the real connection would fail on its own anyway.
        if let Ok(addrs) = (host, port).to_socket_addrs() {
            for addr in addrs {
                reject_blocked_ip(addr.ip(), url)?;
            }
        }
    }
    Ok(())
}

/// Parse a URL host as a literal IP, tolerating the `[..]` brackets a URL wraps an IPv6 literal in.
fn parse_host_ip(host: &str) -> Option<IpAddr> {
    let bare = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    bare.parse::<IpAddr>().ok()
}

fn reject_blocked_ip(ip: IpAddr, url: &reqwest::Url) -> Result<()> {
    if is_blocked_ip(ip) {
        bail!("refusing to fetch {url} — it resolves to a non-routable or internal address ({ip})");
    }
    Ok(())
}

/// Whether an address is in a range a download must never reach: loopback, private, link-local
/// (which covers the `169.254.169.254` cloud metadata service), CGNAT, unspecified/broadcast, or
/// the IPv6 unique-local / link-local equivalents. IPv4-mapped IPv6 is unwrapped so an internal v4
/// tunneled through v6 is caught.
fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || is_cgnat(v4)
        }
        IpAddr::V6(v6) => {
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return is_blocked_ip(IpAddr::V4(mapped));
            }
            v6.is_loopback()
                || v6.is_unspecified()
                || is_v6_unique_local(v6)
                || is_v6_link_local(v6)
        }
    }
}

/// CGNAT / shared address space, `100.64.0.0/10` (RFC 6598).
fn is_cgnat(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    o[0] == 100 && (64..=127).contains(&o[1])
}

/// IPv6 unique-local addresses, `fc00::/7` (RFC 4193) — the v6 analogue of RFC 1918 private space.
fn is_v6_unique_local(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xfe00) == 0xfc00
}

/// IPv6 link-local addresses, `fe80::/10`.
fn is_v6_link_local(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xffc0) == 0xfe80
}

/// A redirect policy that re-runs the SSRF guard on every hop and caps the hop count. A blocked or
/// non-https redirect target aborts the request instead of being followed.
fn ssrf_redirect_policy() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= MAX_REDIRECTS {
            return attempt.error(anyhow!("too many redirects (over {MAX_REDIRECTS})"));
        }
        match guard_download_url(attempt.url()) {
            Ok(()) => attempt.follow(),
            Err(e) => attempt.error(e),
        }
    })
}

/// Maximum attempts a retryable operation makes before surfacing the last error.
const MAX_ATTEMPTS: u32 = 3;

/// Whether a failed operation is worth retrying. Transport faults (connect/timeout/broken body)
/// and the transient server responses (429, 5xx) are retried; a definitive 4xx like 401/404 is
/// the server's final word and must never be retried. The reqwest error is found anywhere in the
/// anyhow context chain so `with_context`-wrapped failures still classify correctly.
pub fn is_retryable(err: &anyhow::Error) -> bool {
    for cause in err.chain() {
        if let Some(re) = cause.downcast_ref::<reqwest::Error>() {
            return match re.status() {
                Some(status) => status.as_u16() == 429 || status.is_server_error(),
                None => re.is_connect() || re.is_timeout() || re.is_request(),
            };
        }
    }
    false
}

/// Run `op`, retrying transient failures with exponential backoff up to `MAX_ATTEMPTS`. A
/// non-retryable error (or the last attempt) surfaces immediately. `what` names the operation for
/// the retry notice.
pub fn with_retry<T>(what: &str, mut op: impl FnMut() -> Result<T>) -> Result<T> {
    let mut attempt = 1;
    loop {
        match op() {
            Ok(value) => return Ok(value),
            Err(err) => {
                if attempt >= MAX_ATTEMPTS || !is_retryable(&err) {
                    return Err(err);
                }
                let backoff = Duration::from_millis(200 * 2u64.pow(attempt - 1));
                eprintln!(
                    "  {what} failed (attempt {attempt}/{MAX_ATTEMPTS}): {err:#} — retrying in {backoff:?}"
                );
                std::thread::sleep(backoff);
                attempt += 1;
            }
        }
    }
}

/// Send `req` and read the response body into memory, aborting once it streams past `max` bytes.
/// This is the one place every download is bounded — a truncated or lying `Content-Length` can't
/// get past the running byte counter. `what` names the resource for error context. Transient
/// network failures are retried; the request is rebuilt each attempt via `try_clone`.
pub fn download_capped(req: RequestBuilder, max: u64, what: &str) -> Result<Vec<u8>> {
    with_retry(what, || {
        let attempt = req
            .try_clone()
            .ok_or_else(|| anyhow!("request for {what} cannot be retried (non-clonable body)"))?;
        download_once(attempt, max, what)
    })
}

/// A single download attempt: send the request, reject an over-cap advertised length early, then
/// stream the body under the running byte cap.
fn download_once(req: RequestBuilder, max: u64, what: &str) -> Result<Vec<u8>> {
    // Guard the initial destination before any bytes leave the process. The redirect policy covers
    // every subsequent hop, but the first URL never passes through it, so it is checked here.
    let peek = req
        .try_clone()
        .ok_or_else(|| anyhow!("request for {what} cannot be validated (non-clonable body)"))?
        .build()
        .with_context(|| format!("building the request for {what}"))?;
    guard_download_url(peek.url())?;

    let mut resp = req
        .send()
        .with_context(|| format!("downloading {what}"))?
        .error_for_status()
        .with_context(|| format!("downloading {what}"))?;

    // Reject early when the server advertises an over-cap length; the loop below is the real guard.
    if let Some(len) = resp.content_length() {
        if len > max {
            bail!("{what} is {len} bytes, over the {max}-byte download cap — refusing");
        }
    }

    read_capped(&mut resp, max, what)
}

/// Decode a checked response body as JSON under `MAX_JSON`, so a metadata host can't stream an
/// unbounded body into a `.json()` decode and OOM the process. A drop-in for `.json()` on a response
/// that has already been through `error_for_status`; `what` names the resource for error context.
pub fn json_capped<T: DeserializeOwned>(resp: Response, what: &str) -> Result<T> {
    let mut resp = resp;
    let bytes = read_capped(&mut resp, MAX_JSON, what)?;
    serde_json::from_slice(&bytes).with_context(|| format!("decoding {what}"))
}

/// Read `reader` fully into a buffer, failing if it yields more than `max` bytes. Split out from
/// the network path so the cap logic is unit-testable without a live server.
fn read_capped<R: Read>(reader: &mut R, max: u64, what: &str) -> Result<Vec<u8>> {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    let mut total: u64 = 0;
    loop {
        let n = reader
            .read(&mut chunk)
            .with_context(|| format!("reading body of {what}"))?;
        if n == 0 {
            break;
        }
        total += n as u64;
        if total > max {
            bail!("{what} exceeded the {max}-byte download cap — aborting");
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn read_capped_accepts_a_body_within_the_cap() {
        let data = vec![7u8; 1000];
        let out = read_capped(&mut Cursor::new(data.clone()), 1000, "x").unwrap();
        assert_eq!(out, data);
    }

    #[test]
    fn read_capped_aborts_past_the_cap() {
        let data = vec![0u8; 2000];
        assert!(read_capped(&mut Cursor::new(data), 1024, "x").is_err());
    }

    #[test]
    fn with_retry_returns_on_first_success_without_looping() {
        use std::cell::Cell;
        let calls = Cell::new(0);
        let out: Result<u32> = with_retry("x", || {
            calls.set(calls.get() + 1);
            Ok(7)
        });
        assert_eq!(out.unwrap(), 7);
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn blocks_internal_and_metadata_addresses() {
        for bad in [
            "127.0.0.1",
            "10.0.0.5",
            "172.16.9.9",
            "192.168.1.1",
            "169.254.169.254", // cloud metadata
            "100.64.0.1",      // CGNAT
            "0.0.0.0",
            "::1",
            "fe80::1",
            "fc00::1",
            "::ffff:127.0.0.1", // IPv4-mapped loopback
        ] {
            assert!(is_blocked_ip(bad.parse().unwrap()), "should block {bad}");
        }
    }

    #[test]
    fn allows_public_addresses() {
        for ok in ["1.1.1.1", "8.8.8.8", "140.82.112.3", "2606:4700:4700::1111"] {
            assert!(!is_blocked_ip(ok.parse().unwrap()), "should allow {ok}");
        }
    }

    #[test]
    fn guard_rejects_non_https_and_literal_internal_hosts() {
        assert!(guard_download_url(&"http://example.com/x".parse().unwrap()).is_err());
        assert!(guard_download_url(&"https://127.0.0.1/x".parse().unwrap()).is_err());
        assert!(guard_download_url(&"https://[::1]/x".parse().unwrap()).is_err());
        assert!(guard_download_url(&"https://169.254.169.254/latest".parse().unwrap()).is_err());
    }

    #[test]
    fn with_retry_does_not_retry_a_non_retryable_error() {
        use std::cell::Cell;
        // A plain (non-reqwest) error is treated as definitive, so the op runs exactly once.
        let calls = Cell::new(0);
        let out: Result<u32> = with_retry("x", || {
            calls.set(calls.get() + 1);
            Err(anyhow!("nope"))
        });
        assert!(out.is_err());
        assert_eq!(calls.get(), 1);
    }
}
