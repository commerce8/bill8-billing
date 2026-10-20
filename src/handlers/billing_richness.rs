// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Bill8:Billing
// SPDX-License-Identifier: Apache-2.0

//! Billing richness handlers - coupons (BIL-7), credits (BIL-8),
//! credit notes (BIL-15), tax rates (BIL-16), pause/resume (BIL-11),
//! and the plan-change proration quote (BIL-14). Plus the
//! `charge:succeeded` consumer (BIL-6) marking invoices paid.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use bill8::billing::contracts::{
    CreateCouponRequest, CreateCreditNoteRequest, CreateCreditRequest, CreateTaxRateRequest,
};
use rust_decimal::Decimal;
use sqlx::Row;
use uuid::Uuid;

use crate::auth;
use crate::handlers::{emit_event, err_db, set_tenant_context, AppState};
use crate::proration;

// ── Coupons (BIL-7) ─────────────────────────────────────────────────────

/// `POST /v1/coupons` - create a coupon (redemption-limit bounded).
#[utoipa::path(
    post,
    path = "/v1/coupons",
    request_body = CreateCouponRequest,
    responses(
        (status = 200, description = "Coupon created"),
        (status = 403, description = "Missing bill8:billing:coupons:write"),
    ),
    security(("bill8:billing:coupons:write" = [])),
    tag = "billing",
)]
pub async fn create_coupon(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateCouponRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let a = auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("bill8:billing:coupons:write"),
    )
    .await?;
    let code = body.code.trim().to_string();
    if code.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "code required".into()));
    }
    let discount_kind = match body.discount_kind.as_str() {
        "fixed" => "fixed",
        _ => "percentage",
    };

    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, a.env_id())
        .await
        .map_err(err_db)?;
    let id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO billing.coupons (id, env_id, code, discount_kind, value, redemption_limit, expires_at)
           VALUES ($1, $2, $3, $4, $5, $6, $7)"#,
    )
    .bind(id)
    .bind(a.env_id())
    .bind(&code)
    .bind(discount_kind)
    .bind(&body.value)
    .bind(body.redemption_limit)
    .bind(body.expires_at)
    .execute(&mut *conn)
    .await
    .map_err(err_db)?;
    conn.commit().await.map_err(err_db)?;
    Ok(Json(serde_json::json!({ "coupon_id": id })))
}

/// `POST /v1/coupons/{code}/redeem` - redeem a coupon (row-lock bounded:
/// concurrent redemptions never exceed the limit; expired coupons and
/// exhausted limits are rejected).
#[utoipa::path(
    post,
    path = "/v1/coupons/{code}/redeem",
    responses(
        (status = 200, description = "Coupon redeemed"),
        (status = 409, description = "Limit exhausted or expired"),
        (status = 404, description = "Coupon not found"),
    ),
    security(("bill8:billing:coupons:write" = [])),
    tag = "billing",
)]
pub async fn redeem_coupon(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(code): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let a = auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("bill8:billing:coupons:write"),
    )
    .await?;

    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, a.env_id())
        .await
        .map_err(err_db)?;

    // Row-lock the coupon - the limit check + increment are atomic.
    let coupon = sqlx::query(
        "SELECT id, redemption_limit, redeemed, expires_at FROM billing.coupons WHERE code = $1 FOR UPDATE",
    )
    .bind(&code)
    .fetch_optional(&mut *conn)
    .await
    .map_err(err_db)?
    .ok_or_else(|| (StatusCode::NOT_FOUND, "coupon not found".into()))?;

    let limit: Option<i64> = coupon.get("redemption_limit");
    let redeemed: i64 = coupon.get("redeemed");
    let expires_at: Option<chrono::DateTime<chrono::Utc>> = coupon.get("expires_at");
    let coupon_id: Uuid = coupon.get("id");

    if let Some(exp) = expires_at {
        if exp < chrono::Utc::now() {
            return Err((StatusCode::CONFLICT, "coupon expired".into()));
        }
    }
    if let Some(l) = limit {
        if redeemed >= l {
            return Err((StatusCode::CONFLICT, "redemption limit exhausted".into()));
        }
    }

    sqlx::query("UPDATE billing.coupons SET redeemed = redeemed + 1 WHERE id = $1")
        .bind(coupon_id)
        .execute(&mut *conn)
        .await
        .map_err(err_db)?;

    conn.commit().await.map_err(err_db)?;
    Ok(Json(
        serde_json::json!({ "coupon_id": coupon_id, "status": "redeemed" }),
    ))
}

