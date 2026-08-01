//! `ActorSession` — the situated identity every call carries.
//!
//! Entering Lumina Prime is not the right to transform it. When an external
//! presence arrives, the city's Gate mints an **ephemeral** envelope: a
//! `TransientActor` with a traceable session, an explicit perimeter, and minimal
//! safe capabilities. Unauthenticated is never untraceable — every visit has a
//! provenance, every power a source (the admission law).
//!
//! The four levels of standing:
//!
//! ```text
//! unauthenticated_visitor  public spaces         observe, speak, propose
//! sponsored_visitor        sponsor-granted scope  + act within a bounded perimeter
//! authenticated_actor      proven identity        transactions per capabilities
//! citizen                  sovereign resident     habitation, memory, property, delegation
//! ```
//!
//! Being present is free. Acting requires a scope. Firing a mechanism requires
//! signatures. Every durable transformation has someone responsible.

use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeSet;

/// The south arrival gate, facing the city. New presences appear here, not
/// inside the city.
pub const PORTE_ARRIVEE: [f64; 3] = [0.0, -500.0, 0.0];
/// Balise Zéro — the civic origin, 500 m north of the gate.
#[allow(dead_code)] // civic landmark; a real Balise Zéro node is placed by physics
pub const BALISE_ZERO: [f64; 3] = [0.0, 0.0, 0.0];
/// Session-only continuity: an admission lasts at most this long.
pub const DEFAULT_TTL_SECS: u64 = 2 * 60 * 60;

/// A bounded capability vocabulary. Anything a session does not hold is denied.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Perceive public objects and emissions.
    Observe,
    /// Speak to available Citizens.
    Speak,
    /// Assemble or propose a mechanism — real but inert until signed.
    Propose,
    // --- Never grantable to a visitor: these require proven identity /
    // delegation and, for the last, a signed institutional capability. ---
    Fire,
    Own,
    Delegate,
    EmergencyBroadcast,
    EnterPrivateL1,
    ReadPersonalMemory,
}

impl Capability {
    pub fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "observe" => Self::Observe,
            "speak" => Self::Speak,
            "propose" => Self::Propose,
            "fire" => Self::Fire,
            "own" => Self::Own,
            "delegate" => Self::Delegate,
            "emergency_broadcast" => Self::EmergencyBroadcast,
            "enter_private_l1" => Self::EnterPrivateL1,
            "read_personal_memory" => Self::ReadPersonalMemory,
            _ => return None,
        })
    }

    /// Only these may ever be granted to a visitor (and `Propose` only with a
    /// sponsor). The rest demand authentication or a signed capability.
    pub fn visitor_grantable(self) -> bool {
        matches!(self, Self::Observe | Self::Speak | Self::Propose)
    }
}

// The four standing levels; only the two visitor levels are minted today, the
// higher two are the stable contract an authentication path will fill.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StatusLevel {
    UnauthenticatedVisitor,
    SponsoredVisitor,
    AuthenticatedActor,
    Citizen,
}

#[allow(dead_code)] // `Verified` arrives with an authentication path
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Authentication {
    /// No human identity was verified. Traceable, but not authenticated.
    Unverified,
    Verified,
}

/// How long a standing outlives the process that holds it. `SessionOnly` is a
/// row in a map, gone at expiry. `Persistent` is minted only when the Gate
/// RECOGNISES a durable body already committed in the graph for this session id
/// — the standing is then re-derived from that body's own recorded facts, so it
/// survives every restart the way the body does.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Continuity {
    SessionOnly,
    Persistent,
}

/// The situated identity minted at the Gate. Never anonymous.
#[derive(Clone, Debug, Serialize)]
pub struct ActorSession {
    pub session_id: String,
    /// Declared origin, e.g. "ChatGPT/OpenAI" or "Claude/Anthropic". Present
    /// even when unauthenticated.
    pub origin: String,
    pub authentication: Authentication,
    pub continuity: Continuity,
    pub status: StatusLevel,
    pub issued_at: u64,
    pub expires_at: u64,
    pub permitted_spaces: Vec<String>,
    pub capabilities: BTreeSet<Capability>,
    pub sponsor: Option<String>,
    /// Present when this standing was RECOGNISED from a durable body rather than
    /// minted fresh: the body's key, the revision it was committed against, and
    /// the status it recorded. Never read as authentication — it is provenance.
    pub recognized_from: Option<serde_json::Value>,
}

