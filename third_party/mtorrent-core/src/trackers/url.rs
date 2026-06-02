use derive_more::{Debug, Deref};
use serde::de::IntoDeserializer;
use serde::{Deserialize, Serialize, Serializer};
use std::str::FromStr;

/// Parsed tracker address.
#[derive(Debug, PartialEq, Eq, Clone, Hash, PartialOrd, Ord)]
pub enum TrackerUrl {
    Http(Http),
    Udp(Udp),
}

#[derive(Debug, PartialEq, Eq, Clone, Hash, PartialOrd, Ord, Deref)]
#[deref(forward)]
#[debug("{_0}")]
pub struct Http(String);

#[derive(Debug, PartialEq, Eq, Clone, Hash, PartialOrd, Ord, Deref)]
#[deref(forward)]
#[debug("{_0}")]
pub struct Udp(String);

impl Serialize for TrackerUrl {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            TrackerUrl::Http(Http(url)) => url.serialize(serializer),
            TrackerUrl::Udp(Udp(addr)) => format!("udp://{addr}").serialize(serializer),
        }
    }
}

impl<'de> Deserialize<'de> for TrackerUrl {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{Error, Unexpected};

        let text_url = String::deserialize(deserializer)?;
        let parsed_url = url::Url::parse(&text_url).map_err(Error::custom)?;
        if let Some(host) = parsed_url.host()
            && host_is_forbidden(&host)
        {
            return Err(Error::invalid_value(
                Unexpected::Str(&text_url),
                &"a non-loopback, non-link-local tracker host",
            ));
        }
        match parsed_url.scheme() {
            "http" | "https" => Ok(TrackerUrl::Http(Http(text_url))),
            "udp" => Ok(TrackerUrl::Udp(Udp(format!(
                "{}:{}",
                parsed_url
                    .host_str()
                    .ok_or(Error::invalid_value(Unexpected::Str(&text_url), &"hostname present"))?,
                parsed_url
                    .port()
                    .ok_or(Error::invalid_value(Unexpected::Str(&text_url), &"port present"))?
            )))),
            scheme => Err(Error::invalid_value(Unexpected::Str(scheme), &"supported scheme")),
        }
    }
}

/// Reject IP-literal tracker hosts in the link-local, unspecified, or broadcast ranges
/// (incl. the cloud-metadata address 169.254.169.254), closing an SSRF vector where a
/// torrent-supplied tracker URL targets a non-routable local-network endpoint. Loopback
/// is intentionally allowed so that legitimate local trackers keep working. Domain names
/// are left untouched (resolution happens later in the network client).
fn host_is_forbidden(host: &url::Host<&str>) -> bool {
    use std::net::IpAddr;
    // For non-special schemes (udp) the url crate keeps the host as an opaque domain
    // string, so an IP literal arrives as `Domain("169.254.1.1")` / `Domain("[fe80::1]")`.
    let ip = match host {
        url::Host::Ipv4(a) => IpAddr::V4(*a),
        url::Host::Ipv6(a) => IpAddr::V6(*a),
        url::Host::Domain(d) => match d.trim_start_matches('[').trim_end_matches(']').parse() {
            Ok(ip) => ip,
            Err(_) => return false,
        },
    };
    match ip {
        IpAddr::V4(a) => a.is_link_local() || a.is_unspecified() || a.is_broadcast(),
        IpAddr::V6(a) => a.is_unspecified() || (a.segments()[0] & 0xffc0) == 0xfe80,
    }
}

impl FromStr for TrackerUrl {
    type Err = serde::de::value::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        TrackerUrl::deserialize(s.into_deserializer())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn test_extract_http_and_udp_trackers() {
        let trackers = [
            "udp://open.stealth.si:80/announce",
            "udp://tracker.opentrackr.org:1337/announce",
            "udp://tracker.tiny-vps.com:6969",
            "https://example.com",
            "udp://tracker.internetwarriors.net:1337/",
            "udp://tracker.skyts.net:6969/announce330",
            "http://example.com",
            "udp://[2001:db8:85a3:8d3:1319:8a2e:370:7348]:6969/",
        ];
        let expected_udp_trackers = [
            "open.stealth.si:80",
            "tracker.opentrackr.org:1337",
            "tracker.tiny-vps.com:6969",
            "tracker.internetwarriors.net:1337",
            "tracker.skyts.net:6969",
            "[2001:db8:85a3:8d3:1319:8a2e:370:7348]:6969",
        ];
        let expected_http_trackers = ["https://example.com", "http://example.com"];

        let mut udp_trackers = HashSet::new();
        let mut http_trackers = HashSet::new();
        for tracker in trackers {
            match tracker.parse::<TrackerUrl>().unwrap() {
                TrackerUrl::Http(addr) => http_trackers.insert(addr),
                TrackerUrl::Udp(addr) => udp_trackers.insert(addr),
            };
        }

        assert_eq!(
            http_trackers,
            expected_http_trackers.into_iter().map(ToString::to_string).map(Http).collect()
        );
        assert_eq!(
            udp_trackers,
            expected_udp_trackers.into_iter().map(ToString::to_string).map(Udp).collect()
        );
    }

    #[test]
    fn test_parse_malformed_tracker_urls() {
        let result = "invalid".parse::<TrackerUrl>();
        assert!(result.is_err(), "{result:?}");

        let err = "udp://tracker.tiny-vps.com/announce".parse::<TrackerUrl>().unwrap_err();
        assert!(err.to_string().contains("expected port present"), "{err}");

        let err = "udp:/run/foo.socket".parse::<TrackerUrl>().unwrap_err();
        assert!(err.to_string().contains("expected hostname present"), "{err}");

        let err = "ssh://open.stealth.si:80/announce".parse::<TrackerUrl>().unwrap_err();
        assert!(err.to_string().contains("expected supported scheme"), "{err}");
    }

    #[test]
    fn test_reject_link_local_and_unspecified_hosts() {
        let forbidden = [
            "http://169.254.1.1/announce",
            "udp://169.254.169.254:80/announce",
            "http://0.0.0.0/announce",
            "udp://[fe80::1]:80/announce",
            "http://[::]/announce",
        ];
        for tracker in forbidden {
            let err = tracker.parse::<TrackerUrl>().unwrap_err();
            assert!(err.to_string().contains("non-link-local"), "{tracker}: {err}");
        }

        // Public IPs, hostnames, and loopback (legitimate local trackers) must still parse.
        assert!("udp://8.8.8.8:1337/announce".parse::<TrackerUrl>().is_ok());
        assert!("http://example.com/announce".parse::<TrackerUrl>().is_ok());
        assert!("udp://127.0.0.1:1337/announce".parse::<TrackerUrl>().is_ok());
        assert!("http://[::1]/announce".parse::<TrackerUrl>().is_ok());
    }
}