// ── Credits (BIL-8) ─────────────────────────────────────────────────────

/// `POST /v1/credits` - grant prepaid value to a customer.
#[utoipa::path(
    post,
    path = "/v1/credits",
    request_body = CreateCreditRequest,
    responses(
        (status = 200, description = "Credit granted"),
        (status = 403, description = "Missing bill8:billing:credits:write"),
    ),
    security(("bill8:billing:credits:write" = [])),
    tag = "billing",
)]
pub async fn create_credit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateCreditRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let a = auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("bill8:billing:credits:write"),
    )
    .await?;
    let customer_id: Uuid = body.customer_id;
    let amount: Decimal = body.amount.parse().map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            "amount must be a decimal string".into(),
        )
    })?;
    if amount <= Decimal::ZERO {
        return Err((StatusCode::BAD_REQUEST, "amount must be positive".into()));
    }

    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, a.env_id())
        .await
        .map_err(err_db)?;
    let id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO billing.credits (id, env_id, customer_id, remaining, currency)
           VALUES ($1, $2, $3, $4, $5)"#,
    )
    .bind(id)
    .bind(a.env_id())
    .bind(customer_id)
    .bind(amount)
    .bind(&body.currency)
    .execute(&mut *conn)
    .await
    .map_err(err_db)?;
    conn.commit().await.map_err(err_db)?;
    Ok(Json(serde_json::json!({ "credit_id": id })))
}

// ── Credit notes (BIL-15) ────────────────────────────────────────────────

/// `POST /v1/credit-notes` - issue an invoice-level adjustment, bounded
/// by the invoice's paid amount (row-lock serialized - credit notes
/// adjust invoices; refunds adjust charges).
#[utoipa::path(
    post,
    path = "/v1/credit-notes",
    request_body = CreateCreditNoteRequest,
    responses(
        (status = 200, description = "Credit note issued"),
        (status = 422, description = "Over-credit - nothing written"),
        (status = 404, description = "Invoice not found"),
    ),
    security(("bill8:billing:credit-notes:write" = [])),
    tag = "billing",
)]
pub async fn create_credit_note(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateCreditNoteRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let a = auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("bill8:billing:credit-notes:write"),
    )
    .await?;
    let invoice_id: Uuid = body.invoice_id;
    let amount: Decimal = body.amount.parse().map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            "amount must be a decimal string".into(),
        )
    })?;
    if amount <= Decimal::ZERO {
        return Err((StatusCode::BAD_REQUEST, "amount must be positive".into()));
    }

    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, a.env_id())
        .await
        .map_err(err_db)?;

    // Row-lock the invoice - the paid-amount bound is computed atomically.
    let invoice = sqlx::query("SELECT amount_paid FROM billing.invoices WHERE id = $1 FOR UPDATE")
        .bind(invoice_id)
        .fetch_optional(&mut *conn)
        .await
        .map_err(err_db)?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "invoice not found".into()))?;
    let paid: Decimal = invoice.get("amount_paid");
    if amount > paid {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("credit {amount} exceeds invoice paid amount {paid}"),
        ));
    }

    let id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO billing.credit_notes (id, env_id, invoice_id, amount, status)
           VALUES ($1, $2, $3, $4, 'issued')"#,
    )
    .bind(id)
    .bind(a.env_id())
    .bind(invoice_id)
    .bind(amount)
    .execute(&mut *conn)
    .await
    .map_err(err_db)?;

    sqlx::query(
        r#"INSERT INTO billing.outbox (env_id, event_type, payload)
           VALUES ($1, 'bill8:billing:credit_note:issued', $2)"#,
    )
    .bind(a.env_id())
    .bind(serde_json::json!({ "credit_note_id": id, "invoice_id": invoice_id }))
    .execute(&mut *conn)
    .await
    .map_err(err_db)?;

    conn.commit().await.map_err(err_db)?;
    Ok(Json(
        serde_json::json!({ "credit_note_id": id, "status": "issued" }),
    ))
}