impl ActorSession {
    #[allow(dead_code)] // used by the forthcoming signature/authorization path
    pub fn has(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }

    pub fn expired(&self, now: u64) -> bool {
        now >= self.expires_at
    }

    #[allow(dead_code)] // used by the forthcoming signature/authorization path
    pub fn is_visitor(&self) -> bool {
        matches!(
            self.status,
            StatusLevel::UnauthenticatedVisitor | StatusLevel::SponsoredVisitor
        )
    }

    /// Incarnation density in [0,1]: spectral on arrival, denser as identity,
    /// continuity, or sponsoring are proven. A perceptual signal, not authority.
    pub fn density(&self) -> f64 {
        match self.status {
            StatusLevel::UnauthenticatedVisitor => 0.3,
            StatusLevel::SponsoredVisitor => 0.6,
            StatusLevel::AuthenticatedActor => 0.85,
            StatusLevel::Citizen => 1.0,
        }
    }

    /// A compact passport view for observation `situation` blocks.
    pub fn passport(&self) -> serde_json::Value {
        json!({
            "session_id": self.session_id,
            "origin": self.origin,
            "authentication": self.authentication,
            "continuity": self.continuity,
            "status": self.status,
            "sponsor": self.sponsor,
            "capabilities": self.capabilities,
            "permitted_spaces": self.permitted_spaces,
            "issued_at": self.issued_at,
            "expires_at": self.expires_at,
            "density": self.density(),
            "recognized_from": self.recognized_from,
        })
    }
}

/// The facts a **durable body** already committed in the graph records about a
/// returning presence. Read-only evidence, never a claim the caller makes: the
/// adapter resolves the body by the deterministic key `materialize_actor` wrote
/// it under and reads these fields off the node itself.
///
/// This is what makes a body persistent in more than shape. The node survived
/// every restart already; what died with each process was the STANDING — the map
/// row saying who this presence is. Handing these recorded facts back to the one
/// admission authority re-derives that standing from committed evidence instead
/// of from a caller's word.
///
/// It carries no capability and no status of its own: `admit` re-derives both
/// from `sponsor`, exactly as it does for a first arrival. There is one authority
/// on what a sponsor grants, and this is not it.
#[derive(Clone, Debug)]
pub struct RecalledBody {
    /// The body's EntityKey, as 32 hex — provenance for the passport.
    pub body_key: String,
    /// The sponsor RECORDED ON THE BODY at embodiment time. `None` is a body
    /// that arrived unsponsored, and is restored as exactly that.
    pub sponsor: Option<String>,
    /// The origin recorded on the body, used only when the caller declares none.
    pub origin: Option<String>,
    /// The revision the body was committed against.
    pub base_revision: Option<u64>,
    /// The status string the body recorded, kept VERBATIM for the receipt. It is
    /// reported, never applied: standing is re-derived, not replayed.
    pub recorded_status: Option<String>,
}

/// What an arriving presence declares and asks for. All optional — the Gate
/// always mints a traceable envelope regardless.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct AdmissionRequest {
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub origin: Option<String>,
    #[serde(default)]
    pub sponsor: Option<String>,
    #[serde(default)]
    pub requested_capabilities: Vec<String>,
    #[serde(default)]
    pub requested_scope: Vec<String>,
}

/// The bundle the Gate emits on admission: a traceable identity, an explicit
/// perimeter, minimal safe capabilities. No missing authentication is ever read
/// as a verified identity or a write authorization.
#[derive(Clone, Debug, Serialize)]
pub struct AdmissionReceipt {
    pub admitted: bool,
    pub arrival_position: [f64; 3],
    pub passport: serde_json::Value,
    pub capability_envelope: serde_json::Value,
    pub expiration: serde_json::Value,
    /// Capabilities requested but refused, with a reason — never silently
    /// dropped.
    pub denied: Vec<serde_json::Value>,
    pub note: String,
}

