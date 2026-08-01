//! Collectivized inference for the Universe.
//!
//! Many inference providers, pluggable, with selection and fallback authored as
//! Universe data rather than compiled as native policy — and a scheduler that
//! keeps the city's order its own once the inference stops being the clock.
//!
//! ```text
//! routing data (graph)          native floor (here)
//! ────────────────────          ───────────────────
//! which providers exist    ->   how to CALL one            (provider.rs)
//! which actor gets which   ->   how to WALK a chain        (router.rs)
//! when to fall back        ->   how to move BYTES          (transport.rs)
//! every budget and bound   ->   how to ADMIT, in order     (clock.rs)
//! ```
//!
//! Start with [`clock`] for the answer to *"what is the clock, once the
//! inference isn't?"*, and [`routing`] for the shape of the authored data.
//!
//! # Module map
//!
//! * [`contract`] — the [`InferenceProvider`] trait and the total outcome
//!   vocabulary. This is the seam the rest of the runtime reconciles against.
//! * [`routing`] — the authored routing table: providers, wire shapes, chains,
//!   budgets, admission mode.
//! * [`transport`] — byte transports (`http://` over a socket, `https://` via
//!   an authorized external binary, and a stub).
//! * [`provider`] — one generic data-driven provider; Ollama and Anthropic are
//!   two instances of it, not two code paths.
//! * [`router`] — walks an authored chain and produces attribution.
//! * [`clock`] — the admission gate: parallel in flight, serial in wake order.

pub mod clock;
pub mod contract;
pub mod provider;
pub mod router;
pub mod routing;
pub mod transport;

pub use clock::{
    AdmissionGate, AdmissionWorld, DispatchRefusal, RejectionKind, Turn, TurnDisposition,
};
pub use contract::{
    AttemptRecord, InferenceAttribution, InferenceObservation, InferenceOutcome, InferenceProvider,
    InferenceRequest, Measured, ProviderAttempt, ProviderReadiness,
};
pub use provider::{scrub, HttpJsonProvider, CREDENTIAL_MARKER};
pub use router::CollectiveRouter;
pub use routing::{ProviderSpec, RoutingSource, RoutingTable};
pub use transport::{
    CurlHttpsTransport, StubTransport, TcpHttpTransport, TransportReadiness, WireTransport,
};

use universe_core::UniverseError;

/// Pick the byte transport an authored scheme requires.
///
/// A scheme with no native transport is a hard error, never a downgrade to a
/// weaker one.
pub fn transport_for(spec: &ProviderSpec) -> Result<Box<dyn WireTransport>, UniverseError> {
    match spec.transport.scheme.as_str() {
        "http" => Ok(Box::new(TcpHttpTransport::new())),
        "https" => Ok(Box::new(CurlHttpsTransport::new())),
        other => Err(UniverseError::Validation(format!(
            "provider {} declares scheme {other:?}, which has no native byte transport",
            spec.provider_id
        ))),
    }
}

/// Install a real provider instance for every provider the routing declares.
///
/// Providers whose credential is absent still get installed — they report
/// `not_configured` when called, which is a measured state an authored chain
/// can react to via `advance_on`.
pub fn install_all(router: &mut CollectiveRouter) -> Result<(), UniverseError> {
    let specs: Vec<ProviderSpec> = router.table().providers.clone();
    for spec in specs {
        let transport = transport_for(&spec)?;
        router.install(Box::new(HttpJsonProvider::new(spec, transport)));
    }
    Ok(())
}

/// Replace one provider with a stub that returns a canned body.
///
/// Only for proving wiring when a real call cannot be made. Every provider
/// installed this way reports `is_stubbed`, and a run that uses one must say
/// so in its evidence: a stubbed attempt is never a measurement of the remote
/// provider.
pub fn install_stub(
    router: &mut CollectiveRouter,
    provider_id: &str,
    status: u16,
    body: impl Into<Vec<u8>>,
) -> Result<(), UniverseError> {
    let spec = router
        .table()
        .provider(provider_id)
        .cloned()
        .ok_or_else(|| {
            UniverseError::Validation(format!("routing declares no provider {provider_id}"))
        })?;
    let stub = StubTransport::new(format!("stub:{provider_id}"), status, body);
    router.install(Box::new(HttpJsonProvider::stubbed(spec, Box::new(stub))));
    Ok(())
}
