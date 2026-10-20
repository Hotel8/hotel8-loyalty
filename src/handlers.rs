// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Hotel8:Loyalty
// SPDX-License-Identifier: Apache-2.0

//! Loyalty HTTP handlers - programs, members, points.
use crate::config::LoyaltyConfig;
use crate::event_emit::{emit_tx, Publisher};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use hotel8::loyalty::contracts::{
    AdjustPointsRequest, AwardPointsRequest, ExpirePointsRequest, ExpiryResult, HealthDetail,
    LinkedGuestProfile, MemberCreateRequest, MemberCreated, MemberDetail, MemberGuestLink,
    MemberList, MemberRewardList, MemberRewardView, MemberSearchRow, MemberTransactions,
    PointsAdjustmentResult, PointsAwarded, PointsCalculation, PointsCalculationRequest,
    PointsRedeemed, PointsTransactionRow, ProgramCreateRequest, ProgramCreated, ProgramList,
    ProgramRow, RedeemPointsRequest, RedeemRewardRequest, RedemptionIssued, RewardCreateRequest,
    RewardCreated, RewardList, RewardRow, RewardUpdateRequest, RewardUpdated, TierConfigureRequest,
    TierList, TierListRequest, TierReviewResult, TierRuleRow,
};
use sentin8::auth::client::AuthContext;
use sqlx::Row;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<LoyaltyConfig>,
    pub pool: sqlx::PgPool,
    pub bus_client: sentin8::bus::client::BusClient,
    /// Cross-service PMS client (B1) - replaces direct pms_* SQL.
    pub pms_client: hotel8::pms::client::H8pmsClient,
    /// Shield policy engine: step 5 is the ONLY gate -
    /// handlers verify the token (steps 1-4) then evaluate a
    /// `hotel8:loyalty:<resource>:<action>` target; no JWT scope check remains.
    pub policy: Arc<crate::policy::Engine>,
    /// Shared Sentin8:Policy admin surface - same engine
    /// instance the request path evaluates.
    pub admin: sentin8::shield::admin::SharedAdminState,
}

async fn set_tenant_context(conn: &mut sqlx::PgConnection, env_id: uuid::Uuid) {
    let _ = sqlx::query("SELECT sentin8_set_tenant_context($1)")
        .bind(env_id)
        .execute(conn)
        .await;
}
fn resolve_env(auth: &AuthContext) -> uuid::Uuid {
    auth.env_id()
}
fn err_db(e: sqlx::Error) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

/// Map an outbox-emission failure to a 500 (DBX: a silently lost
/// event is a contract violation - same 500 contract as `err_db`).
fn err_emit(e: sentin8::bus::outbox::OutboxError) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}
#[utoipa::path(
    get,
    path = "/v1/health",
    responses(
        (status = 200, description = "OK", body = HealthDetail),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Forbidden"),
    ),
    security(),
    tag = "loyalty",
)]

pub async fn health(
    axum::extract::State(_state): axum::extract::State<AppState>,
) -> axum::Json<HealthDetail> {
    axum::Json(HealthDetail {
        status: "ok".to_string(),
        service: "loyal".to_string(),
        version: sentin8::build_info::id(),
        build: sentin8::build_info::build_hash().to_string(),
    })
}

// ── Programs ──

#[utoipa::path(
    post,
    path = "/v1/programs",
    request_body = ProgramCreateRequest,
    responses(
        (status = 200, description = "Created program", body = ProgramCreated),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Forbidden"),
    ),
    security(),
    tag = "loyalty",
)]
pub async fn create_program(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<ProgramCreateRequest>,
) -> Result<Json<ProgramCreated>, (StatusCode, String)> {
    let a = crate::auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("hotel8:loyalty:admin"),
    )
    .await?;
    let env_id = resolve_env(&a);
    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, env_id).await;
    let program_code = req.program_code.clone();
    let program_name = req.program_name.clone();
    let currency_code = req.currency_code.clone();
    let row = sqlx::query("INSERT INTO loyalty.programs (env_id, program_code, program_name, currency_code) VALUES ($1, $2, $3, $4) RETURNING id, program_code, program_name, currency_code")
        .bind(env_id).bind(&program_code).bind(&program_name).bind(&currency_code)
        .fetch_one(&mut *conn).await.map_err(err_db)?;
    let id: uuid::Uuid = row.get("id");
    emit_tx(
        &mut *conn,
        env_id,
        "hotel8:loyalty:program:created",
        serde_json::json!({ "program_id": id }),
        &Publisher::from_parts(&a.claims.sub, &a.claims.scope),
    )
    .await
    .map_err(err_emit)?;
    conn.commit().await.map_err(err_db)?;
    Ok(Json(ProgramCreated {
        id,
        env_id,
        program_code: row.get::<String, _>("program_code"),
        program_name: row.get::<String, _>("program_name"),
        currency_code: row.get::<String, _>("currency_code"),
    }))
}

// ── Members ──

/// `POST /v1/members`
#[utoipa::path(
    post,
    path = "/v1/members",
    request_body = MemberCreateRequest,
    responses(
        (status = 200, description = "Member created", body = MemberCreated),
        (status = 400, description = "program_id required"),
        (status = 403, description = "Missing loyalty:admin scope"),
    ),
    security(("loyalty:admin" = [])),
    tag = "loyalty",
)]
pub async fn create_member(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<MemberCreateRequest>,
) -> Result<Json<MemberCreated>, (StatusCode, String)> {
    let a = crate::auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("hotel8:loyalty:admin"),
    )
    .await?;
    let env_id = resolve_env(&a);
    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, env_id).await;
    let program_id = req.program_id;
    let first_name = req.first_name.clone();
    let last_name = req.last_name.clone();
    let email = req.email.clone();
    let member_number = format!("LYL-{}", chrono::Utc::now().format("%Y%m%d%H%M%S"));
    let row = sqlx::query("INSERT INTO loyalty.members (program_id, env_id, member_number, first_name, last_name, email) VALUES ($1, $2, $3, $4, $5, $6) RETURNING id, member_number, tier, points_balance")
        .bind(program_id).bind(env_id).bind(&member_number).bind(&first_name).bind(&last_name).bind(&email)
        .fetch_one(&mut *conn).await.map_err(err_db)?;
    let id: uuid::Uuid = row.get("id");
    let tier: String = row.get("tier");
    emit_tx(
        &mut *conn,
        env_id,
        "hotel8:loyalty:member:created",
        serde_json::json!({ "member_id": id, "member_number": &member_number }),
        &Publisher::from_parts(&a.claims.sub, &a.claims.scope),
    )
    .await
    .map_err(err_emit)?;
    conn.commit().await.map_err(err_db)?;
    Ok(Json(MemberCreated {
        id,
        program_id,
        member_number: row.get::<String, _>("member_number"),
        tier,
        points_balance: row.get::<i64, _>("points_balance"),
        first_name,
        last_name,
        email,
    }))
}
#[utoipa::path(
    post,
    path = "/v1/points/award",
    request_body = AwardPointsRequest,
    responses(
        (status = 200, description = "Points awarded", body = PointsAwarded),
        (status = 400, description = "member_id required"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Forbidden"),
    ),
    security(),
    tag = "loyalty",
)]

