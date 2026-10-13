use super::webhook_handlers::utils::parse_webhook_payload;
use actix_web::http::header::HeaderMap;
use hmac::{Hmac, KeyInit, Mac};
use serde_json::Value;
use sha2::Sha256;

// Immutable GitHub account IDs: bitsocialnet, bitsocialforge, pkcprotocol.
// A renamed organization remains trusted; a new account taking its old name does not.
const ALLOWED_ORGANIZATION_IDS: [u64; 3] = [243813100, 296465714, 245761493];

#[derive(Clone)]
pub struct WebhookSecurity {
    secret: Vec<u8>,
}

impl std::fmt::Debug for WebhookSecurity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WebhookSecurity { secret: [REDACTED] }")
    }
}

#[derive(Debug, PartialEq)]
pub enum WebhookRejection {
    InvalidSignature,
    InvalidEvent,
    InvalidPayload,
    DisallowedOwner,
    NonPublicRepository,
}

impl WebhookSecurity {
    pub fn new(secret: &str) -> Result<Self, &'static str> {
        if secret.trim().len() < 32 {
            return Err("must contain at least 32 non-padding bytes; use a random secret");
        }
        Ok(Self {
            secret: secret.as_bytes().to_vec(),
        })
    }

    /// Authenticate the exact received bytes before parsing either JSON or form data.
    pub fn authorize(&self, headers: &HeaderMap, body: &[u8]) -> Result<(), WebhookRejection> {
        let mut signatures = headers.get_all("x-hub-signature-256");
        let signature = signatures.next().and_then(|value| value.to_str().ok());
        if signatures.next().is_some() || !self.verify_signature(signature, body) {
            return Err(WebhookRejection::InvalidSignature);
        }

        let mut events = headers.get_all("x-github-event");
        let event = events
            .next()
            .and_then(|value| value.to_str().ok())
            .filter(|value| !value.is_empty())
            .ok_or(WebhookRejection::InvalidEvent)?;
        if events.next().is_some() {
            return Err(WebhookRejection::InvalidEvent);
        }

        let payload: Value =
            parse_webhook_payload(body).map_err(|_| WebhookRejection::InvalidPayload)?;
        if !allowed_owner(&payload, event) {
            return Err(WebhookRejection::DisallowedOwner);
        }
        if let Some(repository) = payload.get("repository") {
            // Fail closed if visibility is missing, malformed, or inconsistent.
            // GitHub push payloads may omit `visibility`, but must say `private: false`.
            if repository["private"].as_bool() != Some(false)
                || repository
                    .get("visibility")
                    .is_some_and(|visibility| visibility.as_str() != Some("public"))
            {
                return Err(WebhookRejection::NonPublicRepository);
            }
        }
        Ok(())
    }

    fn verify_signature(&self, signature: Option<&str>, body: &[u8]) -> bool {
        let Some(digest) = signature.and_then(|value| value.strip_prefix("sha256=")) else {
            return false;
        };
        let mut expected = [0_u8; 32];
        if hex::decode_to_slice(digest, &mut expected).is_err() {
            return false;
        }

        let mut mac =
            Hmac::<Sha256>::new_from_slice(&self.secret).expect("HMAC accepts keys of any length");
        mac.update(body);
        // RustCrypto verifies the complete SHA-256 MAC in constant time.
        mac.verify_slice(&expected).is_ok()
    }
}

