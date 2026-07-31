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

#[allow(dead_code)] // `Persistent` arrives with sovereign (Citizen) identity
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
        })
    }
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
pub fn admit(request: &AdmissionRequest, session_id: String, now: u64, ttl: u64) -> (ActorSession, AdmissionReceipt) {
    let origin = request
        .origin
        .clone()
        .filter(|o| !o.trim().is_empty())
        .unwrap_or_else(|| "unknown-external".to_owned());
    let sponsored = request.sponsor.as_deref().is_some_and(|s| !s.trim().is_empty());
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
        authentication: Authentication::Unverified,
        continuity: Continuity::SessionOnly,
        status,
        issued_at: now,
        expires_at: now.saturating_add(ttl),
        permitted_spaces,
        capabilities,
        sponsor: request.sponsor.clone().filter(|s| !s.trim().is_empty()),
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
    pub fn admit(&mut self, request: &AdmissionRequest, now: u64) -> AdmissionReceipt {
        let session_id = request.session_id.clone().unwrap_or_else(|| {
            self.counter += 1;
            format!("sess-{}", self.counter)
        });
        let (session, receipt) = admit(request, session_id.clone(), now, DEFAULT_TTL_SECS);
        self.sessions.insert(session_id, session);
        receipt
    }

    /// Resolves a session id, minting a minimal traceable walk-in when unknown:
    /// the Gate never lets a presence exist without a provenance.
    pub fn get_or_walk_in(&mut self, session_id: &str, now: u64) -> ActorSession {
        if let Some(existing) = self.sessions.get(session_id) {
            if !existing.expired(now) {
                return existing.clone();
            }
        }
        let request = AdmissionRequest {
            session_id: Some(session_id.to_owned()),
            ..Default::default()
        };
        let (session, _) = admit(&request, session_id.to_owned(), now, DEFAULT_TTL_SECS);
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
        let (s, r) = admit(&req, "s1".into(), 1000, 100);
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
        let (s, _) = admit(&req, "s2".into(), 0, 10);
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
        let (s, r) = admit(&req, "s3".into(), 0, 10);
        assert!(!s.has(Capability::EmergencyBroadcast));
        assert!(!s.has(Capability::Own) && !s.has(Capability::Fire));
        assert_eq!(r.denied.len(), 3, "each dangerous request denied with a reason");
    }

    #[test]
    fn a_walk_in_is_minted_traceably_when_the_session_is_unknown() {
        let mut reg = SessionRegistry::default();
        let s = reg.get_or_walk_in("visitor-x", 0);
        assert_eq!(s.status, StatusLevel::UnauthenticatedVisitor);
        assert_eq!(s.origin, "unknown-external"); // traceable, not anonymous
        assert!(!s.has(Capability::Propose));
    }

    #[test]
    fn density_rises_with_standing() {
        let (unauth, _) = admit(&AdmissionRequest::default(), "a".into(), 0, 10);
        let (sponsored, _) = admit(
            &AdmissionRequest {
                sponsor: Some("NLR".into()),
                ..Default::default()
            },
            "b".into(),
            0,
            10,
        );
        assert!(sponsored.density() > unauth.density());
    }
}