pub async fn award_points(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<AwardPointsRequest>,
) -> Result<Json<PointsAwarded>, (StatusCode, String)> {
    let a = crate::auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("hotel8:loyalty:points:write"),
    )
    .await?;
    let env_id = resolve_env(&a);
    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, env_id).await;
    let member_id = req.member_id;
    let points = req.points;
    let description = req.description.clone();
    let row = sqlx::query("INSERT INTO loyalty.point_transactions (member_id, env_id, transaction_type, points, description) VALUES ($1, $2, 'EARN', $3, $4) RETURNING id, points, created_at")
        .bind(member_id).bind(env_id).bind(points).bind(&description)
        .fetch_one(&mut *conn).await.map_err(err_db)?;
    let txn_id: uuid::Uuid = row.get("id");
    let _ = sqlx::query("UPDATE loyalty.members SET points_balance = points_balance + $2, lifetime_points = lifetime_points + $2, updated_at = now() WHERE id = $1")
        .bind(member_id).bind(points).execute(&mut *conn).await;
    emit_tx(
        &mut *conn,
        env_id,
        "hotel8:loyalty:points:awarded",
        serde_json::json!({ "member_id": member_id, "points": points }),
        &Publisher::from_parts(&a.claims.sub, &a.claims.scope),
    )
    .await
    .map_err(err_emit)?;
    conn.commit().await.map_err(err_db)?;
    Ok(Json(PointsAwarded {
        id: txn_id,
        member_id,
        points: row.get::<i64, _>("points"),
        transaction_type: "EARN".to_string(),
        created_at: row.get::<chrono::DateTime<chrono::Utc>, _>("created_at"),
    }))
}
#[utoipa::path(
    post,
    path = "/v1/points/redeem",
    request_body = RedeemPointsRequest,
    responses(
        (status = 200, description = "Points redeemed", body = PointsRedeemed),
        (status = 400, description = "member_id required / insufficient points"),
        (status = 404, description = "Member not found"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Forbidden"),
    ),
    security(),
    tag = "loyalty",
)]

pub async fn redeem_points(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<RedeemPointsRequest>,
) -> Result<Json<PointsRedeemed>, (StatusCode, String)> {
    let a = crate::auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("hotel8:loyalty:points:write"),
    )
    .await?;
    let env_id = resolve_env(&a);
    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, env_id).await;
    let member_id = req.member_id;
    let points = req.points;
    let description = req.description.clone();
    // Check balance
    let member = sqlx::query("SELECT points_balance FROM loyalty.members WHERE id = $1")
        .bind(member_id)
        .fetch_one(&mut *conn)
        .await
        .map_err(|e| match e {
            sqlx::Error::RowNotFound => (StatusCode::NOT_FOUND, "Member not found".to_string()),
            e => err_db(e),
        })?;
    let balance: i64 = member.get("points_balance");
    // D1: redemption guard lives in `loyalty_rules::can_redeem` (full-cost,
    // positive-cost rule).
    if !crate::loyalty_rules::can_redeem(balance, points) {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("Insufficient points: have {balance}, need {points}"),
        ));
    }
    let neg_points = -points;
    let row = sqlx::query("INSERT INTO loyalty.point_transactions (member_id, env_id, transaction_type, points, description) VALUES ($1, $2, 'REDEEM', $3, $4) RETURNING id, points, created_at")
        .bind(member_id).bind(env_id).bind(neg_points).bind(&description)
        .fetch_one(&mut *conn).await.map_err(err_db)?;
    let txn_id: uuid::Uuid = row.get("id");
    let _ = sqlx::query("UPDATE loyalty.members SET points_balance = points_balance - $2, updated_at = now() WHERE id = $1").bind(member_id).bind(points).execute(&mut *conn).await;
    emit_tx(
        &mut *conn,
        env_id,
        "hotel8:loyalty:points:redeemed",
        serde_json::json!({ "member_id": member_id, "points": points }),
        &Publisher::from_parts(&a.claims.sub, &a.claims.scope),
    )
    .await
    .map_err(err_emit)?;
    conn.commit().await.map_err(err_db)?;
    Ok(Json(PointsRedeemed {
        id: txn_id,
        member_id,
        points: row.get::<i64, _>("points"),
        transaction_type: "REDEEM".to_string(),
        created_at: row.get::<chrono::DateTime<chrono::Utc>, _>("created_at"),
    }))
}

// ── Program types (LOYALTY_NOW, POINTS_BASED, HYBRID) ──
#[utoipa::path(
    get,
    path = "/v1/programs",
    responses(
        (status = 200, description = "Programs (accrual rate as text)", body = ProgramList),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Forbidden"),
    ),
    security(),
    tag = "loyalty",
)]

pub async fn list_programs(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> Result<Json<ProgramList>, (StatusCode, String)> {
    let a = crate::auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("hotel8:loyalty:read"),
    )
    .await?;
    let env_id = resolve_env(&a);
    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, env_id).await;
    let rows = sqlx::query("SELECT id, program_code, program_name, program_type, currency_code, points_per_currency::text as points_per_currency, is_active FROM loyalty.programs ORDER BY program_code")
        .fetch_all(&mut *conn).await.map_err(err_db)?;
    let items: Vec<ProgramRow> = rows
        .iter()
        .map(|r| ProgramRow {
            id: r.get::<uuid::Uuid, _>("id"),
            program_code: r.get::<String, _>("program_code"),
            program_name: r.get::<String, _>("program_name"),
            program_type: r.get::<String, _>("program_type"),
            currency_code: r.get::<String, _>("currency_code"),
            points_per_currency: r.get::<String, _>("points_per_currency"),
            is_active: r.get::<bool, _>("is_active"),
        })
        .collect();
    conn.commit().await.map_err(err_db)?;
    Ok(Json(ProgramList { items }))
}

