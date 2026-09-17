//! `NO_PROXY` matching, shared by the env loader and the HTTP client.
//!
//! Entries are separated by commas or whitespace. An entry is `*`, a domain
//! (`example.com`, `.example.com`, `*.example.com`: the name and every
//! subdomain), an IP address (`127.0.0.1`, `::1`, `[::1]`), or a CIDR block
//! (`10.0.0.0/8`, `fd00::/8`). A domain or address may carry `:port`, which
//! then has to equal the request's port. An IP-literal host only ever matches
//! an address or CIDR entry, never a domain suffix.
//!
//! Upstream bun 63a495cb46 (B9): one matcher replaces the loader's old
//! inline loop (comma-only, no `*.`, no CIDR, IPv6 only bracketed, IP
//! literals suffix-matched).

use core::net::IpAddr;

use bun_core::strings;

fn strip_trailing_dot(host: &[u8]) -> &[u8] {
    host.strip_suffix(b".").unwrap_or(host)
}

/// A dotted-quad or IPv6 address, parsed the same way on every platform, with
/// `::ffff:a.b.c.d` folded into `a.b.c.d`. The resolver's
/// shorthand (`127.1`, `0x7f.1`) is not an address here: `fetch()` and
/// `WebSocket` hosts arrive normalized to the dotted quad, and a host the
/// other callers took from configuration is compared as written, like curl does.
fn parse_ip(text: &[u8]) -> Option<IpAddr> {
    bun_core::fmt::parse_ascii::<IpAddr>(text).map(|ip| ip.to_canonical())
}

/// Splits `host[:port]`, where `host` may be a bracketed or bare IPv6 literal.
fn split_port(entry: &[u8]) -> (&[u8], Option<&[u8]>) {
    if entry.first() == Some(&b'[') {
        if let Some(close) = strings::index_of_char_usize(entry, b']') {
            let rest = &entry[close + 1..];
            return (&entry[1..close], rest.strip_prefix(b":"));
        }
        return (entry, None);
    }
    if strings::count_char(entry, b':') == 1 {
        let colon = strings::index_of_char_usize(entry, b':').unwrap();
        return (&entry[..colon], Some(&entry[colon + 1..]));
    }
    (entry, None)
}

/// A port or a prefix length: ASCII digits only (`parse_unsigned` alone would take `8_0`).
fn parse_digits<T: TryFrom<i128> + TryFrom<u128>>(text: &[u8]) -> Option<T> {
    if text.is_empty() || !text.iter().all(u8::is_ascii_digit) {
        return None;
    }
    bun_core::fmt::parse_unsigned::<T>(text, 10).ok()
}

fn cidr_contains(entry: &[u8], slash: usize, host: IpAddr) -> bool {
    let Some(written) = bun_core::fmt::parse_ascii::<IpAddr>(bun_url::strip_ipv6_brackets(
        &entry[..slash],
    )) else {
        return false;
    };
    let Some(mut bits) = parse_digits::<u8>(&entry[slash + 1..]) else {
        return false;
    };
    // `::ffff:10.0.0.0/104` is `10.0.0.0/8`.
    let network = written.to_canonical();
    if written.is_ipv6() && network.is_ipv4() {
        let Some(v4_bits) = bits.checked_sub(96) else {
            return false;
        };
        bits = v4_bits;
    }
    fn prefix_eq(a: &[u8], b: &[u8], bits: u8) -> bool {
        let bits = bits as usize;
        if bits > a.len() * 8 {
            return false;
        }
        let (whole, rem) = (bits / 8, bits % 8);
        if a[..whole] != b[..whole] {
            return false;
        }
        rem == 0 || (a[whole] ^ b[whole]) >> (8 - rem) == 0
    }
    match (network, host) {
        (IpAddr::V4(n), IpAddr::V4(h)) => prefix_eq(&n.octets(), &h.octets(), bits),
        (IpAddr::V6(n), IpAddr::V6(h)) => prefix_eq(&n.octets(), &h.octets(), bits),
        _ => false,
    }
}

/// Split on any of `,`, space, tab, CR, LF (upstream `strings::tokenize_any`;
/// bao's `strings` has no tokenizer yet, so the split is local).
fn entries(list: &[u8]) -> impl Iterator<Item = &[u8]> {
    struct Entries<'a> {
        rest: &'a [u8],
    }
    impl<'a> Iterator for Entries<'a> {
        type Item = &'a [u8];
        fn next(&mut self) -> Option<Self::Item> {
            let rest = self.rest;
            let start = rest
                .iter()
                .position(|b| !matches!(b, b',' | b' ' | b'\t' | b'\r' | b'\n'))?;
            let rest = &rest[start..];
            let end = rest
                .iter()
                .position(|b| matches!(b, b',' | b' ' | b'\t' | b'\r' | b'\n'))
                .unwrap_or(rest.len());
            self.rest = &rest[end..];
            Some(&rest[..end])
        }
    }
    Entries { rest: list }
}

