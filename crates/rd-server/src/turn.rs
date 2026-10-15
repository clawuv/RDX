//! TURN REST API credential issuance (coturn `use-auth-secret`).
//!
//! The signaling server shares one static secret with coturn and signs
//! time-limited credentials on registration; it never terminates or
//! interprets relayed media (ADR-0002).

use std::time::Duration;

use hmac::{Hmac, Mac};
use rd_protocol::signaling::TurnCredential;
use sha1::Sha1;

type HmacSha1 = Hmac<Sha1>;

/// A new secret invalidates every outstanding credential; deployment rotates
/// it out-of-band (see deploy/README.md).
pub struct TurnIssuer {
    secret: Vec<u8>,
    ttl: Duration,
    turn_addr: String,
    stun_addr: String,
}

impl TurnIssuer {
    pub fn new(
        secret: Vec<u8>,
        ttl: Duration,
        turn_addr: impl Into<String>,
        stun_addr: impl Into<String>,
    ) -> Self {
        Self {
            secret,
            ttl,
            turn_addr: turn_addr.into(),
            stun_addr: stun_addr.into(),
        }
    }

    /// `username = "<unix expiry>:<device id>"`,
    /// `password = base64(HMAC-SHA1(secret, username))` per the TURN REST API
    /// scheme; coturn validates both with the same shared secret.
    pub fn issue(&self, device_id: &str, now_unix: u64) -> TurnCredential {
        let expires_unix = now_unix + self.ttl.as_secs();
        let username = format!("{expires_unix}:{device_id}");
        let mut mac = HmacSha1::new_from_slice(&self.secret).expect("hmac accepts any key length");
        mac.update(username.as_bytes());
        use base64::Engine as _;
        let password =
            base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
        TurnCredential {
            username,
            password,
            expires_unix,
            turn_addr: self.turn_addr.clone(),
            stun_addr: self.stun_addr.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_matches_independent_hmac_vector() {
        let issuer = TurnIssuer::new(
            b"turn-static-secret".to_vec(),
            Duration::from_secs(600),
            "turn.example.com:3478",
            "turn.example.com:3478",
        );
        let credential = issuer.issue("device-42", 1_790_000_000);
        assert_eq!(credential.username, "1790000600:device-42");
        assert_eq!(credential.expires_unix, 1_790_000_600);
        assert_eq!(credential.username_expiry(), Some(1_790_000_600));

        let mut mac = HmacSha1::new_from_slice(b"turn-static-secret").unwrap();
        mac.update(credential.username.as_bytes());
        use base64::Engine as _;
        assert_eq!(
            credential.password,
            base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
        );
        assert_eq!(credential.turn_addr, "turn.example.com:3478");
    }

    #[test]
    fn different_devices_and_times_yield_different_passwords() {
        let issuer = TurnIssuer::new(b"s".to_vec(), Duration::from_secs(60), "t:1", "s:1");
        let a = issuer.issue("a", 100);
        let b = issuer.issue("b", 100);
        let later = issuer.issue("a", 101);
        assert_ne!(a.password, b.password);
        assert_ne!(a.password, later.password);
    }
}