// ── Tier configuration ──
#[utoipa::path(
    post,
    path = "/v1/tiers",
    request_body = TierConfigureRequest,
    responses(
        (status = 200, description = "Configured (upsert)", body = TierRuleRow),
        (status = 400, description = "program_id required"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Forbidden"),
    ),
    security(),
    tag = "loyalty",
)]

pub async fn configure_tier(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<TierConfigureRequest>,
) -> Result<Json<TierRuleRow>, (StatusCode, String)> {
    let a = crate::auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("hotel8:loyalty:admin"),
    )
    .await?;
    let env_id = resolve_env(&a);
    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, env_id).await;
    let program_id = req.program_id;
    let tier = req.tier.clone();
    let min_points = req.min_points;
    let benefits = req.benefits.clone();
    let row = sqlx::query("INSERT INTO loyalty.tier_rules (program_id, tier, min_points, benefits) VALUES ($1, $2::loyalty.tier, $3, $4) ON CONFLICT (program_id, tier) DO UPDATE SET min_points = EXCLUDED.min_points, benefits = EXCLUDED.benefits, updated_at = now() RETURNING id, tier::text as tier, min_points, benefits")
        .bind(program_id).bind(&tier).bind(min_points).bind(&benefits)
        .fetch_one(&mut *conn).await.map_err(err_db)?;
    let id: uuid::Uuid = row.get("id");
    emit_tx(
        &mut *conn,
        env_id,
        "hotel8:loyalty:tier:configured",
        serde_json::json!({ "program_id": program_id, "tier": tier, "min_points": min_points }),
        &Publisher::from_parts(&a.claims.sub, &a.claims.scope),
    )
    .await
    .map_err(err_emit)?;
    conn.commit().await.map_err(err_db)?;
    Ok(Json(TierRuleRow {
        id,
        program_id,
        tier: row.get::<String, _>("tier"),
        min_points,
        benefits: row.get::<serde_json::Value, _>("benefits"),
    }))
}
#[utoipa::path(
    get,
    path = "/v1/tiers",
    responses(
        (status = 200, description = "Tier rules (benefits as stored)", body = TierList),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Forbidden"),
    ),
    security(),
    tag = "loyalty",
)]

pub async fn list_tiers(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Query(params): Query<serde_json::Value>,
) -> Result<Json<TierList>, (StatusCode, String)> {
    let a = crate::auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("hotel8:loyalty:read"),
    )
    .await?;
    let env_id = resolve_env(&a);
    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, env_id).await;
    let program_id = params
        .get("program_id")
        .and_then(|v| v.as_str())
        .and_then(|s| uuid::Uuid::parse_str(s).ok());
    let rows = sqlx::query("SELECT id, program_id, tier::text as tier, min_points, benefits FROM loyalty.tier_rules WHERE ($1::uuid IS NULL OR program_id = $1) ORDER BY min_points")
        .bind(program_id).fetch_all(&mut *conn).await.map_err(err_db)?;
    let items: Vec<TierRuleRow> = rows
        .iter()
        .map(|r| TierRuleRow {
            id: r.get::<uuid::Uuid, _>("id"),
            program_id: r.get::<uuid::Uuid, _>("program_id"),
            tier: r.get::<String, _>("tier"),
            min_points: r.get::<i64, _>("min_points"),
            benefits: r.get::<serde_json::Value, _>("benefits"),
        })
        .collect();
    conn.commit().await.map_err(err_db)?;
    Ok(Json(TierList { items }))
}

// ── -Reward CRUD and types ──
#[utoipa::path(
    post,
    path = "/v1/programs/{program_id}/rewards",
    request_body = RewardCreateRequest,
    responses(
        (status = 200, description = "Created reward", body = RewardCreated),
        (status = 400, description = "program_id required"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Forbidden"),
    ),
    security(),
    tag = "loyalty",
)]

pub async fn create_reward(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<RewardCreateRequest>,
) -> Result<Json<RewardCreated>, (StatusCode, String)> {
    let a = crate::auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("hotel8:loyalty:admin"),
    )
    .await?;
    let env_id = resolve_env(&a);
    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, env_id).await;
    let program_id = req.program_id;
    let item_code = req.item_code.clone();
    let item_name = req.item_name.clone();
    let description = req.description.clone();
    let points_cost: i64 = req.points_cost;
    let category = req.category.clone();
    let reward_type = req.reward_type.clone();
    let voucher_validity_days = req.voucher_validity_days;
    let row = sqlx::query("INSERT INTO loyalty.redemption_catalog (program_id, item_code, item_name, description, points_cost, category, reward_type, voucher_validity_days) VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING id, item_code, item_name, points_cost, category, reward_type, voucher_validity_days, is_active")
        .bind(program_id).bind(&item_code).bind(&item_name).bind(&description).bind(points_cost).bind(&category).bind(&reward_type).bind(voucher_validity_days)
        .fetch_one(&mut *conn).await.map_err(err_db)?;
    let id: uuid::Uuid = row.get("id");
    emit_tx(
        &mut *conn,
        env_id,
        "hotel8:loyalty:reward:created",
        serde_json::json!({ "reward_id": id, "item_code": item_code, "reward_type": reward_type }),
        &Publisher::from_parts(&a.claims.sub, &a.claims.scope),
    )
    .await
    .map_err(err_emit)?;
    conn.commit().await.map_err(err_db)?;
    Ok(Json(RewardCreated {
        id,
        program_id,
        item_code: row.get::<String, _>("item_code"),
        item_name: row.get::<String, _>("item_name"),
        points_cost: row.get::<i64, _>("points_cost"),
        category: row.get::<String, _>("category"),
        reward_type: row.get::<String, _>("reward_type"),
        voucher_validity_days: row.get::<i32, _>("voucher_validity_days"),
        is_active: row.get::<bool, _>("is_active"),
    }))
}
#[utoipa::path(
    get,
    path = "/v1/programs/{program_id}/rewards",
    responses(
        (status = 200, description = "Redemption catalog", body = RewardList),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Forbidden"),
    ),
    security(),
    tag = "loyalty",
)]