/// Whether `hostname` (no port; an IPv6 literal may be bracketed) on `port`
/// is exempted from proxying by the `NO_PROXY` value `list`.
pub fn matches(list: &[u8], hostname: &[u8], port: u16) -> bool {
    let hostname = strip_trailing_dot(bun_url::strip_ipv6_brackets(hostname));
    if hostname.is_empty() {
        return false;
    }
    let mut iter = entries(list).peekable();
    if iter.peek().is_none() {
        return false;
    }
    let host_ip = parse_ip(hostname);

    for entry in iter {
        if entry == b"*" {
            return true;
        }
        if let Some(slash) = strings::index_of_char_usize(entry, b'/') {
            if host_ip.is_some_and(|ip| cidr_contains(entry, slash, ip)) {
                return true;
            }
            continue;
        }

        let (entry_host, entry_port) = split_port(entry);
        if let Some(entry_port) = entry_port {
            match parse_digits::<u16>(entry_port) {
                Some(p) if p == port => {}
                _ => continue,
            }
        }

        let entry_host = entry_host
            .strip_prefix(b"*.")
            .or_else(|| entry_host.strip_prefix(b"."))
            .unwrap_or(entry_host);
        let entry_host = strip_trailing_dot(entry_host);
        if entry_host.is_empty() {
            continue;
        }

        if let Some(ip) = host_ip {
            if parse_ip(entry_host) == Some(ip) {
                return true;
            }
            continue;
        }

        if hostname.len() == entry_host.len() {
            if strings::eql_case_insensitive_ascii(hostname, entry_host, true) {
                return true;
            }
        } else if hostname.len() > entry_host.len()
            && hostname[hostname.len() - entry_host.len() - 1] == b'.'
            && strings::eql_case_insensitive_ascii(
                &hostname[hostname.len() - entry_host.len()..],
                entry_host,
                true,
            )
        {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    //! Semantic pin of the B9 matcher (upstream bun 63a495cb46): a mismatch
    //! here is either a proxy bypass (false negative) or an over-block (false
    //! positive) — both are security-relevant regressions.

    use super::matches;

    #[test]
    fn star_matches_any_host() {
        assert!(matches(b"*", b"example.com", 80));
        assert!(matches(b"*", b"10.0.0.1", 80));
        assert!(matches(b"*", b"[::1]", 80));
        // Only the bare `*` is the wildcard (exact entry compare, as curl);
        // `*:8080` is the host `*`, which no hostname dot-suffix-matches.
        assert!(!matches(b"*:8080", b"example.com", 8080));
        assert!(!matches(b"*:8080", b"example.com", 443));
    }

    #[test]
    fn bare_domain_matches_self_and_subdomains_only() {
        assert!(matches(b"example.com", b"example.com", 443));
        assert!(matches(b"example.com", b"sub.example.com", 443));
        assert!(matches(b"example.com", b"a.b.example.com", 443));
        // No dot-boundary suffix confusion.
        assert!(!matches(b"example.com", b"notexample.com", 443));
        assert!(!matches(b"example.com", b"example.com.evil.io", 443));
    }

    #[test]
    fn leading_dot_and_wildcard_forms() {
        // `.example.com`, `*.example.com` and the bare name are the same set
        // (the name and every subdomain), as curl reads them.
        for list in [b".example.com".as_slice(), b"*.example.com".as_slice()] {
            assert!(matches(list, b"example.com", 80), "{:?}", list);
            assert!(matches(list, b"sub.example.com", 80), "{:?}", list);
            assert!(!matches(list, b"notexample.com", 80), "{:?}", list);
            assert!(!matches(list, b"example.org", 80), "{:?}", list);
        }
    }

    #[test]
    fn case_insensitive_and_trailing_dot() {
        assert!(matches(b"EXAMPLE.com", b"example.com", 80));
        assert!(matches(b"example.com", b"SUB.Example.COM", 80));
        // A trailing dot on the host is stripped before matching.
        assert!(matches(b"example.com", b"example.com.", 80));
        // An entry consisting of only a dot is empty after stripping.
        assert!(!matches(b".", b"example.com", 80));
    }

    #[test]
    fn ipv4_cidr() {
        assert!(matches(b"10.0.0.0/8", b"10.1.2.3", 80));
        assert!(!matches(b"10.0.0.0/8", b"11.0.0.1", 80));
        assert!(matches(b"192.168.1.0/24", b"192.168.1.55", 80));
        assert!(!matches(b"192.168.1.0/24", b"192.168.2.1", 80));
        // /32 is an exact address.
        assert!(matches(b"192.168.1.1/32", b"192.168.1.1", 80));
        assert!(!matches(b"192.168.1.1/32", b"192.168.1.2", 80));
    }

    #[test]
    fn ipv6_cidr_bare_and_bracketed_host() {
        assert!(matches(b"fd00::/8", b"fd00::1", 80));
        assert!(matches(b"fd00::/8", b"[fd12::1]", 80));
        assert!(!matches(b"fd00::/8", b"fe00::1", 80));
        // A CIDR entry never matches a name.
        assert!(!matches(b"fd00::/8", b"fd00.example.com", 80));
    }

    #[test]
    fn v4_mapped_v6_cidr_folds_to_v4_prefix() {
        // `::ffff:10.0.0.0/104` is `10.0.0.0/8` after canonicalization:
        // 104 v6 bits = the 96-bit map prefix + 8 v4 bits, so any 10.x host
        // is inside, and only a different first octet is out.
        assert!(matches(b"::ffff:10.0.0.0/104", b"10.0.0.5", 80));
        assert!(matches(b"::ffff:10.0.0.0/104", b"10.1.0.5", 80));
        assert!(!matches(b"::ffff:10.0.0.0/104", b"11.0.0.5", 80));
    }

    #[test]
    fn bracketed_and_bare_ipv6_entries_and_host() {
        assert!(matches(b"[::1]", b"[::1]", 80));
        assert!(matches(b"[::1]", b"::1", 80));
        assert!(matches(b"::1", b"[::1]", 80));
        assert!(!matches(b"::1", b"::2", 80));
        // `split_port` strips the brackets of ANY bracketed entry without
        // asking whether it is an IPv6 literal (upstream shape), so
        // `[example.com]` names the host `example.com`.
        assert!(matches(b"[example.com]", b"example.com", 80));
    }

    #[test]
    fn host_port_compares_effective_port() {
        assert!(matches(b"example.com:8080", b"example.com", 8080));
        assert!(!matches(b"example.com:8080", b"example.com", 443));
        // A port-less entry applies to every port.
        assert!(matches(b"example.com", b"example.com", 9999));
        // A port on a subdomain entry still matches the subdomain.
        assert!(matches(b".example.com:8080", b"sub.example.com", 8080));
        assert!(!matches(b".example.com:8080", b"sub.example.com", 80));
    }

    #[test]
    fn bracketed_v6_entry_with_port() {
        assert!(matches(b"[::1]:8080", b"[::1]", 8080));
        assert!(matches(b"[::1]:8080", b"::1", 8080));
        assert!(!matches(b"[::1]:8080", b"::1", 443));
    }

    #[test]
    fn ip_literal_host_never_matches_domain_suffix() {
        // The pre-B9 matcher suffix-matched `127.0.0.1` against `.0.0.1`;
        // an IP-literal host only ever matches an address or block entry.
        for entry in [
            b"0.0.1".as_slice(),
            b".0.0.1",
            b"1",
            b".1",
            b"com",
            b"127.0.0.1.example.com",
            b"example.com",
        ] {
            assert!(
                !matches(entry, b"127.0.0.1", 80),
                "IP host must not match domain entry {:?}",
                entry
            );
        }
        // …but the exact address and a covering block do.
        assert!(matches(b"127.0.0.1", b"127.0.0.1", 80));
        assert!(matches(b"127.0.0.0/8", b"127.0.0.1", 80));
    }

    #[test]
    fn resolver_shorthand_is_not_an_address() {
        // `127.1` / `0x7f.1` are resolver shorthand, not dotted quads: an
        // IP-literal host never matches them, and they are compared as
        // written against a non-IP host.
        assert!(!matches(b"127.1", b"127.0.0.1", 80));
        assert!(!matches(b"0x7f.1", b"127.0.0.1", 80));
        // Non-IP host against shorthand: plain domain comparison.
        assert!(matches(b"127.1", b"127.1", 80));
        // IP entry against a non-IP host: no match either way.
        assert!(!matches(b"127.0.0.1", b"127.1", 80));
    }

    #[test]
    fn mixed_comma_and_whitespace_separators() {
        let list = b" a.com,b.com\tc.com\r13.0.0.0/24\nd.com";
        assert!(matches(list, b"a.com", 80));
        assert!(matches(list, b"b.com", 80));
        assert!(matches(list, b"c.com", 80));
        assert!(matches(list, b"d.com", 80));
        assert!(matches(list, b"13.0.0.9", 80));
        assert!(!matches(list, b"e.com", 80));
    }

    #[test]
    fn port_and_prefix_digits_only() {
        // `parse_unsigned` alone would take `8_0`; ASCII digits only.
        assert!(!matches(b"example.com:8_0", b"example.com", 80));
        assert!(!matches(b"example.com:80x", b"example.com", 80));
        assert!(!matches(b"example.com:", b"example.com", 80));
        assert!(!matches(b"example.com:+80", b"example.com", 80));
        // CIDR prefix lengths: same rule, and it must fit u8.
        assert!(!matches(b"10.0.0.0/8x", b"10.1.2.3", 80));
        assert!(!matches(b"10.0.0.0/", b"10.1.2.3", 80));
        assert!(!matches(b"10.0.0.0/999", b"10.1.2.3", 80));
        // A valid prefix still matches.
        assert!(matches(b"10.0.0.0/08", b"10.1.2.3", 80));
    }

    #[test]
    fn empty_list_or_empty_host_never_matches() {
        assert!(!matches(b"", b"example.com", 80));
        assert!(!matches(b"   ", b"example.com", 80));
        assert!(!matches(b",,,", b"example.com", 80));
        assert!(!matches(b"example.com", b"", 80));
        assert!(!matches(b"example.com", b".", 80));
    }
}
