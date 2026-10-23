//! Where a [`crate::WebRtcLink`] meets its room: the signaling server, the room code, the
//! role, and the STUN servers, validated before matchbox_socket sees them (a malformed URL
//! panics both of its stacks).

use ludenea_net::RoomCode;
use thiserror::Error;

/// The STUN server peers use unless told otherwise: a public server of Google's.
pub const DEFAULT_STUN: &str = "stun:stun.l.google.com:19302";

/// Why [`LinkSettings::new`] refused a URL.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum LinkSettingsError {
    /// The signaling URL is not `ws://` or `wss://`, a host, and an optional path.
    #[error("the signaling URL '{url}' {problem}")]
    Signaling { url: String, problem: String },
    /// A STUN URL is not `stun:` or `stuns:`, a host, and an optional port.
    #[error("the STUN URL '{url}' {problem}")]
    Stun { url: String, problem: String },
}

/// The settings of one link, valid by construction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkSettings {
    signaling: String,
    code: RoomCode,
    host: Option<u8>,
    stun: Vec<String>,
}

impl LinkSettings {
    /// A link to room `code` through the signaling server at `signaling` (`ws://` or
    /// `wss://`), as its host when `host` is `Some(room size)`, with the STUN servers `stun`
    /// (`stun:` or `stuns:` URLs; none is allowed).
    ///
    /// # Errors
    ///
    /// [`LinkSettingsError::Signaling`] or [`LinkSettingsError::Stun`], naming the URL and
    /// what is wrong with it.
    pub fn new(
        signaling: &str,
        code: RoomCode,
        host: Option<u8>,
        stun: &[String],
    ) -> Result<LinkSettings, LinkSettingsError> {
        check_signaling(signaling).map_err(|problem| LinkSettingsError::Signaling {
            url: signaling.to_owned(),
            problem: problem.to_owned(),
        })?;
        for url in stun {
            check_stun(url).map_err(|problem| LinkSettingsError::Stun {
                url: url.clone(),
                problem: problem.to_owned(),
            })?;
        }
        Ok(LinkSettings {
            signaling: signaling.to_owned(),
            code,
            host,
            stun: stun.to_vec(),
        })
    }

    /// The signaling server's URL, as given.
    pub fn signaling(&self) -> &str {
        &self.signaling
    }

    /// The room's code.
    pub fn code(&self) -> &RoomCode {
        &self.code
    }

    /// The room's size when this peer hosts it.
    pub fn host(&self) -> Option<u8> {
        self.host
    }

    /// The STUN servers' URLs.
    pub fn stun(&self) -> &[String] {
        &self.stun
    }

    /// The room's URL: the signaling URL without its trailing slash, `/`, the code, and
    /// `?host=N` for the host of a room of N peers.
    pub fn room_url(&self) -> String {
        let base = self.signaling.trim_end_matches('/');
        match self.host {
            Some(size) => format!("{base}/{}?host={size}", self.code),
            None => format!("{base}/{}", self.code),
        }
    }
}

fn check_signaling(url: &str) -> Result<(), &'static str> {
    let rest = url
        .strip_prefix("ws://")
        .or_else(|| url.strip_prefix("wss://"))
        .ok_or("must start with ws:// or wss://")?;
    if rest.contains(['?', '#']) {
        return Err("must not have a query or a fragment: the room code and role follow it");
    }
    if rest
        .chars()
        .any(|character| character.is_whitespace() || character.is_control())
    {
        return Err("must not contain spaces or control characters");
    }
    let authority = rest.split('/').next().unwrap_or_default();
    check_authority(authority)
}

fn check_stun(url: &str) -> Result<(), &'static str> {
    let authority = url
        .strip_prefix("stun:")
        .or_else(|| url.strip_prefix("stuns:"))
        .ok_or("must start with stun: or stuns:")?;
    check_authority(authority)
}

