use aidoku::{
	alloc::{String, string::ToString, vec},
	imports::defaults::{DefaultValue, defaults_set},
};
use aidoku_test::aidoku_test;

use crate::auth::{
	ACCESS_TOKEN_KEY, AUTH_JSON_KEY, CLIENT_ID_KEY, REFRESH_TOKEN_KEY, Tokens,
	candidate_client_ids, clean_token, fingerprint, get_tokens, is_expired, jwt_claims,
	parse_auth_json, refresh_recently_failed, remember_failed_refresh,
};

// {"aud":"1","exp":1700000000,"sub":"4242"}
const JWT: &str =
	"eyJhbGciOiJSUzI1NiJ9.eyJhdWQiOiIxIiwiZXhwIjoxNzAwMDAwMDAwLCJzdWIiOiI0MjQyIn0.c2ln";

fn clear() {
	for key in [
		ACCESS_TOKEN_KEY,
		REFRESH_TOKEN_KEY,
		AUTH_JSON_KEY,
		CLIENT_ID_KEY,
	] {
		defaults_set(key, DefaultValue::String(String::new()));
	}
}

#[aidoku_test]
fn clean_token_strips_noise() {
	assert_eq!(clean_token("  \"Bearer abc\" "), Some("abc".to_string()));
	assert_eq!(clean_token("bearer abc"), Some("abc".to_string()));
	assert_eq!(clean_token("abc"), Some("abc".to_string()));
	assert_eq!(clean_token("   "), None);
}

#[aidoku_test]
fn jwt_claims_are_decoded() {
	let claims = jwt_claims(JWT).expect("claims");
	assert_eq!(claims.exp, Some(1700000000.0));
	assert!(jwt_claims("not-a-jwt").is_none());
}

#[aidoku_test]
fn expiry_uses_leeway() {
	assert!(is_expired(JWT, 1700000000));
	assert!(is_expired(JWT, 1700000000 - 60));
	assert!(!is_expired(JWT, 1700000000 - 600));
	// Opaque tokens are never considered expired.
	assert!(!is_expired("opaque", i64::MAX));
}

#[aidoku_test]
fn local_storage_auth_is_parsed() {
	let parsed = parse_auth_json(
		r#"{"token":{"token_type":"Bearer","access_token":"a","refresh_token":"r","expires_in":1},"auth":{}}"#,
	)
	.expect("parsed");
	assert_eq!(parsed.access_token, Some("a".to_string()));
	assert_eq!(parsed.refresh_token, Some("r".to_string()));

	let parsed = parse_auth_json(r#"{"access_token":"a2"}"#).expect("parsed");
	assert_eq!(parsed.access_token, Some("a2".to_string()));
	assert!(parse_auth_json("plain").is_none());
}

#[aidoku_test]
fn tokens_precedence() {
	clear();
	defaults_set(
		AUTH_JSON_KEY,
		DefaultValue::String(r#"{"token":{"access_token":"j","refresh_token":"jr"}}"#.into()),
	);
	assert_eq!(get_tokens().access_token, Some("j".to_string()));

	defaults_set(ACCESS_TOKEN_KEY, DefaultValue::String("Bearer x".into()));
	defaults_set(REFRESH_TOKEN_KEY, DefaultValue::String("y".into()));
	let tokens = get_tokens();
	assert_eq!(tokens.access_token, Some("x".to_string()));
	assert_eq!(tokens.refresh_token, Some("y".to_string()));

	// Token JSON pasted into the access token field: its own refresh token
	// wins over an old value in the refresh token field.
	defaults_set(
		ACCESS_TOKEN_KEY,
		DefaultValue::String(r#"{"access_token":"na","refresh_token":"nr"}"#.into()),
	);
	let tokens = get_tokens();
	assert_eq!(tokens.access_token, Some("na".to_string()));
	assert_eq!(tokens.refresh_token, Some("nr".to_string()));
	defaults_set(
		ACCESS_TOKEN_KEY,
		DefaultValue::String(r#"{"access_token":"na"}"#.into()),
	);
	assert_eq!(get_tokens().refresh_token, Some("y".to_string()));
	clear();
}

#[aidoku_test]
fn client_ids_order() {
	clear();
	let tokens = Tokens {
		access_token: Some(JWT.to_string()),
		refresh_token: Some("r".to_string()),
		token_type: None,
	};
	assert_eq!(candidate_client_ids(&tokens), vec!["1", "3", "2"]);

	defaults_set(CLIENT_ID_KEY, DefaultValue::String("3".into()));
	assert_eq!(candidate_client_ids(&tokens), vec!["3", "1", "2"]);
	clear();
}

#[aidoku_test]
fn failed_refresh_is_not_retried_immediately() {
	remember_failed_refresh(Some("def50200aa"), 1000);
	assert!(refresh_recently_failed("def50200aa", 1000 + 60));
	assert!(!refresh_recently_failed("def50200aa", 1000 + 601));
	assert!(!refresh_recently_failed("def50200bb", 1000 + 60));
	assert_ne!(fingerprint("a"), fingerprint("b"));
	remember_failed_refresh(None, 0);
	assert!(!refresh_recently_failed("def50200aa", 10));
}
