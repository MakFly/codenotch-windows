//! Pairing client: invite preview, pairing, account declaration, detach, erase. All POST,
//! all to the admitted [`Origin`] only; the invite code and the device token never go
//! anywhere else, and redirects are not followed (a 3xx is "not a Teams server").
//!
//! Replies are held to a closed contract: the visibility is a value of a closed set, the team
//! id a lower-case uuid, the floor a checked date. The team name is the one free text kept: it
//! is shown in the settings window and stored in `teams.json`, never logged (`Debug` leaves it
//! out) and refused when it repeats the invite code or the device token.

use codenotch_core::{
    HttpClient, HttpRequest, HttpResponse, PlanCode, SecretValue, TeamsPayer, TeamsVisibility,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::filter::Policy;
use crate::origin::{InviteCode, Origin};
use crate::sync::raise_floor;
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

/// What an invite shows before the member accepts it. `Debug` leaves out the team name.
#[derive(Clone, PartialEq, Eq)]
pub struct InvitePreview {
    pub team_name: String,
    pub visibility: TeamsVisibility,
    pub consent_version: u32,
}

/// Result of a successful pairing. `device_token` goes to the Credential Manager. `Debug`
/// shows neither the token, the team id nor the team name.
#[derive(Clone)]
pub struct Paired {
    pub device_token: SecretValue,
    pub team_id: String,
    pub team_name: String,
    pub data_epoch: u32,
    /// First hour that may be sent: the server's floor, never before the local pairing time.
    pub floor_at: i64,
}

impl std::fmt::Debug for InvitePreview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InvitePreview")
            .field("visibility", &self.visibility)
            .field("consent_version", &self.consent_version)
            .finish_non_exhaustive()
    }
}

impl std::fmt::Debug for Paired {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Paired")
            .field("data_epoch", &self.data_epoch)
            .field("floor_at", &self.floor_at)
            .finish_non_exhaustive()
    }
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
    // A visibility outside the closed set cannot be shown to the member: a newer contract.
    let visibility = TeamsVisibility::parse(&r.visibility).ok_or(PairError::UpdateRequired)?;
    Ok(InvitePreview {
        team_name: team_name(&r.team_name, &[code.expose()])?,
        visibility,
        consent_version: r.consent_version,
    })
}

/// `POST /v1/devices/pair`, after the member confirmed the preview (`consent_version` is the
/// one the preview showed). `now` is the member's consent: no hour before it is ever sent
/// unless they also agree to the backfill, whatever `floorAt` the server returns.
pub fn pair(
    http: &dyn HttpClient,
    origin: &Origin,
    code: &InviteCode,
    device_id: &str,
    consent_version: u32,
    now: i64,
) -> Result<Paired, PairError> {
    let body = json!({
        "inviteCode": code.expose(),
        "deviceId": device_id,
        "appVersion": APP_VERSION,
        "consentVersion": consent_version,
    });
    let reply = post(http, origin, "/v1/devices/pair", None, &body).map_err(invite_error)?;
    let r: PairReply = reply.json().map_err(|_| PairError::NotATeamsServer)?;
    let token = r.device_token.trim();
    if token.is_empty() || !is_team_id(&r.team_id) || r.team_id == token {
        return Err(PairError::NotATeamsServer);
    }
    let floor_at = raise_floor(now, r.floor_at, now).ok_or(PairError::NotATeamsServer)?;
    Ok(Paired {
        team_name: team_name(&r.team_name, &[code.expose(), token])?,
        device_token: SecretValue::new(r.device_token),
        team_id: r.team_id,
        data_epoch: r.data_epoch,
        floor_at,
    })
}

/// What the member declares about one shared account.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Declaration {
    /// Closed plan code (`unknown` allowed).
    pub plan: PlanCode,
    pub payer: TeamsPayer,
    /// Monthly price the member declares when they pay (`None` = not declared).
    pub price_eur_month: Option<f64>,
}

/// `POST /v1/accounts/declare`: a shared account's plan (closed code, `unknown` allowed),
/// who pays for it and, when the member pays, the monthly price they declare. Only the
/// account reference, two closed codes and a number leave. An account that is not shared, a
/// plan of another provider, or a negative / non-finite price is refused before any request.
pub fn declare_account(
    http: &dyn HttpClient,
    origin: &Origin,
    token: &str,
    policy: &Policy,
    account_id: &str,
    declaration: &Declaration,
) -> Result<(), PairError> {
    let Declaration {
        plan,
        payer,
        price_eur_month,
    } = *declaration;
    let shared = policy.account(account_id).ok_or(PairError::Refused)?;
    if !plan.allowed_for(shared.provider) {
        return Err(PairError::Refused);
    }
    if price_eur_month.is_some_and(|p| !p.is_finite() || p < 0.0 || p > 100_000.0) {
        return Err(PairError::Refused);
    }
    let body = json!({
        "accountRef": shared.account_ref,
        "plan": plan.as_str(),
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

/// The team name to show and store: no control characters, at most 100 characters. Refused
/// when it repeats one of `secrets` (the invite code, the device token).
fn team_name(text: &str, secrets: &[&str]) -> Result<String, PairError> {
    let visible: String = text.chars().filter(|c| !c.is_control()).collect();
    if secrets.iter().any(|s| !s.is_empty() && visible.contains(s)) {
        return Err(PairError::NotATeamsServer);
    }
    Ok(visible
        .chars()
        .take(100)
        .collect::<String>()
        .trim()
        .to_string())
}

/// A team id: a lower-case hyphenated uuid, nothing else.
pub fn is_team_id(s: &str) -> bool {
    s.len() == 36
        && s.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_digit() || ('a'..='f').contains(&c),
        })
}