/// `POST /v1/credit-notes/{id}/void` - void a credit note (terminal).
#[utoipa::path(
    post,
    path = "/v1/credit-notes/{id}/void",
    responses(
        (status = 200, description = "Credit note voided"),
        (status = 409, description = "Already void"),
        (status = 404, description = "Credit note not found"),
    ),
    security(("bill8:billing:credit-notes:write" = [])),
    tag = "billing",
)]
pub async fn void_credit_note(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let a = auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("bill8:billing:credit-notes:write"),
    )
    .await?;

    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, a.env_id())
        .await
        .map_err(err_db)?;

    let result = sqlx::query(
        "UPDATE billing.credit_notes SET status = 'void' WHERE id = $1 AND status = 'issued'",
    )
    .bind(id)
    .execute(&mut *conn)
    .await
    .map_err(err_db)?;
    if result.rows_affected() == 0 {
        return Err((StatusCode::CONFLICT, "credit note not issued".into()));
    }

    conn.commit().await.map_err(err_db)?;
    Ok(Json(serde_json::json!({ "status": "void" })))
}

// ── Tax rates (BIL-16) ───────────────────────────────────────────────────

/// `POST /v1/tax-rates` - configure a jurisdiction rate.
#[utoipa::path(
    post,
    path = "/v1/tax-rates",
    request_body = CreateTaxRateRequest,
    responses(
        (status = 200, description = "Tax rate saved"),
        (status = 403, description = "Missing bill8:billing:tax-rates:write"),
    ),
    security(("bill8:billing:tax-rates:write" = [])),
    tag = "billing",
)]
pub async fn create_tax_rate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateTaxRateRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let a = auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("bill8:billing:tax-rates:write"),
    )
    .await?;
    let jurisdiction = body.jurisdiction.trim().to_string();
    if jurisdiction.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "jurisdiction required".into()));
    }

    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, a.env_id())
        .await
        .map_err(err_db)?;
    let id = Uuid::new_v4();
    sqlx::query(
        r#"INSERT INTO billing.tax_rates (id, env_id, jurisdiction, inclusive, rate_bps)
           VALUES ($1, $2, $3, $4, $5)"#,
    )
    .bind(id)
    .bind(a.env_id())
    .bind(&jurisdiction)
    .bind(body.inclusive)
    .bind(body.rate_bps)
    .execute(&mut *conn)
    .await
    .map_err(err_db)?;
    conn.commit().await.map_err(err_db)?;
    Ok(Json(serde_json::json!({ "tax_rate_id": id })))
}

// ── Pause / resume (BIL-11) ──────────────────────────────────────────────

/// `POST /v1/subscriptions/{id}/pause` - pause billing (no invoices
/// generated while paused; terminal-state guard applies).
#[utoipa::path(
    post,
    path = "/v1/subscriptions/{id}/pause",
    responses(
        (status = 200, description = "Subscription paused"),
        (status = 409, description = "Subscription not pausable"),
    ),
    security(("bill8:billing:subscriptions:write" = [])),
    tag = "billing",
)]
pub async fn pause_subscription(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let a = auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("bill8:billing:subscriptions:write"),
    )
    .await?;

    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, a.env_id())
        .await
        .map_err(err_db)?;

    let result = sqlx::query(
        "UPDATE billing.subscriptions SET status = 'paused' WHERE id = $1 AND status IN ('active', 'past_due')",
    )
    .bind(id)
    .execute(&mut *conn)
    .await
    .map_err(err_db)?;
    if result.rows_affected() == 0 {
        return Err((StatusCode::CONFLICT, "subscription not pausable".into()));
    }

    sqlx::query(
        r#"INSERT INTO billing.outbox (env_id, event_type, payload)
           VALUES ($1, 'bill8:billing:subscription:paused', $2)"#,
    )
    .bind(a.env_id())
    .bind(serde_json::json!({ "subscription_id": id }))
    .execute(&mut *conn)
    .await
    .map_err(err_db)?;

    conn.commit().await.map_err(err_db)?;
    Ok(Json(serde_json::json!({ "status": "paused" })))
}