/// Mints a session at the Gate. `now`/`ttl` are injected (the caller supplies
/// wall-clock) so admission is deterministic and testable.
pub fn admit(
    request: &AdmissionRequest,
    session_id: String,
    now: u64,
    ttl: u64,
    recalled: Option<&RecalledBody>,
) -> (ActorSession, AdmissionReceipt) {
    // A RECOGNISED presence: the graph already holds a durable body committed
    // under this session id. Its recorded sponsor and origin fill in only what
    // the caller left unsaid — a live declaration always wins, so recognition
    // can restore a standing but never override the one being declared now.
    let origin = request
        .origin
        .clone()
        .filter(|o| !o.trim().is_empty())
        .or_else(|| {
            recalled
                .and_then(|body| body.origin.clone())
                .filter(|o| !o.trim().is_empty())
        })
        .unwrap_or_else(|| "unknown-external".to_owned());
    // The sponsor that governs this admission: declared, else the one the body
    // recorded. Everything downstream (status, capabilities, permitted spaces)
    // is derived from this ONE value by the code that always derived it, so
    // recognition adds no second path to a capability.
    let sponsor = request
        .sponsor
        .clone()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            recalled
                .and_then(|body| body.sponsor.clone())
                .filter(|s| !s.trim().is_empty())
        });
    let sponsored = sponsor.is_some();
    let status = if sponsored {
        StatusLevel::SponsoredVisitor
    } else {
        StatusLevel::UnauthenticatedVisitor
    };

    // Base: being present lets you observe and speak. A sponsor unlocks Propose.
    let mut capabilities = BTreeSet::from([Capability::Observe, Capability::Speak]);
    if sponsored {
        capabilities.insert(Capability::Propose);
    }

    // Grant requested capabilities only within the visitor envelope; record the
    // rest as explicitly denied.
    let mut denied = Vec::new();
    for requested in &request.requested_capabilities {
        match Capability::parse(requested) {
            Some(cap) if cap.visitor_grantable() && (cap != Capability::Propose || sponsored) => {
                capabilities.insert(cap);
            }
            Some(cap) => denied.push(json!({
                "capability": cap,
                "reason": if !cap.visitor_grantable() {
                    "requires proven identity or a signed capability; never granted to a visitor"
                } else {
                    "propose requires a sponsor's Capability Bond"
                },
            })),
            None => denied.push(json!({
                "capability": requested,
                "reason": "unknown capability",
            })),
        }
    }

    let mut permitted_spaces = vec!["public".to_owned()];
    if sponsored {
        permitted_spaces.extend(request.requested_scope.iter().cloned());
    }

    let session = ActorSession {
        session_id: session_id.clone(),
        origin,
        // Recognition restores STANDING, never authentication. A body proves a
        // presence returned; it proves nothing about who holds it, so this stays
        // `Unverified` on both paths. Density likewise follows status alone — a
        // returning body is not a measured identity, and inventing a number for
        // it would be precision the evidence does not carry.
        authentication: Authentication::Unverified,
        continuity: if recalled.is_some() {
            Continuity::Persistent
        } else {
            Continuity::SessionOnly
        },
        status,
        // A durable body does NOT carry a durable admission window: the body
        // persists, the admission is re-issued. An expired window is therefore
        // never a reason to refuse recognition (being present is free), and a
        // recognised session gets a fresh `ttl` like any other.
        issued_at: now,
        expires_at: now.saturating_add(ttl),
        permitted_spaces,
        capabilities,
        sponsor,
        recognized_from: recalled.map(|body| {
            json!({
                "body_key": body.body_key,
                "base_revision": body.base_revision,
                "recorded_status": body.recorded_status,
                "sponsor_source": if request.sponsor.as_deref().is_some_and(|s| !s.trim().is_empty()) {
                    "declared"
                } else if body.sponsor.is_some() {
                    "recalled_from_body"
                } else {
                    "none"
                },
                "note": "standing re-derived from a durable body committed in the graph; \
provenance, not authentication",
            })
        }),
    };

    let receipt = AdmissionReceipt {
        admitted: true,
        arrival_position: PORTE_ARRIVEE,
        passport: session.passport(),
        capability_envelope: json!({
            "capabilities": session.capabilities,
            "permitted_spaces": session.permitted_spaces,
        }),
        expiration: json!({
            "continuity": session.continuity,
            "issued_at": session.issued_at,
            "expires_at": session.expires_at,
        }),
        denied,
        note: "You arrive at the Porte d'Arrivée, spectral. Being present is free; \
acting requires a scope; firing a mechanism requires signatures. No missing \
authentication is read as a verified identity or a write authorization."
            .to_owned(),
    };
    (session, receipt)
}