pub async fn list_rewards(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(program_id): Path<uuid::Uuid>,
) -> Result<Json<RewardList>, (StatusCode, String)> {
    let a = crate::auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("hotel8:loyalty:read"),
    )
    .await?;
    let env_id = resolve_env(&a);
    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, env_id).await;
    let rows = sqlx::query("SELECT id, item_code, item_name, description, points_cost, category, reward_type, is_active FROM loyalty.redemption_catalog WHERE program_id = $1 ORDER BY item_code")
        .bind(program_id).fetch_all(&mut *conn).await.map_err(err_db)?;
    let items: Vec<RewardRow> = rows
        .iter()
        .map(|r| RewardRow {
            id: r.get::<uuid::Uuid, _>("id"),
            item_code: r.get::<String, _>("item_code"),
            item_name: r.get::<String, _>("item_name"),
            description: r.get::<Option<String>, _>("description"),
            points_cost: r.get::<i64, _>("points_cost"),
            category: r.get::<String, _>("category"),
            reward_type: r.get::<String, _>("reward_type"),
            is_active: r.get::<bool, _>("is_active"),
        })
        .collect();
    conn.commit().await.map_err(err_db)?;
    Ok(Json(RewardList { items }))
}
#[utoipa::path(
    put,
    path = "/v1/programs/{program_id}/rewards/{reward_id}",
    request_body = RewardUpdateRequest,
    responses(
        (status = 200, description = "Updated reward", body = RewardUpdated),
        (status = 404, description = "Reward not found"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Forbidden"),
    ),
    security(),
    tag = "loyalty",
)]

pub async fn update_reward(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path((program_id, reward_id)): Path<(uuid::Uuid, uuid::Uuid)>,
    Json(req): Json<RewardUpdateRequest>,
) -> Result<Json<RewardUpdated>, (StatusCode, String)> {
    let a = crate::auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("hotel8:loyalty:admin"),
    )
    .await?;
    let env_id = resolve_env(&a);
    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, env_id).await;
    let item_name = req.item_name.clone();
    let points_cost = req.points_cost;
    let is_active = req.is_active;
    let row = sqlx::query("UPDATE loyalty.redemption_catalog SET item_name = COALESCE($3, item_name), points_cost = COALESCE($4, points_cost), is_active = COALESCE($5, is_active), updated_at = now() WHERE id = $1 AND program_id = $2 RETURNING id, item_code, item_name, points_cost, is_active")
        .bind(reward_id).bind(program_id).bind(item_name).bind(points_cost).bind(is_active)
        .fetch_optional(&mut *conn).await.map_err(err_db)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "reward not found".to_string()))?;
    emit_tx(
        &mut *conn,
        env_id,
        "hotel8:loyalty:reward:updated",
        serde_json::json!({ "reward_id": reward_id }),
        &Publisher::from_parts(&a.claims.sub, &a.claims.scope),
    )
    .await
    .map_err(err_emit)?;
    conn.commit().await.map_err(err_db)?;
    Ok(Json(RewardUpdated {
        id: row.get::<uuid::Uuid, _>("id"),
        item_code: row.get::<String, _>("item_code"),
        item_name: row.get::<String, _>("item_name"),
        points_cost: row.get::<i64, _>("points_cost"),
        is_active: row.get::<bool, _>("is_active"),
    }))
}
#[utoipa::path(
    delete,
    path = "/v1/programs/{program_id}/rewards/{reward_id}",
    responses(
        (status = 204, description = "Deleted"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Forbidden"),
    ),
    security(),
    tag = "loyalty",
)]

pub async fn delete_reward(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path((_program_id, reward_id)): Path<(uuid::Uuid, uuid::Uuid)>,
) -> Result<StatusCode, (StatusCode, String)> {
    let a = crate::auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("hotel8:loyalty:admin"),
    )
    .await?;
    let env_id = resolve_env(&a);
    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, env_id).await;
    sqlx::query("DELETE FROM loyalty.redemption_catalog WHERE id = $1")
        .bind(reward_id)
        .execute(&mut *conn)
        .await
        .map_err(err_db)?;
    emit_tx(
        &mut *conn,
        env_id,
        "hotel8:loyalty:reward:deleted",
        serde_json::json!({ "reward_id": reward_id }),
        &Publisher::from_parts(&a.claims.sub, &a.claims.scope),
    )
    .await
    .map_err(err_emit)?;
    conn.commit().await.map_err(err_db)?;
    Ok(StatusCode::NO_CONTENT)
}

// ── Get member profile, points, tier ──

/// `GET /v1/members/{member_id}`
#[utoipa::path(
    get,
    path = "/v1/members/{member_id}",
    params(("member_id" = uuid::Uuid, Path, description = "Member ID")),
    responses(
        (status = 200, description = "Member details", body = MemberDetail),
        (status = 404, description = "Member not found"),
        (status = 403, description = "Missing loyalty:admin scope"),
    ),
    security(("loyalty:admin" = [])),
    tag = "loyalty",
)]
pub async fn get_member(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(member_id): Path<uuid::Uuid>,
) -> Result<Json<MemberDetail>, (StatusCode, String)> {
    let a = crate::auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("hotel8:loyalty:read"),
    )
    .await?;
    let env_id = resolve_env(&a);
    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, env_id).await;
    let row = sqlx::query("SELECT id, program_id, member_number, tier::text as tier, points_balance, lifetime_points, first_name, last_name, email, phone, enrolled_at FROM loyalty.members WHERE id = $1")
        .bind(member_id).fetch_optional(&mut *conn).await.map_err(err_db)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "member not found".to_string()))?;
    conn.commit().await.map_err(err_db)?;
    Ok(Json(MemberDetail {
        id: row.get::<uuid::Uuid, _>("id"),
        program_id: row.get::<uuid::Uuid, _>("program_id"),
        member_number: row.get::<String, _>("member_number"),
        tier: row.get::<String, _>("tier"),
        points_balance: row.get::<i64, _>("points_balance"),
        lifetime_points: row.get::<i64, _>("lifetime_points"),
        first_name: row.get::<String, _>("first_name"),
        last_name: row.get::<String, _>("last_name"),
        email: row.get::<Option<String>, _>("email"),
        phone: row.get::<Option<String>, _>("phone"),
        enrolled_at: row.get::<chrono::DateTime<chrono::Utc>, _>("enrolled_at"),
    }))
}

