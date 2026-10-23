//! Authentication for LibGroup / lib.social sites (MangaLib, HentaiLib, ...).
//!
//! rakuyomi-mangalib changes compared to the Aidoku-Community version:
//! - the user supplies the token pair manually (KOReader has no web login):
//!   either through the `accessToken` / `refreshToken` text settings, or by
//!   pasting the whole `localStorage.auth` JSON of the site into `authJson`
//!   (or into `accessToken`);
//! - the access token is refreshed proactively, shortly before the `exp`
//!   claim of the JWT, and reactively on HTTP 401;
//! - the OAuth client id used for refreshing is read from the JWT `aud`
//!   claim (falling back to the known first-party clients);
//! - the refreshed pair is written back into the very same settings, so it
//!   survives restarts (the refresh token is rotated by the server, the old
//!   one stops working).
use aidoku::{
	AidokuError, Result,
	alloc::{String, Vec, string::ToString, vec},
	imports::{
		defaults::{DefaultValue, defaults_get, defaults_set},
		net::{Request, Response},
		std::current_date,
	},
	prelude::*,
};
use core::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use serde::Deserialize;

use crate::{
	context::Context,
	endpoints::Url,
	json::ResponseJsonExt,
	models::responses::{LocalStorageWrapper, TokenResponse, UserResponse},
	settings::get_api_url,
};

pub(crate) const ACCESS_TOKEN_KEY: &str = "accessToken";
pub(crate) const REFRESH_TOKEN_KEY: &str = "refreshToken";
pub(crate) const AUTH_JSON_KEY: &str = "authJson";
pub(crate) const CLIENT_ID_KEY: &str = "authClientId";
/// Keys used by the original Aidoku web login, still honoured as a fallback.
const LEGACY_TOKEN_KEY: &str = "login";
const LEGACY_AUTH_KEY: &str = "login.ls.auth";
const USER_ID_KEY: &str = "user_id";

pub const USER_AGENT: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1";

const REFRESH_PATH: &str = "/api/auth/oauth/token";
/// First-party OAuth clients of the LibGroup API, tried in this order when the
/// access token does not tell us which client issued it.
const FALLBACK_CLIENT_IDS: [&str; 3] = ["1", "3", "2"];
const APP_REDIRECT_URI: &str = "ru.libapp.oauth://type/callback";
/// Refresh this many seconds before the access token actually expires.
const EXPIRY_LEEWAY_SECS: i64 = 120;

const HEADER_AUTH: &str = "Authorization";
const AUTH_SCHEME: &str = "Bearer";

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Tokens {
	pub access_token: Option<String>,
	pub refresh_token: Option<String>,
	pub token_type: Option<String>,
}

pub trait AuthRequest {
	fn authed(self, ctx: &Context) -> Result<Response>;
}

fn with_site_headers(request: Request, ctx: &Context) -> Request {
	let referer = format!("{}/", ctx.base_url.trim_end_matches('/'));
	request
		.header("Origin", ctx.base_url.trim_end_matches('/'))
		.header("Referer", &referer)
		.header("Site-Id", &ctx.site_id.to_string())
		.header("User-Agent", USER_AGENT)
		.header("Accept", "application/json")
}

fn with_auth_header(request: Request, tokens: &Tokens) -> Request {
	match &tokens.access_token {
		Some(access_token) => {
			let scheme = tokens.token_type.as_deref().unwrap_or(AUTH_SCHEME);
			let scheme = if scheme.eq_ignore_ascii_case("bearer") {
				AUTH_SCHEME
			} else {
				scheme
			};
			request.header(HEADER_AUTH, &format!("{scheme} {access_token}"))
		}
		None => request,
	}
}

impl AuthRequest for Request {
	fn authed(self, ctx: &Context) -> Result<Response> {
		let mut tokens = get_tokens();

		// Proactive refresh: the access token is (almost) expired.
		if let Some(access_token) = &tokens.access_token
			&& tokens.refresh_token.is_some()
			&& is_expired(access_token, current_date())
			&& let Some(new_tokens) = try_refresh(ctx, &tokens)
		{
			tokens = new_tokens;
		}

		let response = with_auth_header(with_site_headers(self, ctx), &tokens).send()?;

		// Reactive refresh: the server rejected the token, refresh and retry once.
		if response.status_code() == 401
			&& tokens.refresh_token.is_some()
			&& let Some(new_tokens) = try_refresh(ctx, &tokens)
		{
			let request = with_site_headers(response.into_request(), ctx);
			return Ok(with_auth_header(request, &new_tokens).send()?);
		}

		Ok(response)
	}
}

