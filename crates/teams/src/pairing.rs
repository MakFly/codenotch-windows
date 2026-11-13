//! Pairing client: invite preview, pairing, account declaration, detach, erase. All POST,
//! all to the admitted [`Origin`] only; the invite code and the device token never go
//! anywhere else, and redirects are not followed (a 3xx is "not a Teams server").

use codenotch_core::{HttpClient, HttpRequest, HttpResponse, SecretValue, TeamsPayer};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::filter::Policy;
use crate::origin::{InviteCode, Origin};
use crate::{i18n, APP_VERSION};

/// Why a pairing call failed. Carries no value from the server or the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairError {
    /// Network failure (no HTTP status).
    Network,
    /// The invite is unknown, expired or used up (400/404/410 on an invite call).
    InvalidInvite,
    /// 401/403 on a device call: the token is no longer valid.
    Revoked,
    /// 409 `consent_required`.
    ConsentNeeded,
    /// 426: this app version is too old for the server.
    UpdateRequired,
    /// The origin does not answer like a Teams server (redirect, unreadable reply).
    NotATeamsServer,
    /// Any other status.
    Server(u16),
    /// Refused locally before any request (bad price, account not shared …).
    Refused,
}

impl PairError {
    /// English i18n key for the settings window.
    pub fn message(self) -> &'static str {
        match self {
            PairError::Revoked => i18n::REVOKED,
            PairError::ConsentNeeded => i18n::CONSENT,
            PairError::UpdateRequired => i18n::UPDATE,
            PairError::NotATeamsServer => i18n::NOT_A_SERVER,
            PairError::Network
            | PairError::InvalidInvite
            | PairError::Server(_)
            | PairError::Refused => i18n::PAUSED,
        }
    }
}

/// What an invite shows before the member accepts it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvitePreview {
    pub team_name: String,
    /// What the team sees, as the server names it (`named` in version 1).
    pub visibility: String,
    pub consent_version: u32,
}

/// Result of a successful pairing. `device_token` goes to the Credential Manager.
#[derive(Debug, Clone)]
pub struct Paired {
    pub device_token: SecretValue,
    pub team_id: String,
    pub team_name: String,
    pub data_epoch: u32,
    pub floor_at: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PreviewReply {
    team_name: String,
    visibility: String,
    consent_version: u32,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairReply {
    device_token: String,
    team_id: String,
    team_name: String,
    data_epoch: u32,
    floor_at: i64,
}

/// A fresh device id (lower-case uuid v4), made at pairing.
pub fn new_device_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// `POST /v1/invites/preview`.
pub fn preview(
    http: &dyn HttpClient,
    origin: &Origin,
    code: &InviteCode,
) -> Result<InvitePreview, PairError> {
    let body = json!({ "inviteCode": code.expose() });
    let reply = post(http, origin, "/v1/invites/preview", None, &body).map_err(invite_error)?;
    let r: PreviewReply = reply.json().map_err(|_| PairError::NotATeamsServer)?;
    Ok(InvitePreview {
        team_name: clean(&r.team_name),
        visibility: clean(&r.visibility),
        consent_version: r.consent_version,
    })
}

/// `POST /v1/devices/pair`, after the member confirmed the preview (`consent_version` is the
/// one the preview showed).
pub fn pair(
    http: &dyn HttpClient,
    origin: &Origin,
    code: &InviteCode,
    device_id: &str,
    consent_version: u32,
) -> Result<Paired, PairError> {
    let body = json!({
        "inviteCode": code.expose(),
        "deviceId": device_id,
        "appVersion": APP_VERSION,
        "consentVersion": consent_version,
    });
    let reply = post(http, origin, "/v1/devices/pair", None, &body).map_err(invite_error)?;
    let r: PairReply = reply.json().map_err(|_| PairError::NotATeamsServer)?;
    if r.device_token.trim().is_empty() || r.team_id.trim().is_empty() {
        return Err(PairError::NotATeamsServer);
    }
    Ok(Paired {
        device_token: SecretValue::new(r.device_token),
        team_id: r.team_id,
        team_name: clean(&r.team_name),
        data_epoch: r.data_epoch,
        floor_at: r.floor_at,
    })
}

/// `POST /v1/accounts/declare`: who pays for a shared account's plan and, when the member
/// pays, the monthly price they declare. Only the account reference, the payer and a number
/// leave; an account that is not shared, or a negative / non-finite price, is refused
/// before any request.
pub fn declare_account(
    http: &dyn HttpClient,
    origin: &Origin,
    token: &str,
    policy: &Policy,
    account_id: &str,
    payer: TeamsPayer,
    price_eur_month: Option<f64>,
) -> Result<(), PairError> {
    let shared = policy.account(account_id).ok_or(PairError::Refused)?;
    if price_eur_month.is_some_and(|p| !p.is_finite() || p < 0.0 || p > 100_000.0) {
        return Err(PairError::Refused);
    }
    let body = json!({
        "accountRef": shared.account_ref,
        "payer": match payer {
            TeamsPayer::Company => "company",
            TeamsPayer::Member => "member",
        },
        "priceEurMonth": price_eur_month,
    });
    post(http, origin, "/v1/accounts/declare", Some(token), &body).map(|_| ())
}

/// `POST /v1/devices/self/detach`. The caller deletes the local state whatever the result
/// (see [`crate::TeamsSync::detach_local`]).
pub fn detach(http: &dyn HttpClient, origin: &Origin, token: &str) -> Result<(), PairError> {
    post(
        http,
        origin,
        "/v1/devices/self/detach",
        Some(token),
        &json!({}),
    )
    .map(|_| ())
}

/// `POST /v1/members/self/erase`: erase this member's data on the server.
pub fn erase(http: &dyn HttpClient, origin: &Origin, token: &str) -> Result<(), PairError> {
    post(
        http,
        origin,
        "/v1/members/self/erase",
        Some(token),
        &json!({}),
    )
    .map(|_| ())
}

fn post(
    http: &dyn HttpClient,
    origin: &Origin,
    path: &str,
    token: Option<&str>,
    body: &Value,
) -> Result<HttpResponse, PairError> {
    let mut request = HttpRequest::post(origin.url(path)).json(body);
    if let Some(token) = token {
        request = request.bearer(token);
    }
    let reply = http.send(&request).map_err(|_| PairError::Network)?;
    match reply.status {
        200..=299 => Ok(reply),
        401 | 403 => Err(PairError::Revoked),
        409 if reply
            .json::<Value>()
            .is_ok_and(|v| v["error"] == "consent_required") =>
        {
            Err(PairError::ConsentNeeded)
        }
        426 => Err(PairError::UpdateRequired),
        300..=399 => Err(PairError::NotATeamsServer),
        status => Err(PairError::Server(status)),
    }
}

/// On invite calls, an unknown or expired code is an invalid invite, not a revocation.
fn invite_error(e: PairError) -> PairError {
    match e {
        PairError::Server(400 | 404 | 410) | PairError::Revoked => PairError::InvalidInvite,
        other => other,
    }
}

/// Server text shown in the UI: no control characters, at most 100 characters.
fn clean(text: &str) -> String {
    text.chars()
        .filter(|c| !c.is_control())
        .take(100)
        .collect::<String>()
        .trim()
        .to_string()
}