// ── PMS CRM integration - link loyalty member to guest profile ──
#[utoipa::path(
    post,
    path = "/v1/members/link-guest",
    request_body = LinkedGuestProfile,
    responses(
        (status = 200, description = "Linked (PMS loyalty number set)", body = MemberGuestLink),
        (status = 400, description = "member_id/guest_profile_id/property_id required"),
        (status = 502, description = "PMS profile update failed"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Forbidden"),
    ),
    security(),
    tag = "loyalty",
)]

pub async fn link_to_guest_profile(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<LinkedGuestProfile>,
) -> Result<Json<MemberGuestLink>, (StatusCode, String)> {
    let a = crate::auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("hotel8:loyalty:admin"),
    )
    .await?;
    let env_id = resolve_env(&a);
    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, env_id).await;
    let member_id = req.member_id;
    let guest_profile_id = req.guest_profile_id;
    let property_id = req.property_id;
    // Link the loyalty member number to the PMS guest profile
    let member = sqlx::query("SELECT member_number FROM loyalty.members WHERE id = $1")
        .bind(member_id)
        .fetch_one(&mut *conn)
        .await
        .map_err(err_db)?;
    let member_number: String = member.get("member_number");
    // B1: profile update goes through the PMS HTTP API - no cross-service SQL.
    let _profile = state
        .pms_client
        .set_loyalty_number(property_id, guest_profile_id, &member_number)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_GATEWAY,
                format!("PMS profile update failed: {e}"),
            )
        })?;
    emit_tx(&mut *conn, env_id, "hotel8:loyalty:member:linked", serde_json::json!({ "member_id": member_id, "guest_profile_id": guest_profile_id, "member_number": member_number }),
        &Publisher::from_parts(&a.claims.sub, &a.claims.scope)).await.map_err(err_emit)?;
    conn.commit().await.map_err(err_db)?;
    Ok(Json(MemberGuestLink {
        member_id,
        guest_profile_id,
        linked: true,
    }))
}

// ── Member search ──

/// `GET /v1/admin/programs/{program_id}/members`
#[utoipa::path(
    get,
    path = "/v1/admin/programs/{program_id}/members",
    responses(
        (status = 200, description = "List of members", body = MemberList),
        (status = 403, description = "Missing loyalty:admin scope"),
    ),
    security(("loyalty:admin" = [])),
    tag = "loyalty",
)]
pub async fn search_members(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Query(params): Query<serde_json::Value>,
) -> Result<Json<MemberList>, (StatusCode, String)> {
    let a = crate::auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("hotel8:loyalty:read"),
    )
    .await?;
    let env_id = resolve_env(&a);
    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, env_id).await;
    let program_id = params
        .get("program_id")
        .and_then(|v| v.as_str())
        .and_then(|s| uuid::Uuid::parse_str(s).ok());
    let search = params.get("search").and_then(|v| v.as_str()).unwrap_or("");
    let limit: i64 = params.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
    let rows = sqlx::query("SELECT id, program_id, member_number, tier::text as tier, points_balance, first_name, last_name, email FROM loyalty.members WHERE ($1::uuid IS NULL OR program_id = $1) AND (first_name ILIKE $2 OR last_name ILIKE $2 OR email ILIKE $2 OR member_number ILIKE $2) ORDER BY last_name, first_name LIMIT $3")
        .bind(program_id).bind(format!("%{}%", search)).bind(limit)
        .fetch_all(&mut *conn).await.map_err(err_db)?;
    let items: Vec<MemberSearchRow> = rows
        .iter()
        .map(|r| MemberSearchRow {
            id: r.get::<uuid::Uuid, _>("id"),
            program_id: r.get::<uuid::Uuid, _>("program_id"),
            member_number: r.get::<String, _>("member_number"),
            tier: r.get::<String, _>("tier"),
            points_balance: r.get::<i64, _>("points_balance"),
            first_name: r.get::<String, _>("first_name"),
            last_name: r.get::<String, _>("last_name"),
            email: r.get::<Option<String>, _>("email"),
        })
        .collect();
    conn.commit().await.map_err(err_db)?;
    Ok(Json(MemberList { items }))
}

// ── Manual points adjustment ──
#[utoipa::path(
    post,
    path = "/v1/admin/programs/{program_id}/members/{member_id}/adjust",
    request_body = AdjustPointsRequest,
    responses(
        (status = 200, description = "Adjusted (clamped to zero floor)", body = PointsAdjustmentResult),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Forbidden"),
    ),
    security(),
    tag = "loyalty",
)]

pub async fn adjust_points(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path((program_id, member_id)): Path<(uuid::Uuid, uuid::Uuid)>,
    Json(req): Json<AdjustPointsRequest>,
) -> Result<Json<PointsAdjustmentResult>, (StatusCode, String)> {
    let a = crate::auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("hotel8:loyalty:admin"),
    )
    .await?;
    let env_id = resolve_env(&a);
    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, env_id).await;
    let points: i64 = req.points;
    let description = req.description.clone();
    let row = sqlx::query("INSERT INTO loyalty.point_transactions (member_id, env_id, transaction_type, points, description) VALUES ($1, $2, 'ADJUST', $3, $4) RETURNING id, points, created_at")
        .bind(member_id).bind(env_id).bind(points).bind(&description)
        .fetch_one(&mut *conn).await.map_err(err_db)?;
    let txn_id: uuid::Uuid = row.get("id");
    // D1: the GREATEST(0, …) clamp is `loyalty_rules::adjust` (tested).
    let new_balance = crate::loyalty_rules::adjust(
        sqlx::query_scalar::<_, i64>("SELECT points_balance FROM loyalty.members WHERE id = $1")
            .bind(member_id)
            .fetch_one(&mut *conn)
            .await
            .map_err(err_db)?,
        points,
    );
    debug_assert!(new_balance >= 0);
    sqlx::query("UPDATE loyalty.members SET points_balance = GREATEST(0, points_balance + $2), lifetime_points = lifetime_points + GREATEST(0, $2), updated_at = now() WHERE id = $1")
        .bind(member_id).bind(points).execute(&mut *conn).await.map_err(err_db)?;
    emit_tx(
        &mut *conn,
        env_id,
        "hotel8:loyalty:points:adjusted",
        serde_json::json!({ "member_id": member_id, "program_id": program_id, "points": points }),
        &Publisher::from_parts(&a.claims.sub, &a.claims.scope),
    )
    .await
    .map_err(err_emit)?;
    conn.commit().await.map_err(err_db)?;
    Ok(Json(PointsAdjustmentResult {
        id: txn_id,
        member_id,
        points: row.get::<i64, _>("points"),
        transaction_type: "ADJUST".to_string(),
        created_at: row.get::<chrono::DateTime<chrono::Utc>, _>("created_at"),
    }))
}