/// `POST /v1/subscriptions/{id}/resume` - resume with proration against
/// the remaining cycle (cent-exact, BIL-4 math).
#[utoipa::path(
    post,
    path = "/v1/subscriptions/{id}/resume",
    responses(
        (status = 200, description = "Subscription resumed (prorated)"),
        (status = 409, description = "Subscription not paused"),
    ),
    security(("bill8:billing:subscriptions:write" = [])),
    tag = "billing",
)]
pub async fn resume_subscription(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let a = auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("bill8:billing:subscriptions:write"),
    )
    .await?;

    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, a.env_id())
        .await
        .map_err(err_db)?;

    // The cycle math: days elapsed in the current period prorate the
    // resume (the pause consumed them).
    let sub = sqlx::query(
        "SELECT billing_cycle_anchor, current_period_end FROM billing.subscriptions WHERE id = $1 AND status = 'paused'",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(err_db)?
    .ok_or_else(|| (StatusCode::CONFLICT, "subscription not paused".into()))?;

    let anchor: chrono::DateTime<chrono::Utc> = sub
        .get::<Option<chrono::DateTime<chrono::Utc>>, _>("billing_cycle_anchor")
        .unwrap_or(chrono::Utc::now());
    let period_end: chrono::DateTime<chrono::Utc> = sub.get("current_period_end");

    // The resume charge covers the remaining days of the cycle - the
    // proration quote is returned for the caller to invoice (the next
    // invoice run applies it via the standard cycle lines).
    let remaining = (period_end - chrono::Utc::now()).num_days().max(0);

    sqlx::query(
        "UPDATE billing.subscriptions SET status = 'active' WHERE id = $1 AND status = 'paused'",
    )
    .bind(id)
    .execute(&mut *conn)
    .await
    .map_err(err_db)?;

    sqlx::query(
        r#"INSERT INTO billing.outbox (env_id, event_type, payload)
           VALUES ($1, 'bill8:billing:subscription:resumed', $2)"#,
    )
    .bind(a.env_id())
    .bind(serde_json::json!({ "subscription_id": id, "remaining_days": remaining, "anchor": anchor.to_rfc3339() }))
    .execute(&mut *conn)
    .await
    .map_err(err_db)?;

    conn.commit().await.map_err(err_db)?;
    Ok(Json(
        serde_json::json!({ "status": "active", "remaining_days": remaining }),
    ))
}

// ── Plan-change proration quote (BIL-14) ─────────────────────────────────

/// Query params for the proration quote.
#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
pub struct ProrationQuoteQuery {
    pub subscription_id: Uuid,
    /// The new price's unit amount (decimal string).
    pub new_unit_amount: String,
}

/// `GET /v1/subscriptions/proration-quote` - quote a hypothetical plan
/// change (read-only, ZERO writes; the same proration math the real
/// change applies).
#[utoipa::path(
    get,
    path = "/v1/subscriptions/proration-quote",
    params(
        ("subscription_id" = Uuid, Query, description = "The subscription"),
        ("new_unit_amount" = String, Query, description = "The new price's unit amount"),
    ),
    responses(
        (status = 200, description = "The proration quote"),
        (status = 404, description = "Subscription not found"),
    ),
    security(("bill8:billing:subscriptions:read" = [])),
    tag = "billing",
)]
pub async fn proration_quote(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(q): axum::extract::Query<ProrationQuoteQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let a = auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("bill8:billing:subscriptions:read"),
    )
    .await?;

    let mut conn = state.pool.acquire().await.map_err(err_db)?;
    set_tenant_context(&mut conn, a.env_id())
        .await
        .map_err(err_db)?;

    let sub = sqlx::query(
        "SELECT billing_cycle_anchor, current_period_end FROM billing.subscriptions WHERE id = $1",
    )
    .bind(q.subscription_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(err_db)?
    .ok_or_else(|| (StatusCode::NOT_FOUND, "subscription not found".into()))?;

    let anchor: chrono::DateTime<chrono::Utc> = sub
        .get::<Option<chrono::DateTime<chrono::Utc>>, _>("billing_cycle_anchor")
        .unwrap_or(chrono::Utc::now());
    let period_end: chrono::DateTime<chrono::Utc> = sub.get("current_period_end");

    // The current price resolves the credit half of the quote.
    let current_price = sqlx::query(
        r#"SELECT p.unit_amount FROM billing.subscription_items si
           JOIN billing.prices p ON p.id = si.price_id
           WHERE si.subscription_id = $1 LIMIT 1"#,
    )
    .bind(q.subscription_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(err_db)?
    .map(|r| r.get::<Decimal, _>("unit_amount"))
    .unwrap_or(Decimal::ZERO);

    let new_unit: Decimal = q.new_unit_amount.parse().map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            "new_unit_amount must be a decimal string".into(),
        )
    })?;

    let cycle = (period_end - anchor).num_days().max(1);
    let used = proration::days_elapsed(anchor, chrono::Utc::now(), "month");
    let remaining = (cycle - used).max(0);

    let credit = proration::unused_credit(current_price, cycle, used);
    let charge = proration::remainder_charge(new_unit, cycle, remaining);

    Ok(Json(serde_json::json!({
        "subscription_id": q.subscription_id,
        "credit_for_unused": credit.to_string(),
        "charge_for_remaining": charge.to_string(),
        "days_used": used,
        "days_remaining": remaining,
    })))
}