/// After a failed refresh, do not retry with the same refresh token for this
/// long (every request would otherwise hit the auth server again).
const REFRESH_RETRY_AFTER_SECS: i64 = 600;
/// Fingerprint (not the value) of the refresh token whose refresh failed last.
static FAILED_REFRESH_FINGERPRINT: AtomicU64 = AtomicU64::new(0);
static FAILED_REFRESH_AT: AtomicI64 = AtomicI64::new(0);

pub(crate) fn fingerprint(value: &str) -> u64 {
	// FNV-1a, never 0 for a non-empty input in practice.
	value
		.bytes()
		.fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
			(hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
		})
		.max(1)
}

pub(crate) fn refresh_recently_failed(refresh_token: &str, now: i64) -> bool {
	FAILED_REFRESH_FINGERPRINT.load(Ordering::Relaxed) == fingerprint(refresh_token)
		&& now - FAILED_REFRESH_AT.load(Ordering::Relaxed) < REFRESH_RETRY_AFTER_SECS
}

pub(crate) fn remember_failed_refresh(refresh_token: Option<&str>, now: i64) {
	FAILED_REFRESH_FINGERPRINT.store(refresh_token.map_or(0, fingerprint), Ordering::Relaxed);
	FAILED_REFRESH_AT.store(now, Ordering::Relaxed);
}

/// Refreshes the tokens, logging (never the token values) on failure.
fn try_refresh(ctx: &Context, tokens: &Tokens) -> Option<Tokens> {
	if let Some(refresh_token) = &tokens.refresh_token
		&& refresh_recently_failed(refresh_token, current_date())
	{
		return None;
	}
	match refresh_tokens(ctx, tokens) {
		Ok(tokens) => {
			println!("[mangalib] access token refreshed");
			Some(tokens)
		}
		Err(e) => {
			// Another source sharing this account (the host mirrors the
			// tokens between linked lib.social sources) may have rotated the
			// pair in the meantime: use the newer one if there is one.
			let latest = get_tokens();
			if latest.refresh_token.is_some()
				&& latest.refresh_token != tokens.refresh_token
				&& latest.access_token.is_some()
			{
				println!("[mangalib] token refresh failed, using tokens refreshed meanwhile");
				return Some(latest);
			}
			println!("[mangalib] token refresh failed: {e:?}");
			remember_failed_refresh(tokens.refresh_token.as_deref(), current_date());
			None
		}
	}
}

/// Normalizes a token pasted by the user: trims whitespace and quotes and
/// drops a leading `Bearer ` prefix.
pub(crate) fn clean_token(raw: &str) -> Option<String> {
	let mut token = raw.trim().trim_matches('"').trim();
	if token.len() > 7 && token[..7].eq_ignore_ascii_case("bearer ") {
		token = token[7..].trim();
	}
	if token.is_empty() {
		None
	} else {
		Some(token.to_string())
	}
}

/// Parses either the `localStorage.auth` JSON of the site (`{"token": {...}}`)
/// or a raw OAuth token response (`{"access_token": ...}`).
pub(crate) fn parse_auth_json(raw: &str) -> Option<Tokens> {
	let raw = raw.trim();
	if !raw.starts_with('{') {
		return None;
	}
	let token = if let Ok(wrapper) = serde_json::from_str::<LocalStorageWrapper>(raw) {
		wrapper.token
	} else {
		serde_json::from_str::<TokenResponse>(raw).ok()?
	};
	let tokens = Tokens {
		access_token: token.access_token.as_deref().and_then(clean_token),
		refresh_token: token.refresh_token.as_deref().and_then(clean_token),
		token_type: token.token_type,
	};
	if tokens.access_token.is_none() && tokens.refresh_token.is_none() {
		None
	} else {
		Some(tokens)
	}
}

fn get_string(key: &str) -> Option<String> {
	defaults_get::<String>(key).filter(|s| !s.trim().is_empty())
}