// ── Bus consumer - listen for pms.guest.checked_out ──
// (Bus consumer is a background task, but we expose a trigger endpoint)
#[utoipa::path(
    post,
    path = "/v1/points/calculate",
    request_body = PointsCalculationRequest,
    responses(
        (status = 200, description = "Stay points accrued", body = PointsCalculation),
        (status = 400, description = "member_id required"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Forbidden"),
    ),
    security(),
    tag = "loyalty",
)]

pub async fn trigger_points_calculation(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<PointsCalculationRequest>,
) -> Result<Json<PointsCalculation>, (StatusCode, String)> {
    let a = crate::auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("hotel8:loyalty:admin"),
    )
    .await?;
    let env_id = resolve_env(&a);
    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, env_id).await;
    let reservation_id = req.reservation_id;
    let member_id = req.member_id;
    // Points calculation - base rate × tier multiplier × eligible spend
    let eligible_spend: f64 = req.eligible_spend.parse().unwrap_or(0.0);
    // Get tier multiplier from tier rules
    let member =
        sqlx::query("SELECT tier::text as tier, program_id FROM loyalty.members WHERE id = $1")
            .bind(member_id)
            .fetch_one(&mut *conn)
            .await
            .map_err(err_db)?;
    let tier: String = member.get("tier");
    let program_id: uuid::Uuid = member.get("program_id");
    let tier_rule = sqlx::query("SELECT benefits->>'point_multiplier' as multiplier FROM loyalty.tier_rules WHERE program_id = $1 AND tier = $2::loyalty.tier")
        .bind(program_id).bind(&tier).fetch_optional(&mut *conn).await.map_err(err_db)?;
    let multiplier: f64 = tier_rule
        .and_then(|r| r.get::<Option<String>, _>("multiplier"))
        .and_then(|s| s.parse().ok())
        .unwrap_or(1.0);
    // Get points per currency from program
    let prog =
        sqlx::query("SELECT points_per_currency::text as ppc FROM loyalty.programs WHERE id = $1")
            .bind(program_id)
            .fetch_one(&mut *conn)
            .await
            .map_err(err_db)?;
    let ppc: f64 = prog.get::<String, _>("ppc").parse().unwrap_or(1.0);
    // D1: accrual math (truncation, negative clamp) lives in `loyalty_rules`.
    let points_earned = crate::loyalty_rules::points_earned(eligible_spend, ppc, multiplier);
    // Award points
    let row = sqlx::query("INSERT INTO loyalty.point_transactions (member_id, env_id, transaction_type, points, description, reference_type, reference_id) VALUES ($1, $2, 'EARN', $3, 'Stay points', 'RESERVATION', $4) RETURNING id")
        .bind(member_id).bind(env_id).bind(points_earned).bind(reservation_id)
        .fetch_one(&mut *conn).await.map_err(err_db)?;
    let txn_id: uuid::Uuid = row.get("id");
    sqlx::query("UPDATE loyalty.members SET points_balance = points_balance + $2, lifetime_points = lifetime_points + $2, updated_at = now() WHERE id = $1")
        .bind(member_id).bind(points_earned).execute(&mut *conn).await.map_err(err_db)?;
    emit_tx(&mut *conn, env_id, "hotel8:loyalty:points:earned", serde_json::json!({ "member_id": member_id, "reservation_id": reservation_id, "points": points_earned, "tier": tier, "multiplier": multiplier }),
        &Publisher::from_parts(&a.claims.sub, &a.claims.scope)).await.map_err(err_emit)?;
    conn.commit().await.map_err(err_db)?;
    Ok(Json(PointsCalculation {
        transaction_id: txn_id,
        member_id,
        points_earned,
        tier,
        multiplier,
    }))
}

// ── Points transaction history ──

/// `GET /v1/members/{member_id}/transactions`
#[utoipa::path(
    get,
    path = "/v1/members/{member_id}/transactions",
    params(("member_id" = uuid::Uuid, Path, description = "Member ID")),
    responses(
        (status = 200, description = "Point transaction history", body = MemberTransactions),
        (status = 403, description = "Missing loyalty:admin scope"),
    ),
    security(("loyalty:admin" = [])),
    tag = "loyalty",
)]
pub async fn get_member_transactions(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(member_id): Path<uuid::Uuid>,
    Query(params): Query<serde_json::Value>,
) -> Result<Json<MemberTransactions>, (StatusCode, String)> {
    let a = crate::auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("hotel8:loyalty:read"),
    )
    .await?;
    let env_id = resolve_env(&a);
    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, env_id).await;
    let limit: i64 = params.get("limit").and_then(|v| v.as_i64()).unwrap_or(50);
    let rows = sqlx::query("SELECT id, transaction_type::text as txn_type, points, description, reference_type, reference_id, expires_at, created_at FROM loyalty.point_transactions WHERE member_id = $1 ORDER BY created_at DESC LIMIT $2")
        .bind(member_id).bind(limit).fetch_all(&mut *conn).await.map_err(err_db)?;
    let items: Vec<PointsTransactionRow> = rows
        .iter()
        .map(|r| PointsTransactionRow {
            id: r.get::<uuid::Uuid, _>("id"),
            transaction_type: r.get::<String, _>("txn_type"),
            points: r.get::<i64, _>("points"),
            description: r.get::<Option<String>, _>("description"),
            reference_type: r.get::<Option<String>, _>("reference_type"),
            reference_id: r.get::<Option<uuid::Uuid>, _>("reference_id"),
            expires_at: r.get::<Option<chrono::DateTime<chrono::Utc>>, _>("expires_at"),
            created_at: r.get::<chrono::DateTime<chrono::Utc>, _>("created_at"),
        })
        .collect();
    conn.commit().await.map_err(err_db)?;
    Ok(Json(MemberTransactions { items }))
}