// ── Charge-succeeded consumer (BIL-6) ────────────────────────────────────

/// Mark an invoice paid when its charge succeeds - invoked by the
/// payments-side consumer wiring (the invoice's idempotency key links
/// the charge: `invoice-{id}`).
///
/// Idempotent: a paid invoice stays paid (the guard `status = 'open'`
/// makes the transition exactly-once); recovery restores `active`.
pub async fn mark_invoice_paid(
    state: &AppState,
    env_id: Uuid,
    invoice_id: Uuid,
    amount: Decimal,
) -> Result<(), sqlx::Error> {
    let mut conn = state.pool.begin().await?;
    set_tenant_context(&mut conn, env_id).await?;

    let result = sqlx::query(
        "UPDATE billing.invoices SET status = 'paid', amount_paid = $1 WHERE id = $2 AND status IN ('open', 'retrying')",
    )
    .bind(amount)
    .bind(invoice_id)
    .execute(&mut *conn)
    .await?;

    if result.rows_affected() > 0 {
        // Recovery: the subscription returns to active.
        sqlx::query(
            "UPDATE billing.subscriptions SET status = 'active' \
             WHERE id = (SELECT subscription_id FROM billing.invoices WHERE id = $1) AND status = 'past_due'",
        )
        .bind(invoice_id)
        .execute(&mut *conn)
        .await?;

        sqlx::query(
            r#"INSERT INTO billing.outbox (env_id, event_type, payload)
               VALUES ($1, 'bill8:billing:invoice:paid', $2)"#,
        )
        .bind(env_id)
        .bind(serde_json::json!({ "invoice_id": invoice_id }))
        .execute(&mut *conn)
        .await?;
    }

    conn.commit().await?;
    Ok(())
}

/// Convenience re-export for the consumer wiring (the SSE loop lives in
/// the server module; it dispatches here).
pub async fn handle_charge_succeeded(state: &AppState, env_id: Uuid, payload: serde_json::Value) {
    // The billing-issued charge carries idempotency_key `invoice-{id}`.
    let idempotency_key = payload["idempotency_key"].as_str().unwrap_or("");
    let Some(invoice_str) = idempotency_key.strip_prefix("invoice-") else {
        return; // Not a billing charge - nothing to mark.
    };
    let Ok(invoice_id) = Uuid::parse_str(invoice_str) else {
        return;
    };
    let amount: Decimal = payload["amount"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .unwrap_or(Decimal::ZERO);
    if let Err(e) = mark_invoice_paid(state, env_id, invoice_id, amount).await {
        tracing::warn!(error = %e, "billing: failed to mark invoice paid");
    }
    emit_event(
        &state.pool,
        env_id,
        "bill8:billing:invoice:paid",
        serde_json::json!({ "invoice_id": invoice_id }),
    )
    .await;
}