fn allowed_owner(payload: &Value, event: &str) -> bool {
    let organization = payload.get("organization").filter(|value| !value.is_null());
    let owner_id = match payload.get("repository") {
        Some(repository) => {
            let owner = &repository["owner"];
            if owner["type"].as_str() != Some("Organization") {
                return false;
            }
            owner["id"].as_u64()
        }
        // GitHub organization-hook pings do not include a repository.
        None if event == "ping" => organization.and_then(|org| org["id"].as_u64()),
        None => None,
    };

    let Some(owner_id) = owner_id else {
        return false;
    };
    ALLOWED_ORGANIZATION_IDS.contains(&owner_id)
        && organization.is_none_or(|org| org["id"].as_u64() == Some(owner_id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use actix_web::http::header::{HeaderName, HeaderValue};
    use serde_json::json;

    const TEST_SECRET: &str = "a-long-test-secret-used-only-for-unit-tests";

    fn security() -> WebhookSecurity {
        WebhookSecurity::new(TEST_SECRET).unwrap()
    }

    fn signed_headers(body: &[u8], event: &str) -> HeaderMap {
        let mut mac = Hmac::<Sha256>::new_from_slice(TEST_SECRET.as_bytes()).unwrap();
        mac.update(body);
        let signature = format!("sha256={}", hex::encode(mac.finalize().into_bytes()));
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static("x-hub-signature-256"),
            HeaderValue::from_str(&signature).unwrap(),
        );
        headers.insert(
            HeaderName::from_static("x-github-event"),
            HeaderValue::from_str(event).unwrap(),
        );
        headers
    }

    fn repository_payload(id: u64) -> Value {
        json!({"repository": {"private": false, "owner": {"id": id, "type": "Organization"}}})
    }

    #[test]
    fn verifies_github_documented_test_vector() {
        // Public interoperability vector from GitHub's webhook validation docs.
        let verifier = WebhookSecurity {
            secret: b"It's a Secret to Everybody".to_vec(),
        };
        assert!(verifier.verify_signature(
            Some("sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17"),
            b"Hello, World!"
        ));
    }

    #[test]
    fn rejects_missing_empty_and_short_configuration() {
        for secret in ["", " ", "too-short", "                                "] {
            assert!(WebhookSecurity::new(secret).is_err());
        }
        assert!(!format!("{:?}", security()).contains(TEST_SECRET));
    }

    #[test]
    fn accepts_all_three_organizations_in_json_and_form_data() {
        for id in ALLOWED_ORGANIZATION_IDS {
            let json = repository_payload(id).to_string();
            let form = serde_urlencoded::to_string([("payload", &json)]).unwrap();
            for body in [json.as_bytes(), form.as_bytes()] {
                assert_eq!(
                    security().authorize(&signed_headers(body, "push"), body),
                    Ok(())
                );
            }
            // Signing decoded JSON must not authenticate the encoded form bytes.
            assert_eq!(
                security().authorize(&signed_headers(json.as_bytes(), "push"), form.as_bytes()),
                Err(WebhookRejection::InvalidSignature)
            );
        }
    }

    #[test]
    fn rejects_missing_malformed_wrong_and_duplicate_signatures() {
        let body = repository_payload(ALLOWED_ORGANIZATION_IDS[0]).to_string();
        for signature in [
            None,
            Some(""),
            Some("sha1=abc"),
            Some("sha256=aa"),
            Some("sha256=zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"),
            Some("sha256=0000000000000000000000000000000000000000000000000000000000000000"),
        ] {
            assert!(!security().verify_signature(signature, body.as_bytes()));
        }
        let mut headers = signed_headers(body.as_bytes(), "push");
        headers.append(
            HeaderName::from_static("x-hub-signature-256"),
            HeaderValue::from_static("sha256=invalid"),
        );
        assert_eq!(
            security().authorize(&headers, body.as_bytes()),
            Err(WebhookRejection::InvalidSignature)
        );
    }

    #[test]
    fn rejects_tampering_and_wrong_secret() {
        let body = repository_payload(ALLOWED_ORGANIZATION_IDS[0]).to_string();
        let headers = signed_headers(body.as_bytes(), "push");
        assert_eq!(
            security().authorize(&headers, format!("{body} ").as_bytes()),
            Err(WebhookRejection::InvalidSignature)
        );
        let other = WebhookSecurity::new("another-long-test-secret-for-negative-tests").unwrap();
        assert_eq!(
            other.authorize(&headers, body.as_bytes()),
            Err(WebhookRejection::InvalidSignature)
        );
    }

    #[test]
    fn rejects_foreign_missing_malformed_and_conflicting_owners() {
        let id = ALLOWED_ORGANIZATION_IDS[0];
        for payload in [
            repository_payload(1),
            json!({}),
            json!({"repository": null, "organization": {"id": id}}),
            json!({"repository": {"owner": {"login": "bitsocialnet"}}}),
            json!({"repository": {"owner": {"id": 1, "type": "Organization", "login": "bitsocialnet"}}}),
            json!({"repository": {"owner": {"id": id, "type": "User"}}}),
            json!({"repository": {"owner": {"id": id.to_string(), "type": "Organization"}}}),
            json!({"repository": {"owner": {"id": 1, "type": "Organization"}}, "organization": {"id": id}}),
            json!({"repository": {"owner": {"id": id, "type": "Organization"}}, "organization": {"id": 1}}),
            json!({"organization": {"id": id}}),
        ] {
            let body = payload.to_string();
            assert_eq!(
                security().authorize(&signed_headers(body.as_bytes(), "push"), body.as_bytes()),
                Err(WebhookRejection::DisallowedOwner),
                "{body}"
            );
        }
    }

    #[test]
    fn handles_organization_pings_without_bypassing_repository_checks() {
        let id = ALLOWED_ORGANIZATION_IDS[0];
        assert!(allowed_owner(&json!({"organization": {"id": id}}), "ping"));
        assert!(!allowed_owner(&json!({"organization": {"id": 1}}), "ping"));
        assert!(!allowed_owner(
            &json!({"repository": {}, "organization": {"id": id}}),
            "ping"
        ));
        assert!(allowed_owner(
            &json!({"repository": {"owner": {"id": id, "type": "Organization"}}, "organization": {"id": id}}),
            "issues"
        ));
    }

    #[test]
    fn excludes_private_and_uncertain_visibility_in_json_and_form_data() {
        for private in [
            None,
            Some(json!(true)),
            Some(json!(false)),
            Some(json!(null)),
            Some(json!("false")),
        ] {
            for visibility in [
                None,
                Some(json!("public")),
                Some(json!("private")),
                Some(json!("internal")),
                Some(json!(null)),
            ] {
                let mut payload = repository_payload(ALLOWED_ORGANIZATION_IDS[0]);
                let repository = payload["repository"].as_object_mut().unwrap();
                repository.remove("private");
                if let Some(value) = &private {
                    repository.insert("private".into(), value.clone());
                }
                if let Some(value) = &visibility {
                    repository.insert("visibility".into(), value.clone());
                }
                let is_public = private == Some(json!(false))
                    && visibility.as_ref().is_none_or(|value| value == "public");
                let json = payload.to_string();
                let form = serde_urlencoded::to_string([("payload", &json)]).unwrap();
                for event in ["push", "issues", "release", "ping"] {
                    for body in [json.as_bytes(), form.as_bytes()] {
                        let result = security().authorize(&signed_headers(body, event), body);
                        if is_public {
                            assert_eq!(result, Ok(()));
                        } else {
                            assert_eq!(result, Err(WebhookRejection::NonPublicRepository));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn permits_signed_organization_pings_without_repository_visibility() {
        for id in ALLOWED_ORGANIZATION_IDS {
            let json = json!({"organization": {"id": id}}).to_string();
            let form = serde_urlencoded::to_string([("payload", &json)]).unwrap();
            for body in [json.as_bytes(), form.as_bytes()] {
                assert_eq!(
                    security().authorize(&signed_headers(body, "ping"), body),
                    Ok(())
                );
                assert_eq!(
                    security().authorize(&signed_headers(body, "push"), body),
                    Err(WebhookRejection::DisallowedOwner)
                );
            }
        }
    }

    #[test]
    fn rejects_signed_invalid_payloads_and_missing_or_duplicate_event_headers() {
        let body = b"not json or form data";
        assert_eq!(
            security().authorize(&signed_headers(body, "push"), body),
            Err(WebhookRejection::InvalidPayload)
        );
        let body = repository_payload(ALLOWED_ORGANIZATION_IDS[0]).to_string();
        let mut headers = signed_headers(body.as_bytes(), "push");
        headers.remove("x-github-event");
        assert_eq!(
            security().authorize(&headers, body.as_bytes()),
            Err(WebhookRejection::InvalidEvent)
        );
        let mut headers = signed_headers(body.as_bytes(), "push");
        headers.append(
            HeaderName::from_static("x-github-event"),
            HeaderValue::from_static("ping"),
        );
        assert_eq!(
            security().authorize(&headers, body.as_bytes()),
            Err(WebhookRejection::InvalidEvent)
        );
    }
}