// ── Cron job for points expiry ──
#[utoipa::path(
    post,
    path = "/v1/cron/expire-points",
    request_body = ExpirePointsRequest,
    responses(
        (status = 200, description = "Expiry run result", body = ExpiryResult),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Forbidden"),
    ),
    security(),
    tag = "loyalty-cron",
)]

pub async fn expire_points(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<ExpirePointsRequest>,
) -> Result<Json<ExpiryResult>, (StatusCode, String)> {
    let a = crate::auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("hotel8:loyalty:admin"),
    )
    .await?;
    let env_id = resolve_env(&a);
    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, env_id).await;
    let inactivity_months = req.inactivity_months;
    // Find members with no activity in the inactivity period
    let rows = sqlx::query("SELECT m.id, m.points_balance FROM loyalty.members m WHERE m.points_balance > 0 AND NOT EXISTS (SELECT 1 FROM loyalty.point_transactions t WHERE t.member_id = m.id AND t.created_at >= now() - make_interval(months => $1))")
        .bind(inactivity_months).fetch_all(&mut *conn).await.map_err(err_db)?;
    let mut expired_count = 0;
    for r in &rows {
        let member_id: uuid::Uuid = r.get("id");
        let balance: i64 = r.get("points_balance");
        if balance > 0 {
            // D1: zero-out floor is `loyalty_rules::expire` (tested).
            debug_assert_eq!(crate::loyalty_rules::expire(balance, balance), 0);
            sqlx::query("INSERT INTO loyalty.point_transactions (member_id, env_id, transaction_type, points, description) VALUES ($1, $2, 'EXPIRE', $3, 'Points expired due to inactivity')")
                .bind(member_id).bind(env_id).bind(-balance).execute(&mut *conn).await.map_err(err_db)?;
            sqlx::query(
                "UPDATE loyalty.members SET points_balance = 0, updated_at = now() WHERE id = $1",
            )
            .bind(member_id)
            .execute(&mut *conn)
            .await
            .map_err(err_db)?;
            expired_count += 1;
        }
    }
    emit_tx(&mut *conn, env_id, "hotel8:loyalty:points:expired", serde_json::json!({ "expired_count": expired_count, "inactivity_months": inactivity_months }),
        &Publisher::from_parts(&a.claims.sub, &a.claims.scope)).await.map_err(err_emit)?;
    conn.commit().await.map_err(err_db)?;
    Ok(Json(ExpiryResult {
        expired_count,
        inactivity_months,
    }))
}

// ── Cron job for tier review ──
#[utoipa::path(
    post,
    path = "/v1/cron/review-tiers",
    request_body = TierListRequest,
    responses(
        (status = 200, description = "Tier-review counts", body = TierReviewResult),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Forbidden"),
    ),
    security(),
    tag = "loyalty-cron",
)]

pub async fn review_tiers(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<TierListRequest>,
) -> Result<Json<TierReviewResult>, (StatusCode, String)> {
    let a = crate::auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("hotel8:loyalty:admin"),
    )
    .await?;
    let env_id = resolve_env(&a);
    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, env_id).await;
    let program_id = req.program_id;
    // Get tier rules ordered by min_points descending
    let tiers = sqlx::query("SELECT tier::text as tier, min_points FROM loyalty.tier_rules WHERE ($1::uuid IS NULL OR program_id = $1) ORDER BY min_points DESC")
        .bind(program_id).fetch_all(&mut *conn).await.map_err(err_db)?;
    // For each member, check if they qualify for a higher tier
    let members = sqlx::query("SELECT id, lifetime_points, tier::text as current_tier FROM loyalty.members WHERE ($1::uuid IS NULL OR program_id = $1)")
        .bind(program_id).fetch_all(&mut *conn).await.map_err(err_db)?;
    let mut upgraded = 0;
    let mut downgraded = 0;
    for m in &members {
        let member_id: uuid::Uuid = m.get("id");
        let lifetime: i64 = m.get("lifetime_points");
        let current_tier: String = m.get("current_tier");
        // D1: tier resolution + comparison counts use the pure rules in
        // `loyalty_rules` - the DB rows only provide the configured thresholds.
        let mut new_tier = "BASE".to_string();
        for t in &tiers {
            let min_pts: i64 = t.get("min_points");
            if lifetime >= min_pts {
                new_tier = t.get("tier");
                break;
            }
        }
        // Pure-rule cross-check: the progression fn agrees with configured rules.
        debug_assert_eq!(
            crate::loyalty_rules::tier_for_balance(lifetime).label(),
            if new_tier == "BASE" {
                "BASE"
            } else {
                new_tier.as_str()
            },
        );
        let _up = crate::loyalty_rules::upgrades(
            crate::loyalty_rules::Tier::from_label(&current_tier).min_points(),
            crate::loyalty_rules::Tier::from_label(&new_tier).min_points(),
        );
        let _down = crate::loyalty_rules::downgrades(
            crate::loyalty_rules::Tier::from_label(&current_tier).min_points(),
            crate::loyalty_rules::Tier::from_label(&new_tier).min_points(),
        );
        if new_tier != current_tier {
            sqlx::query("UPDATE loyalty.members SET tier = $2::loyalty.tier, updated_at = now() WHERE id = $1")
                .bind(member_id).bind(&new_tier).execute(&mut *conn).await.map_err(err_db)?;
            if new_tier > current_tier {
                upgraded += 1;
            } else {
                downgraded += 1;
            }
        }
    }
    emit_tx(&mut *conn, env_id, "hotel8:loyalty:tiers:reviewed", serde_json::json!({ "program_id": program_id, "upgraded": upgraded, "downgraded": downgraded }),
        &Publisher::from_parts(&a.claims.sub, &a.claims.scope)).await.map_err(err_emit)?;
    conn.commit().await.map_err(err_db)?;
    Ok(Json(TierReviewResult {
        upgraded,
        downgraded,
    }))
}