/// A host (a name, an IPv4 address, or an IPv6 address in brackets) and an optional port.
fn check_authority(authority: &str) -> Result<(), &'static str> {
    let (host, port) = if let Some(bracketed) = authority.strip_prefix('[') {
        let (address, after) = bracketed
            .split_once(']')
            .ok_or("has an IPv6 address without its closing bracket")?;
        if address.is_empty()
            || !address
                .chars()
                .all(|character| character.is_ascii_hexdigit() || matches!(character, ':' | '.'))
        {
            return Err("has a malformed IPv6 address");
        }
        match after {
            "" => (address, None),
            _ => (
                address,
                Some(
                    after
                        .strip_prefix(':')
                        .ok_or("has text after its IPv6 address")?,
                ),
            ),
        }
    } else {
        match authority.split_once(':') {
            Some((host, port)) => (host, Some(port)),
            None => (authority, None),
        }
    };
    if host.is_empty() {
        return Err("has no host");
    }
    if !authority.starts_with('[')
        && !host.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '.' | '_')
        })
    {
        return Err("has a host that is not a name or an address");
    }
    if let Some(port) = port
        && !(port.bytes().all(|byte| byte.is_ascii_digit())
            && port.parse::<u16>().is_ok_and(|port| port != 0))
    {
        return Err("has a port that is not a number from 1 to 65535");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code() -> RoomCode {
        RoomCode::parse("ABC234").unwrap()
    }

    fn settings(signaling: &str, host: Option<u8>) -> LinkSettings {
        LinkSettings::new(signaling, code(), host, &[DEFAULT_STUN.to_owned()]).unwrap()
    }

    fn signaling_problem(url: &str) -> String {
        LinkSettings::new(url, code(), None, &[])
            .unwrap_err()
            .to_string()
    }

    fn stun_problem(url: &str) -> String {
        LinkSettings::new("ws://127.0.0.1:3536/rooms", code(), None, &[url.to_owned()])
            .unwrap_err()
            .to_string()
    }

    #[test]
    fn valid_urls_are_accepted() {
        for url in [
            "ws://127.0.0.1:3536/rooms",
            "wss://games.example.com/rooms/",
            "ws://localhost",
            "ws://[::1]:3536/rooms",
        ] {
            assert!(LinkSettings::new(url, code(), None, &[]).is_ok(), "{url}");
        }
        let stun = [
            DEFAULT_STUN.to_owned(),
            "stuns:stun.example.com".to_owned(),
            "stun:192.168.1.2:3478".to_owned(),
            "stun:[2001:db8::1]:3478".to_owned(),
        ];
        assert!(LinkSettings::new("ws://localhost", code(), None, &stun).is_ok());
    }

    #[test]
    fn a_signaling_url_is_refused_naming_it_and_the_problem() {
        let cases = [
            ("http://127.0.0.1/rooms", "must start with ws:// or wss://"),
            ("127.0.0.1:3536", "must start with ws:// or wss://"),
            ("ws://", "has no host"),
            ("ws:///rooms", "has no host"),
            ("ws://:3536/rooms", "has no host"),
            (
                "ws://host:port/rooms",
                "has a port that is not a number from 1 to 65535",
            ),
            (
                "ws://host:70000",
                "has a port that is not a number from 1 to 65535",
            ),
            (
                "ws://host:0",
                "has a port that is not a number from 1 to 65535",
            ),
            (
                "ws://host:+80",
                "has a port that is not a number from 1 to 65535",
            ),
            (
                "ws://user@host/rooms",
                "has a host that is not a name or an address",
            ),
            (
                "ws://[::1/rooms",
                "has an IPv6 address without its closing bracket",
            ),
            ("ws://[]:80", "has a malformed IPv6 address"),
            ("ws://[::1]x/rooms", "has text after its IPv6 address"),
            (
                "ws://host/rooms?x=1",
                "must not have a query or a fragment: the room code and role follow it",
            ),
            (
                "ws://host/my rooms",
                "must not contain spaces or control characters",
            ),
        ];
        for (url, problem) in cases {
            assert_eq!(
                signaling_problem(url),
                format!("the signaling URL '{url}' {problem}")
            );
        }
    }

    #[test]
    fn a_stun_url_is_refused_naming_it_and_the_problem() {
        let cases = [
            ("turn:turn.example.com", "must start with stun: or stuns:"),
            ("stun:", "has no host"),
            (
                "stun://stun.example.com",
                "has a host that is not a name or an address",
            ),
            (
                "stun:stun.example.com:x",
                "has a port that is not a number from 1 to 65535",
            ),
            (
                "stun:stun.example.com/path",
                "has a host that is not a name or an address",
            ),
            (
                "stun:[::1",
                "has an IPv6 address without its closing bracket",
            ),
        ];
        for (url, problem) in cases {
            assert_eq!(stun_problem(url), format!("the STUN URL '{url}' {problem}"));
        }
    }

    #[test]
    fn the_first_bad_stun_url_is_named() {
        let stun = [
            DEFAULT_STUN.to_owned(),
            "stun:".to_owned(),
            "nonsense".to_owned(),
        ];
        let error = LinkSettings::new("ws://localhost", code(), None, &stun).unwrap_err();
        assert_eq!(
            error,
            LinkSettingsError::Stun {
                url: "stun:".to_owned(),
                problem: "has no host".to_owned()
            }
        );
    }

    #[test]
    fn the_room_url_drops_the_trailing_slash_and_names_the_host() {
        assert_eq!(
            settings("ws://127.0.0.1:3536/rooms", None).room_url(),
            "ws://127.0.0.1:3536/rooms/ABC234"
        );
        assert_eq!(
            settings("wss://games.example.com/rooms/", Some(3)).room_url(),
            "wss://games.example.com/rooms/ABC234?host=3"
        );
        assert_eq!(
            settings("ws://127.0.0.1:3536", Some(2)).room_url(),
            "ws://127.0.0.1:3536/ABC234?host=2"
        );
    }
}