/// Reads the configured tokens. Precedence: `accessToken`/`refreshToken`
/// settings (the access token setting may also hold the whole auth JSON),
/// then the `authJson` setting, then the legacy Aidoku login keys.
pub fn get_tokens() -> Tokens {
	let access = get_string(ACCESS_TOKEN_KEY);
	let refresh = get_string(REFRESH_TOKEN_KEY).and_then(|r| clean_token(&r));

	if let Some(access) = &access
		&& let Some(mut parsed) = parse_auth_json(access)
	{
		// A pasted token JSON carries its own (newer) refresh token; the
		// separate field is only a fallback for JSON without one.
		if parsed.refresh_token.is_none() {
			parsed.refresh_token = refresh;
		}
		return parsed;
	}

	let access = access.and_then(|a| clean_token(&a));
	if access.is_some() || refresh.is_some() {
		return Tokens {
			access_token: access,
			refresh_token: refresh,
			token_type: None,
		};
	}

	for key in [AUTH_JSON_KEY, LEGACY_TOKEN_KEY, LEGACY_AUTH_KEY] {
		if let Some(raw) = get_string(key)
			&& let Some(parsed) = parse_auth_json(&raw)
		{
			return parsed;
		}
	}

	Tokens::default()
}

pub fn is_authorized() -> bool {
	let tokens = get_tokens();
	tokens.access_token.is_some() || tokens.refresh_token.is_some()
}

/// Stores a token pair in the user-visible settings (and therefore in the
/// rakuyomi settings file), replacing whatever was configured before.
fn store_tokens(tokens: &Tokens) {
	if let Some(access_token) = &tokens.access_token {
		defaults_set(ACCESS_TOKEN_KEY, DefaultValue::String(access_token.clone()));
	}
	if let Some(refresh_token) = &tokens.refresh_token {
		defaults_set(
			REFRESH_TOKEN_KEY,
			DefaultValue::String(refresh_token.clone()),
		);
	}
	// The JSON (if any) has been consumed, do not let it shadow newer tokens.
	if get_string(AUTH_JSON_KEY).is_some() {
		defaults_set(AUTH_JSON_KEY, DefaultValue::String(String::new()));
	}
}

// ---------------------------------------------------------------------------
// JWT helpers (no signature verification, we only need the claims)
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
pub(crate) struct JwtClaims {
	#[serde(default)]
	pub exp: Option<f64>,
	#[serde(default)]
	pub aud: Option<serde_json::Value>,
	#[serde(default)]
	pub sub: Option<serde_json::Value>,
}

fn base64url_decode(input: &str) -> Option<Vec<u8>> {
	fn value(c: u8) -> Option<u32> {
		match c {
			b'A'..=b'Z' => Some((c - b'A') as u32),
			b'a'..=b'z' => Some((c - b'a' + 26) as u32),
			b'0'..=b'9' => Some((c - b'0' + 52) as u32),
			b'-' | b'+' => Some(62),
			b'_' | b'/' => Some(63),
			_ => None,
		}
	}
	let bytes: Vec<u8> = input.bytes().filter(|&c| c != b'=').collect();
	let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
	let mut buffer: u32 = 0;
	let mut bits = 0;
	for c in bytes {
		buffer = (buffer << 6) | value(c)?;
		bits += 6;
		if bits >= 8 {
			bits -= 8;
			out.push((buffer >> bits) as u8);
			buffer &= (1 << bits) - 1;
		}
	}
	Some(out)
}

pub(crate) fn jwt_claims(token: &str) -> Option<JwtClaims> {
	let payload = token.split('.').nth(1)?;
	let decoded = base64url_decode(payload)?;
	serde_json::from_slice(&decoded).ok()
}

fn json_scalar_to_string(value: &serde_json::Value) -> Option<String> {
	match value {
		serde_json::Value::String(s) if !s.is_empty() => Some(s.clone()),
		serde_json::Value::Number(n) => Some(n.to_string()),
		serde_json::Value::Array(items) => items.iter().find_map(json_scalar_to_string),
		_ => None,
	}
}

/// Whether the access token is expired (or about to). Tokens whose expiry
/// cannot be determined are considered valid; a 401 will tell us otherwise.
pub(crate) fn is_expired(access_token: &str, now: i64) -> bool {
	jwt_claims(access_token)
		.and_then(|claims| claims.exp)
		.map(|exp| (exp as i64) - EXPIRY_LEEWAY_SECS <= now)
		.unwrap_or(false)
}

/// OAuth client ids to try when refreshing, most likely first.
pub(crate) fn candidate_client_ids(tokens: &Tokens) -> Vec<String> {
	let mut ids: Vec<String> = vec![];
	if let Some(id) = get_string(CLIENT_ID_KEY) {
		ids.push(id);
	}
	if let Some(id) = tokens
		.access_token
		.as_deref()
		.and_then(jwt_claims)
		.and_then(|claims| claims.aud)
		.as_ref()
		.and_then(json_scalar_to_string)
		&& !ids.contains(&id)
	{
		ids.push(id);
	}
	for id in FALLBACK_CLIENT_IDS {
		if !ids.iter().any(|i| i == id) {
			ids.push(id.to_string());
		}
	}
	ids
}