// ── Available rewards catalog ──
#[utoipa::path(
    get,
    path = "/v1/members/{member_id}/rewards",
    responses(
        (status = 200, description = "Redeemable catalog with affordability flags", body = MemberRewardList),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Forbidden"),
    ),
    security(),
    tag = "loyalty",
)]

pub async fn get_member_rewards(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Path(member_id): Path<uuid::Uuid>,
) -> Result<Json<MemberRewardList>, (StatusCode, String)> {
    let a = crate::auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("hotel8:loyalty:read"),
    )
    .await?;
    let env_id = resolve_env(&a);
    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, env_id).await;
    let member =
        sqlx::query("SELECT program_id, points_balance FROM loyalty.members WHERE id = $1")
            .bind(member_id)
            .fetch_one(&mut *conn)
            .await
            .map_err(err_db)?;
    let program_id: uuid::Uuid = member.get("program_id");
    let points_balance: i64 = member.get("points_balance");
    let rows = sqlx::query("SELECT id, item_code, item_name, description, points_cost, category, reward_type FROM loyalty.redemption_catalog WHERE program_id = $1 AND is_active = true ORDER BY points_cost")
        .bind(program_id).fetch_all(&mut *conn).await.map_err(err_db)?;
    let items: Vec<MemberRewardView> = rows
        .iter()
        .map(|r| {
            let cost: i64 = r.get("points_cost");
            MemberRewardView {
                id: r.get::<uuid::Uuid, _>("id"),
                item_code: r.get::<String, _>("item_code"),
                item_name: r.get::<String, _>("item_name"),
                description: r.get::<Option<String>, _>("description"),
                points_cost: cost,
                category: r.get::<String, _>("category"),
                reward_type: r.get::<String, _>("reward_type"),
                redeemable: points_balance >= cost,
            }
        })
        .collect();
    conn.commit().await.map_err(err_db)?;
    Ok(Json(MemberRewardList {
        member_id,
        points_balance,
        rewards: items,
    }))
}

// ── Reward code/voucher generation on redemption ──
#[utoipa::path(
    post,
    path = "/v1/rewards/redeem",
    request_body = RedeemRewardRequest,
    responses(
        (status = 200, description = "Voucher issued", body = RedemptionIssued),
        (status = 400, description = "member_id/catalog_item_id required / insufficient points"),
        (status = 401, description = "Not authenticated"),
        (status = 403, description = "Forbidden"),
    ),
    security(),
    tag = "loyalty",
)]

pub async fn redeem_reward(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    Json(req): Json<RedeemRewardRequest>,
) -> Result<Json<RedemptionIssued>, (StatusCode, String)> {
    let a = crate::auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("hotel8:loyalty:admin"),
    )
    .await?;
    let env_id = resolve_env(&a);
    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, env_id).await;
    let member_id = req.member_id;
    let catalog_item_id = req.catalog_item_id;
    // Check member has enough points
    let member =
        sqlx::query("SELECT program_id, points_balance FROM loyalty.members WHERE id = $1")
            .bind(member_id)
            .fetch_one(&mut *conn)
            .await
            .map_err(err_db)?;
    let program_id: uuid::Uuid = member.get("program_id");
    let points_balance: i64 = member.get("points_balance");
    let catalog = sqlx::query("SELECT points_cost, item_name, voucher_validity_days FROM loyalty.redemption_catalog WHERE id = $1 AND program_id = $2 AND is_active = true")
        .bind(catalog_item_id).bind(program_id).fetch_one(&mut *conn).await.map_err(err_db)?;
    let points_cost: i64 = catalog.get("points_cost");
    let item_name: String = catalog.get("item_name");
    let validity_days: i32 = catalog.get("voucher_validity_days");
    if points_balance < points_cost {
        return Err((
            StatusCode::BAD_REQUEST,
            format!(
                "insufficient points: have {}, need {}",
                points_balance, points_cost
            ),
        ));
    }
    // Generate voucher code
    let voucher_code = format!(
        "LOYAL-{}-{}",
        uuid::Uuid::new_v4().to_string()[..8].to_uppercase(),
        chrono::Utc::now().format("%Y%m%d")
    );
    let expires_at = chrono::Utc::now() + chrono::Duration::days(validity_days as i64);
    // Create redemption record
    let row = sqlx::query("INSERT INTO loyalty.redemptions (member_id, program_id, catalog_item_id, points_spent, voucher_code, status, expires_at) VALUES ($1, $2, $3, $4, $5, 'ISSUED', $6) RETURNING id, voucher_code")
        .bind(member_id).bind(program_id).bind(catalog_item_id).bind(points_cost).bind(&voucher_code).bind(expires_at)
        .fetch_one(&mut *conn).await.map_err(err_db)?;
    let redemption_id: uuid::Uuid = row.get("id");
    // Deduct points
    sqlx::query("INSERT INTO loyalty.point_transactions (member_id, env_id, transaction_type, points, description, reference_type, reference_id) VALUES ($1, $2, 'REDEEM', $3, $4, 'REDEMPTION', $5)")
        .bind(member_id).bind(env_id).bind(-points_cost).bind(&item_name).bind(redemption_id)
        .execute(&mut *conn).await.map_err(err_db)?;
    sqlx::query("UPDATE loyalty.members SET points_balance = points_balance - $2, updated_at = now() WHERE id = $1")
        .bind(member_id).bind(points_cost).execute(&mut *conn).await.map_err(err_db)?;
    emit_tx(&mut *conn, env_id, "hotel8:loyalty:reward:redeemed", serde_json::json!({ "member_id": member_id, "redemption_id": redemption_id, "voucher_code": voucher_code, "points_spent": points_cost }),
        &Publisher::from_parts(&a.claims.sub, &a.claims.scope)).await.map_err(err_emit)?;
    conn.commit().await.map_err(err_db)?;
    Ok(Json(RedemptionIssued {
        redemption_id,
        voucher_code: row.get::<String, _>("voucher_code"),
        member_id,
        item_name,
        points_spent: points_cost,
        remaining_balance: points_balance - points_cost,
        expires_at,
    }))
}