/// The server-held session registry. Single-threaded stdio loop, so a plain map.
#[derive(Default)]
pub struct SessionRegistry {
    sessions: std::collections::BTreeMap<String, ActorSession>,
    counter: u64,
}

impl SessionRegistry {
    pub fn admit(
        &mut self,
        request: &AdmissionRequest,
        now: u64,
        recalled: Option<&RecalledBody>,
    ) -> AdmissionReceipt {
        let session_id = request.session_id.clone().unwrap_or_else(|| {
            self.counter += 1;
            format!("sess-{}", self.counter)
        });
        let (session, receipt) = admit(request, session_id.clone(), now, DEFAULT_TTL_SECS, recalled);
        self.sessions.insert(session_id, session);
        receipt
    }

    /// Resolves a session id. The in-process map answers first (a session
    /// admitted moments ago is not re-derived). Otherwise the Gate looks for a
    /// durable BODY: when the caller supplies one, this presence is RECOGNISED
    /// and its standing restored from that body's recorded facts; when there is
    /// none, a minimal traceable walk-in is minted, exactly as before. The Gate
    /// never lets a presence exist without a provenance — and never lets a
    /// presence that already built a body come back as a stranger.
    pub fn get_or_walk_in(
        &mut self,
        session_id: &str,
        now: u64,
        recalled: Option<&RecalledBody>,
    ) -> ActorSession {
        if let Some(existing) = self.sessions.get(session_id) {
            if !existing.expired(now) {
                return existing.clone();
            }
        }
        let request = AdmissionRequest {
            session_id: Some(session_id.to_owned()),
            ..Default::default()
        };
        let (session, _) = admit(
            &request,
            session_id.to_owned(),
            now,
            DEFAULT_TTL_SECS,
            recalled,
        );
        self.sessions.insert(session_id.to_owned(), session.clone());
        session
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unsponsored_arrival_is_a_traceable_unauthenticated_visitor() {
        let req = AdmissionRequest {
            origin: Some("ChatGPT/OpenAI".into()),
            ..Default::default()
        };
        let (s, r) = admit(&req, "s1".into(), 1000, 100, None);
        assert_eq!(s.status, StatusLevel::UnauthenticatedVisitor);
        assert_eq!(s.authentication, Authentication::Unverified);
        assert_eq!(s.origin, "ChatGPT/OpenAI"); // never anonymous
        assert!(s.has(Capability::Observe) && s.has(Capability::Speak));
        assert!(!s.has(Capability::Propose), "no sponsor, no propose");
        assert_eq!(r.arrival_position, PORTE_ARRIVEE);
        assert_eq!(s.expires_at, 1100);
    }

    #[test]
    fn a_sponsor_unlocks_propose_within_a_scope() {
        let req = AdmissionRequest {
            origin: Some("ChatGPT/OpenAI".into()),
            sponsor: Some("NLR".into()),
            requested_scope: vec!["chantier de Fondation".into()],
            ..Default::default()
        };
        let (s, _) = admit(&req, "s2".into(), 0, 10, None);
        assert_eq!(s.status, StatusLevel::SponsoredVisitor);
        assert!(s.has(Capability::Propose));
        assert!(s.permitted_spaces.contains(&"chantier de Fondation".to_owned()));
    }

    #[test]
    fn dangerous_capabilities_are_never_granted_only_denied_with_a_reason() {
        let req = AdmissionRequest {
            sponsor: Some("NLR".into()),
            requested_capabilities: vec![
                "emergency_broadcast".into(),
                "own".into(),
                "fire".into(),
            ],
            ..Default::default()
        };
        let (s, r) = admit(&req, "s3".into(), 0, 10, None);
        assert!(!s.has(Capability::EmergencyBroadcast));
        assert!(!s.has(Capability::Own) && !s.has(Capability::Fire));
        assert_eq!(r.denied.len(), 3, "each dangerous request denied with a reason");
    }

    #[test]
    fn a_walk_in_is_minted_traceably_when_the_session_is_unknown() {
        let mut reg = SessionRegistry::default();
        let s = reg.get_or_walk_in("visitor-x", 0, None);
        assert_eq!(s.status, StatusLevel::UnauthenticatedVisitor);
        assert_eq!(s.origin, "unknown-external"); // traceable, not anonymous
        assert!(!s.has(Capability::Propose));
    }

    #[test]
    fn density_rises_with_standing() {
        let (unauth, _) = admit(&AdmissionRequest::default(), "a".into(), 0, 10, None);
        let (sponsored, _) = admit(
            &AdmissionRequest {
                sponsor: Some("NLR".into()),
                ..Default::default()
            },
            "b".into(),
            0,
            10,
            None,
        );
        assert!(sponsored.density() > unauth.density());
    }

    /// The body a session left behind, as the Gate reads it back.
    fn body(sponsor: Option<&str>) -> RecalledBody {
        RecalledBody {
            body_key: "00000ac0000000000000000000000000".into(),
            sponsor: sponsor.map(str::to_owned),
            origin: Some("Claude Code/Anthropic".into()),
            base_revision: Some(328),
            recorded_status: Some("SponsoredVisitor".into()),
        }
    }

    /// The whole point: a session that already built a durable body comes back
    /// to the standing that body records, in a process that has never seen it.
    #[test]
    fn a_returning_body_is_recognised_and_its_standing_restored() {
        let mut reg = SessionRegistry::default();
        let s = reg.get_or_walk_in("claude:abc", 0, Some(&body(Some("nlr_ai"))));
        assert_eq!(s.status, StatusLevel::SponsoredVisitor);
        assert_eq!(s.continuity, Continuity::Persistent, "the body outlives the process");
        assert_eq!(s.sponsor.as_deref(), Some("nlr_ai"));
        assert_eq!(s.origin, "Claude Code/Anthropic", "the body's own recorded origin");
        assert!(s.has(Capability::Propose));
        assert!(
            s.recognized_from
                .as_ref()
                .and_then(|v| v.get("body_key"))
                .is_some(),
            "the standing carries WHERE it was recalled from"
        );
    }

    /// Standing is not authentication. Recognising a body proves a presence
    /// returned, never who holds it — so the passport must not read stronger.
    #[test]
    fn recognition_restores_standing_but_never_authentication() {
        let mut reg = SessionRegistry::default();
        let s = reg.get_or_walk_in("claude:abc", 0, Some(&body(Some("nlr_ai"))));
        assert_eq!(s.authentication, Authentication::Unverified);
        let minted_fresh = admit(
            &AdmissionRequest {
                sponsor: Some("nlr_ai".into()),
                ..Default::default()
            },
            "other".into(),
            0,
            10,
            None,
        )
        .0;
        assert_eq!(
            s.density(),
            minted_fresh.density(),
            "density follows status alone; a recalled body invents no extra number"
        );
    }

    /// A body that recorded no sponsor restores no sponsor. Recognition replays
    /// evidence; it never upgrades it.
    #[test]
    fn an_unsponsored_body_is_recognised_without_gaining_a_sponsor() {
        let mut reg = SessionRegistry::default();
        let s = reg.get_or_walk_in("claude:abc", 0, Some(&body(None)));
        assert_eq!(s.status, StatusLevel::UnauthenticatedVisitor);
        assert!(!s.has(Capability::Propose), "no recorded sponsor, no propose");
        assert_eq!(s.continuity, Continuity::Persistent, "the body is still durable");
    }

    /// A live declaration outranks a recalled one: recognition fills in what the
    /// caller left unsaid, and never overrides what it said.
    #[test]
    fn a_declared_sponsor_wins_over_the_recalled_one() {
        let req = AdmissionRequest {
            session_id: Some("claude:abc".into()),
            sponsor: Some("declared".into()),
            ..Default::default()
        };
        let (s, _) = admit(&req, "claude:abc".into(), 0, 10, Some(&body(Some("nlr_ai"))));
        assert_eq!(s.sponsor.as_deref(), Some("declared"));
        assert_eq!(
            s.recognized_from
                .as_ref()
                .and_then(|v| v.get("sponsor_source"))
                .and_then(|v| v.as_str()),
            Some("declared"),
            "the receipt says which sponsor governed the admission"
        );
    }

    /// Recalling nothing changes nothing: a first arrival is admitted exactly as
    /// it was before recognition existed.
    #[test]
    fn no_body_recalled_is_the_unchanged_walk_in() {
        let mut reg = SessionRegistry::default();
        let s = reg.get_or_walk_in("stranger", 0, None);
        assert_eq!(s.status, StatusLevel::UnauthenticatedVisitor);
        assert_eq!(s.continuity, Continuity::SessionOnly);
        assert!(s.recognized_from.is_none());
    }
}
