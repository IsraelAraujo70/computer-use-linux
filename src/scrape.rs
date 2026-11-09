//! `scrape`: fetch a public web page and return its readable text, without
//! a browser. Guarded against server-side request forgery: only http(s),
//! no credentials in the URL, and every hop (redirects included) must
//! resolve to public addresses only; the checked address is pinned for curl
//! (`--resolve`) so DNS cannot change between check and fetch.

use anyhow::{bail, Context, Result};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use std::time::Duration;

const MAX_REDIRECTS: usize = 5;
const MAX_BYTES: u64 = 5 * 1024 * 1024;
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);
pub(crate) const DEFAULT_MAX_CHARS: usize = 20_000;

/// The page at `url` as text: its title, then the readable body (links as
/// `[text](href)`), cut at `max_chars`.
pub(crate) async fn scrape(url: &str, max_chars: usize) -> Result<String> {
    let mut current = url.trim().to_string();
    for _ in 0..=MAX_REDIRECTS {
        let target = Target::parse(&current)?;
        let ip = public_address(&target.host, target.port)?;
        let fetched = fetch(&current, &target, ip).await?;
        if (300..400).contains(&fetched.status) {
            let location = fetched
                .location
                .with_context(|| format!("{} redirect without Location", fetched.status))?;
            current = resolve_location(&target, &location);
            continue;
        }
        if !(200..300).contains(&fetched.status) {
            bail!("{current} answered HTTP {}", fetched.status);
        }
        let text = String::from_utf8_lossy(&fetched.body).into_owned();
        let readable = if fetched.content_type.contains("html")
            || text.trim_start().starts_with('<')
        {
            html_to_text(&text)
        } else if fetched.content_type.starts_with("text/") || fetched.content_type.contains("json")
        {
            text
        } else {
            bail!(
                "{current} is {}, not a web page",
                fetched.content_type.trim()
            );
        };
        let total = readable.chars().count();
        let mut out = format!("url: {current}\n");
        out.extend(readable.chars().take(max_chars));
        if total > max_chars {
            out.push_str(&format!(
                "\n… ({} more characters; raise max_chars)",
                total - max_chars
            ));
        }
        return Ok(out);
    }
    bail!("more than {MAX_REDIRECTS} redirects")
}

struct Target {
    scheme: String,
    host: String,
    port: u16,
}

impl Target {
    fn parse(url: &str) -> Result<Self> {
        let (scheme, rest) = url
            .split_once("://")
            .context("the URL needs http:// or https://")?;
        let scheme = scheme.to_ascii_lowercase();
        let default_port = match scheme.as_str() {
            "http" => 80,
            "https" => 443,
            other => bail!("only http and https URLs are fetched, not {other}"),
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        if authority.contains('@') {
            bail!("URLs with credentials are refused");
        }
        let (host, port) = match authority.strip_prefix('[') {
            Some(v6) => {
                let (host, after) = v6.split_once(']').context("unclosed IPv6 host")?;
                let port = after.strip_prefix(':').map(str::parse).transpose()?;
                (host.to_string(), port)
            }
            None => match authority.rsplit_once(':') {
                Some((host, port)) => (host.to_string(), Some(port.parse()?)),
                None => (authority.to_string(), None),
            },
        };
        if host.is_empty() {
            bail!("the URL has no host");
        }
        Ok(Self {
            scheme,
            host: host.to_ascii_lowercase(),
            port: port.unwrap_or(default_port),
        })
    }
}

/// The host's first address, after checking that every address it resolves
/// to is public.
fn public_address(host: &str, port: u16) -> Result<IpAddr> {
    let addresses: Vec<IpAddr> = (host, port)
        .to_socket_addrs()
        .with_context(|| format!("could not resolve {host}"))?
        .map(|address| address.ip())
        .collect();
    let first = *addresses
        .first()
        .with_context(|| format!("{host} has no address"))?;
    if let Some(private) = addresses.iter().find(|ip| !is_public(ip)) {
        bail!("{host} resolves to {private}, a private or local address; only public sites are fetched");
    }
    Ok(first)
}

fn is_public(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => is_public_v4(&v4),
            None => is_public_v6(v6),
        },
    }
}

fn is_public_v4(ip: &Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || ip.is_documentation()
        // 100.64.0.0/10: carrier-grade NAT, where Tailscale addresses live.
        || (a == 100 && (64..128).contains(&b))
        || a == 0
        || a >= 240)
}

fn is_public_v6(ip: &Ipv6Addr) -> bool {
    let first = ip.segments()[0];
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_multicast()
        // fc00::/7 unique local, fe80::/10 link local.
        || (first & 0xfe00) == 0xfc00
        || (first & 0xffc0) == 0xfe80)
}

struct Fetched {
    status: u16,
    content_type: String,
    location: Option<String>,
    body: Vec<u8>,
}