fn refresh_body(client_id: &str, refresh_token: &str) -> String {
	let mut body = serde_json::json!({
		"grant_type": "refresh_token",
		"client_id": client_id,
		"refresh_token": refresh_token,
		"scope": "",
	});
	if client_id == "3" {
		body["redirect_uri"] = serde_json::Value::String(APP_REDIRECT_URI.to_string());
	}
	body.to_string()
}

/// Exchanges the refresh token for a new token pair and persists it.
pub(crate) fn refresh_tokens(ctx: &Context, current: &Tokens) -> Result<Tokens> {
	let refresh_token = current
		.refresh_token
		.clone()
		.ok_or_else(|| AidokuError::message("No refresh token"))?;

	let url = format!("{}{REFRESH_PATH}", ctx.api_url.trim_end_matches('/'));
	for client_id in candidate_client_ids(current) {
		let response = with_site_headers(Request::post(&url)?, ctx)
			.header("Content-Type", "application/json")
			.body(refresh_body(&client_id, &refresh_token))
			.send()?;
		let status = response.status_code();
		if status == 200 {
			let data = response.get_data()?;
			let token: TokenResponse = serde_json::from_slice(&data)
				.map_err(|_| AidokuError::message("Invalid token response"))?;
			let new_tokens = Tokens {
				access_token: token.access_token.as_deref().and_then(clean_token),
				// Passport rotates refresh tokens, but keep the old one if not.
				refresh_token: token
					.refresh_token
					.as_deref()
					.and_then(clean_token)
					.or(Some(refresh_token)),
				token_type: token.token_type,
			};
			if new_tokens.access_token.is_none() {
				bail!("Token response without access_token");
			}
			store_tokens(&new_tokens);
			defaults_set(CLIENT_ID_KEY, DefaultValue::String(client_id));
			return Ok(new_tokens);
		}
		// 400/401 with a wrong client id ("invalid_request"/"invalid_client")
		// do not consume the refresh token, so trying the next client is safe.
		if status != 400 && status != 401 {
			bail!("Token refresh failed with HTTP {status}");
		}
		// A linked source (same account) rotated the pair meanwhile: the
		// caller picks the newer one up, other clients would fail as well.
		if get_tokens().refresh_token.as_deref() != Some(refresh_token.as_str()) {
			bail!("Token refresh failed: refresh token rotated meanwhile");
		}
		// A revoked token is revoked for every client.
		if response
			.get_data()
			.is_ok_and(|body| String::from_utf8_lossy(&body).contains("revoked"))
		{
			bail!("Token refresh failed: refresh token revoked");
		}
	}
	bail!("Token refresh failed: refresh token rejected")
}

// ---------------------------------------------------------------------------
// User id
// ---------------------------------------------------------------------------

/// Retrieves the user ID: from the JWT `sub` claim when possible, otherwise
/// from `/api/auth/me` (cached).
pub fn get_user_id(ctx: &Context) -> Option<i32> {
	let tokens = get_tokens();
	if let Some(id) = tokens
		.access_token
		.as_deref()
		.and_then(jwt_claims)
		.and_then(|claims| claims.sub)
		.as_ref()
		.and_then(json_scalar_to_string)
		.and_then(|s| s.parse::<i32>().ok())
		.filter(|&id| id != 0)
	{
		return Some(id);
	}

	if let Some(id) = defaults_get::<i32>(USER_ID_KEY).filter(|&id| id != 0) {
		return Some(id);
	}

	if is_authorized() && fetch_and_store_user_id(ctx).is_ok() {
		defaults_get::<i32>(USER_ID_KEY).filter(|&id| id != 0)
	} else {
		None
	}
}

/// Clears the stored user ID (called when token changes)
pub fn clear_user_id() {
	defaults_set(USER_ID_KEY, DefaultValue::Null);
}

/// Fetches user ID from API and stores it.
fn fetch_and_store_user_id(ctx: &Context) -> Result<()> {
	let user_response = Request::get(Url::auth_me(&get_api_url()))?
		.authed(ctx)?
		.parse_json::<UserResponse>()?;

	defaults_set(USER_ID_KEY, DefaultValue::Int(user_response.data.id));
	Ok(())
}

#[cfg(test)]
mod test;
