// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Hotel8:Loyalty
// SPDX-License-Identifier: Apache-2.0

//! Axum server setup for Loyalty.
use crate::handlers::AppState;
use axum::routing::{get, post, put};
use std::sync::Arc;

pub async fn build_state(
    core: sentin8::shell::ServiceCore,
) -> anyhow::Result<crate::handlers::AppState> {
    let config = crate::config::LoyaltyConfig::from_env();

    // Cert-presenting Bus client (CERTFREE-4V): the `loyalty` workload
    // CERTIFICATE is the identity — no DCR, no token (auth DESIGN
    // §4.3f); the mint retries while the fleet boots.
    let bus_client = loop {
        match sentin8::bus::client::BusClient::for_workload(
            &config.sentin8_bus_url,
            &config.sentin8_auth_issuer,
            "loyalty",
        )
        .await
        {
            Ok(c) => break c,
            Err(e) => {
                tracing::debug!(error = %e, "loyalty bus client: certificate not ready yet");
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        }
    };

    // PMS cross-service client (CERTFREE-4V): this service presents its
    // own `loyalty` workload certificate — PMS's TLS layer authenticates
    // the handshake and the registry map authorizes the scopes (the
    // `pms:reservations:read` / `pms:profiles:read` grants ride the
    // `loyalty` map row). The mint retries while the fleet boots.
    let pms_client = loop {
        match hotel8::pms::client::H8pmsClient::with_workload_certificate(
            &config.pms_url,
            &config.sentin8_auth_issuer,
            "loyalty",
        )
        .await
        {
            Ok(c) => break c,
            Err(e) => {
                tracing::debug!(error = %e, "loyalty pms client: certificate not ready yet");
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        }
    };

    // ── Sentin8:Policy (Shield) ──
    // Load the service's policy sets; an unparsable policy text REFUSES TO
    // START (fail-closed at boot, design §3.3). Hot-reload consumes the
    // `policy` stream as a direct consumer (design §3.2); deny/would-deny
    // decisions emit to Stat (§4.2f). The engine is stored in AppState so
    // handlers gate via `policy::require` (step 5 is the ONLY gate).
    // ── Sentin8:Policy (Shield, shell-owned boot): consume the
    // engine the shell booted in step 8 — no second engine, consumer, or
    // DCR registration here.
    let policy_engine = core.admin.policy.clone();

    let state = AppState {
        config: Arc::new(config),
        pool: core.pool.clone(),
        bus_client,
        pms_client,
        policy: policy_engine.clone(),
        admin: sentin8::shield::admin::SharedAdminState {
            service: "hotel8:loyalty".to_string(),
            pool: core.pool.clone(),
            policy: policy_engine,
            auth_client: Arc::new(core.config.sentin8_auth_client.clone()),
        },
    };

    // Event-driven consumer — subscribes to the PMS stream family.
    crate::event_consumer::spawn(core.pool.clone(), state.bus_client.clone());

    Ok(state)
}

/// Add this service's routes onto the shell-provided router. The shell
/// owns the boot sequence, state binding, OpenAPI merge and the standard
/// middleware stack; this adds ONLY routes (including the service's own
/// `/v1/health` — custom payload).
pub fn routes(
    router: axum::Router<crate::handlers::AppState>,
) -> axum::Router<crate::handlers::AppState> {
    router
        .route("/v1/health", get(crate::handlers::health))
        // ── Admin UI (RFW) ─────────────────────────────────────────────────
        .route("/v1/admin/ui/{*page}", get(crate::admin_ui::get_admin_ui))
        // ── Sentin8:Policy admin API (shared implementation,
        // handlers generic over this AppState).
        .route(
            "/v1/admin/policy-sets",
            get(sentin8::shield::admin::list_policy_sets::<crate::handlers::AppState>),
        )
        .route(
            "/v1/admin/policy-sets/{name}",
            get(sentin8::shield::admin::get_policy_set::<crate::handlers::AppState>)
                .put(sentin8::shield::admin::put_policy_set::<crate::handlers::AppState>)
                .delete(sentin8::shield::admin::delete_policy_set::<crate::handlers::AppState>),
        )
        .route(
            "/v1/admin/policy-sets/{name}/test",
            post(sentin8::shield::admin::test_policy_set::<crate::handlers::AppState>),
        )
        .route(
            "/v1/admin/policy-sets/enforce",
            post(sentin8::shield::admin::admin_enforce_policy_set::<crate::handlers::AppState>),
        )
        .route(
            "/v1/admin/policy-sets/shadow",
            post(sentin8::shield::admin::admin_shadow_policy_set::<crate::handlers::AppState>),
        )
        .route(
            "/v1/admin/policy-sets/disable",
            post(sentin8::shield::admin::admin_disable_policy_set::<crate::handlers::AppState>),
        )
        .route(
            "/v1/admin/relations",
            get(sentin8::shield::admin::list_relations::<crate::handlers::AppState>)
                .put(sentin8::shield::admin::put_relation::<crate::handlers::AppState>)
                .delete(sentin8::shield::admin::delete_relation::<crate::handlers::AppState>),
        )
        .route(
            "/v1/admin/relations/check",
            get(sentin8::shield::admin::check_relation::<crate::handlers::AppState>),
        )
        .route(
            "/v1/admin/attributes",
            get(sentin8::shield::admin::list_attributes::<crate::handlers::AppState>)
                .put(sentin8::shield::admin::put_attribute::<crate::handlers::AppState>)
                .delete(sentin8::shield::admin::delete_attribute::<crate::handlers::AppState>),
        )
        // Create + list programs
        .route(
            "/v1/programs",
            post(crate::handlers::create_program).get(crate::handlers::list_programs),
        )
        .route("/v1/members", post(crate::handlers::create_member))
        .route("/v1/points/award", post(crate::handlers::award_points))
        .route("/v1/points/redeem", post(crate::handlers::redeem_points))
        // Tier configuration
        .route(
            "/v1/tiers",
            post(crate::handlers::configure_tier).get(crate::handlers::list_tiers),
        )
        // Reward CRUD
        .route(
            "/v1/programs/{program_id}/rewards",
            post(crate::handlers::create_reward).get(crate::handlers::list_rewards),
        )
        .route(
            "/v1/programs/{program_id}/rewards/{reward_id}",
            put(crate::handlers::update_reward).delete(crate::handlers::delete_reward),
        )
        // Member profile
        .route("/v1/members/{member_id}", get(crate::handlers::get_member))
        // Link to guest profile
        .route(
            "/v1/members/link-guest",
            post(crate::handlers::link_to_guest_profile),
        )
        // Member search
        .route(
            "/v1/admin/programs/{program_id}/members",
            get(crate::handlers::search_members),
        )
        // Manual points adjustment
        .route(
            "/v1/admin/programs/{program_id}/members/{member_id}/adjust",
            post(crate::handlers::adjust_points),
        )
        // Points calculation
        .route(
            "/v1/points/calculate",
            post(crate::handlers::trigger_points_calculation),
        )
        // Transaction history
        .route(
            "/v1/members/{member_id}/transactions",
            get(crate::handlers::get_member_transactions),
        )
        // Points expiry
        .route(
            "/v1/cron/expire-points",
            post(crate::handlers::expire_points),
        )
        // Tier review
        .route("/v1/cron/review-tiers", post(crate::handlers::review_tiers))
        // Member rewards catalog
        .route(
            "/v1/members/{member_id}/rewards",
            get(crate::handlers::get_member_rewards),
        )
        // Reward redemption with voucher
        .route("/v1/rewards/redeem", post(crate::handlers::redeem_reward))
}

/// Shared policy-admin surface accessor — the shared
/// handlers extract this service's admin state via `HasPolicyAdmin`.
impl sentin8::shield::admin::HasPolicyAdmin for crate::handlers::AppState {
    fn policy_admin(&self) -> &sentin8::shield::admin::SharedAdminState {
        &self.admin
    }
}