async fn fetch(url: &str, target: &Target, ip: IpAddr) -> Result<Fetched> {
    let pinned = match ip {
        IpAddr::V4(v4) => format!("{}:{}:{v4}", target.host, target.port),
        IpAddr::V6(v6) => format!("{}:{}:[{v6}]", target.host, target.port),
    };
    let output = tokio::time::timeout(
        FETCH_TIMEOUT + Duration::from_secs(2),
        tokio::process::Command::new("curl")
            .args([
                "--silent",
                "--show-error",
                "--proto",
                "=http,https",
                "--max-redirs",
                "0",
                "--max-time",
                &FETCH_TIMEOUT.as_secs().to_string(),
                "--max-filesize",
                &MAX_BYTES.to_string(),
                "--compressed",
                "--user-agent",
                "Mozilla/5.0 (computer-use scrape)",
                "--resolve",
                &pinned,
                "--include",
                "--",
                url,
            ])
            .kill_on_drop(true)
            .output(),
    )
    .await
    .context("the fetch timed out")?
    .context("could not run curl")?;
    if !output.status.success() {
        bail!(
            "fetching {url} failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    parse_response(&output.stdout)
}

/// Splits curl's `--include` output into the last header block and body.
fn parse_response(raw: &[u8]) -> Result<Fetched> {
    let mut rest = raw;
    loop {
        let split = rest
            .windows(4)
            .position(|window| window == b"\r\n\r\n")
            .map(|at| (at, 4))
            .or_else(|| {
                rest.windows(2)
                    .position(|window| window == b"\n\n")
                    .map(|at| (at, 2))
            })
            .context("response has no headers")?;
        let (head, body) = (&rest[..split.0], &rest[split.0 + split.1..]);
        let head = String::from_utf8_lossy(head);
        let mut lines = head.lines();
        let status: u16 = lines
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .and_then(|code| code.parse().ok())
            .context("response has no status line")?;
        // 1xx interim responses come before the real one.
        if (100..200).contains(&status) && body.starts_with(b"HTTP/") {
            rest = body;
            continue;
        }
        let header = |name: &str| {
            head.lines().find_map(|line| {
                let (key, value) = line.split_once(':')?;
                key.trim()
                    .eq_ignore_ascii_case(name)
                    .then(|| value.trim().to_string())
            })
        };
        return Ok(Fetched {
            status,
            content_type: header("content-type")
                .unwrap_or_default()
                .to_ascii_lowercase(),
            location: header("location"),
            body: body.to_vec(),
        });
    }
}

fn resolve_location(base: &Target, location: &str) -> String {
    if location.contains("://") {
        return location.to_string();
    }
    let origin = format!("{}://{}:{}", base.scheme, base.host, base.port);
    match location.strip_prefix("//") {
        Some(rest) => format!("{}://{rest}", base.scheme),
        None if location.starts_with('/') => format!("{origin}{location}"),
        None => format!("{origin}/{location}"),
    }
}

/// Readable text of an HTML page: title, headings, paragraphs and list
/// items on their own lines, links as `[text](href)`, scripts and styles
/// dropped, entities decoded.
pub(crate) fn html_to_text(html: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let title = lower.find("<title").and_then(|start| {
        let open_end = lower[start..].find('>')? + start + 1;
        let close = lower[open_end..].find("</title")? + open_end;
        Some(decode_entities(html[open_end..close].trim()))
    });
    let mut out = String::new();
    if let Some(title) = title.filter(|title| !title.is_empty()) {
        out.push_str(&format!("# {title}\n\n"));
    }
    let body_start = lower.find("<body").unwrap_or(0);
    let mut i = body_start;
    let bytes = html.as_bytes();
    let mut link: Option<String> = None;
    while i < html.len() {
        if bytes[i] == b'<' {
            let Some(end) = html[i..].find('>').map(|at| at + i) else {
                break;
            };
            let tag = &lower[i + 1..end];
            let name = tag
                .trim_start_matches('/')
                .split(|c: char| c.is_whitespace() || c == '/')
                .next()
                .unwrap_or_default();
            let closing = tag.starts_with('/');
            if tag.starts_with("!--") {
                i = lower[i..].find("-->").map_or(html.len(), |at| at + i + 3);
                continue;
            }
            if !closing
                && matches!(
                    name,
                    "script" | "style" | "noscript" | "svg" | "template" | "head"
                )
            {
                let close = format!("</{name}");
                i = lower[end..]
                    .find(&close)
                    .and_then(|at| lower[end + at..].find('>').map(|gt| end + at + gt + 1))
                    .unwrap_or(html.len());
                continue;
            }
            match (name, closing) {
                ("br", _)
                | (
                    "p" | "div" | "tr" | "section" | "article" | "header" | "footer" | "table"
                    | "ul" | "ol",
                    _,
                ) => out.push('\n'),
                ("li", false) => out.push_str("\n- "),
                ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", false) => {
                    let level = name[1..].parse::<usize>().unwrap_or(2);
                    out.push_str(&format!("\n\n{} ", "#".repeat(level.min(6))));
                }
                ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", true) => out.push('\n'),
                ("td" | "th", false) => out.push_str(" | "),
                ("a", false) => {
                    link = attribute(&html[i + 1..end], "href");
                    out.push('[');
                }
                ("a", true) => match link.take() {
                    Some(href) if !href.starts_with('#') && !href.starts_with("javascript:") => {
                        out.push_str(&format!("]({href})"))
                    }
                    _ => out.push(']'),
                },
                _ => {}
            }
            i = end + 1;
        } else {
            let next = html[i..].find('<').map_or(html.len(), |at| at + i);
            out.push_str(&decode_entities(&html[i..next]));
            i = next;
        }
    }
    // Collapse whitespace inside lines and runs of blank lines; drop empty
    // link brackets.
    let mut text = String::new();
    let mut blank = 0;
    for line in out.replace("[]", "").lines() {
        let line = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if line.is_empty() || line == "-" {
            blank += 1;
            if blank == 1 && !text.is_empty() {
                text.push('\n');
            }
            continue;
        }
        blank = 0;
        text.push_str(&line);
        text.push('\n');
    }
    text.trim().to_string()
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let lower = tag.to_ascii_lowercase();
    let at = lower.find(&format!("{name}="))? + name.len() + 1;
    let rest = &tag[at..];
    let value = match rest.chars().next()? {
        quote @ ('"' | '\'') => rest[1..].split(quote).next()?,
        _ => rest.split(|c: char| c.is_whitespace() || c == '>').next()?,
    };
    Some(decode_entities(value))
}

fn decode_entities(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        rest = &rest[at..];
        let Some(end) = rest[..rest.len().min(12)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" | "#39" => Some('\''),
            "nbsp" => Some(' '),
            _ => entity
                .strip_prefix("#x")
                .or_else(|| entity.strip_prefix("#X"))
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|dec| dec.parse().ok()))
                .and_then(char::from_u32),
        };
        match decoded {
            Some(character) => {
                out.push(character);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_and_tailnet_addresses_are_refused() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "192.168.0.10",
            "172.16.5.4",
            "169.254.169.254",
            "100.106.74.86",
            "::1",
            "fd00::1",
            "fe80::1",
            "::ffff:10.0.0.1",
        ] {
            assert!(!is_public(&ip.parse().unwrap()), "{ip} must be refused");
        }
        for ip in ["93.184.216.34", "2606:2800:220:1::1", "1.1.1.1"] {
            assert!(is_public(&ip.parse().unwrap()), "{ip} is public");
        }
    }

    #[test]
    fn urls_with_credentials_or_other_schemes_are_refused() {
        assert!(Target::parse("file:///etc/passwd").is_err());
        assert!(Target::parse("https://user:pw@example.com/").is_err());
        let target = Target::parse("https://Example.com:8443/a?b").unwrap();
        assert_eq!((target.host.as_str(), target.port), ("example.com", 8443));
        let v6 = Target::parse("http://[2606:2800::1]/").unwrap();
        assert_eq!((v6.host.as_str(), v6.port), ("2606:2800::1", 80));
    }

    #[test]
    fn html_becomes_readable_text_with_links() {
        let html = r#"<html><head><title>Boleto &amp; Pagamento</title><style>p{}</style></head>
            <body><h1>Pagar</h1><script>alert(1)</script><p>Valor: R&#36; 10,00&nbsp;hoje</p>
            <ul><li>Um</li><li><a href="/segunda-via">Segunda via</a></li></ul><!-- x --></body></html>"#;
        let text = html_to_text(html);
        assert!(text.starts_with("# Boleto & Pagamento"), "{text}");
        assert!(text.contains("# Pagar"), "{text}");
        assert!(text.contains("Valor: R$ 10,00 hoje"), "{text}");
        assert!(text.contains("- [Segunda via](/segunda-via)"), "{text}");
        assert!(!text.contains("alert") && !text.contains("p{}"), "{text}");
    }

    #[test]
    fn curl_output_splits_into_status_headers_and_body() {
        let raw = b"HTTP/1.1 100 Continue\r\n\r\nHTTP/2 301\r\nLocation: /next\r\nContent-Type: text/html\r\n\r\n<p>moved</p>";
        let fetched = parse_response(raw).unwrap();
        assert_eq!(fetched.status, 301);
        assert_eq!(fetched.location.as_deref(), Some("/next"));
        assert_eq!(fetched.body, b"<p>moved</p>");
        let base = Target::parse("https://example.com/a").unwrap();
        assert_eq!(
            resolve_location(&base, "/next"),
            "https://example.com:443/next"
        );
    }
}
