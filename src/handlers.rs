// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Bill8:Billing
// SPDX-License-Identifier: Apache-2.0

//! Billing HTTP handlers — products, prices, subscriptions, invoices,
//! usage records. Invoices are immutable once finalized; usage
//! aggregation is a deterministic SUM at invoice time.

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use bill8::billing::contracts::{
    CreateSubscriptionRequest, CreateUsageRecordRequest, Invoice, Price, Product, Subscription,
    UsageRecord,
};
use rust_decimal::Decimal;
use sqlx::Row;
pub mod billing_richness;
pub mod invoice_runs;

use std::sync::Arc;
use uuid::Uuid;

use crate::auth;
use crate::config::BillingConfig;
use crate::proration;
use crate::policy::Engine;

/// Shared application state bound into the shell.
#[derive(Clone)]
pub struct AppState {
    /// Service configuration.
    pub config: Arc<BillingConfig>,
    /// Capped PostgreSQL pool.
    pub pool: sqlx::PgPool,
    /// M2M-backed Bus client.
    pub bus_client: sentin8::bus::client::BusClient,
    /// Shield policy engine — step 5 is the ONLY gate.
    pub policy: Arc<Engine>,
}
// (The billing handler modules are declared above; this file holds the
// shared state + tenant-context helpers every module uses.)
/// Set the transaction-local RLS tenant context.
pub async fn set_tenant_context(
    conn: &mut sqlx::PgConnection,
    env_id: uuid::Uuid,
) -> Result<(), sqlx::Error> {
    sqlx::query("SELECT sentin8_set_tenant_context($1, NULL, NULL, NULL, false, false, false, NULL, 'billing')")
        .bind(env_id)
        .execute(conn)
        .await
        .map(|_| ())
}

/// Standard DB error → HTTP 500.
pub fn err_db(e: sqlx::Error) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, e.to_string())
}

/// Best-effort outbox emit.
pub async fn emit_event(
    pool: &sqlx::PgPool,
    env_id: uuid::Uuid,
    event_type: &str,
    payload: serde_json::Value,
) {
    if let Err(e) =
        sentin8::bus::outbox::insert_outbox_row_pool(pool, env_id, "billing", event_type, &payload).await
    {
        tracing::warn!(error = %e, event_type, "bill8:billing: failed to emit outbox event");
    }
}

/// Health probe payload.
pub async fn health() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "status": "ok",
        "service": "billing",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

/// `POST /v1/products` — create a product.
#[utoipa::path(
    post,
    path = "/v1/products",
    responses(
        (status = 200, description = "Product created", body = Product),
        (status = 403, description = "Missing bill8:billing:products:write"),
    ),
    security(("bill8:billing:products:write" = [])),
    tag = "billing",
)]
pub async fn create_product(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<Product>, (StatusCode, String)> {
    let a = auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("bill8:billing:products:write"),
    ).await?;
    let name = body["name"].as_str().unwrap_or_default().trim().to_string();
    if name.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "name required".into()));
    }

    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, a.env_id()).await.map_err(err_db)?;
    let id = Uuid::new_v4();
    let row = sqlx::query(
        r#"INSERT INTO billing.products (id, env_id, name)
           VALUES ($1, $2, $3)
           RETURNING id, env_id, name, status, created_at"#,
    )
    .bind(id)
    .bind(a.env_id())
    .bind(&name)
    .fetch_one(&mut *conn)
    .await
    .map_err(err_db)?;
    conn.commit().await.map_err(err_db)?;

    Ok(Json(Product {
        id: row.get("id"),
        env_id: row.get("env_id"),
        name: row.get("name"),
        status: row.get("status"),
        created_at: row.get("created_at"),
    }))
}

/// `POST /v1/prices` — create a price (immutable once referenced).
#[utoipa::path(
    post,
    path = "/v1/prices",
    request_body = Price,
    responses(
        (status = 200, description = "Price created", body = Price),
        (status = 403, description = "Missing bill8:billing:prices:write"),
    ),
    security(("bill8:billing:prices:write" = [])),
    tag = "billing",
)]
pub async fn create_price(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<serde_json::Value>,
) -> Result<Json<Price>, (StatusCode, String)> {
    let a = auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("bill8:billing:prices:write"),
    ).await?;
    let product_id: Uuid = body["product_id"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "product_id required".into()))?;
    let interval = body["interval"].as_str().filter(|s| *s != "null");
    let unit_amount: Decimal = body["unit_amount"]
        .as_str()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| (StatusCode::BAD_REQUEST, "unit_amount must be a decimal string".into()))?;
    let currency = body["currency"].as_str().unwrap_or("USD").to_string();
    let pricing_model = body["pricing_model"].as_str().unwrap_or("flat").to_string();

    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, a.env_id()).await.map_err(err_db)?;
    let id = Uuid::new_v4();
    let row = sqlx::query(
        r#"INSERT INTO billing.prices (id, env_id, product_id, interval, pricing_model, unit_amount, currency)
           VALUES ($1, $2, $3, $4, $5, $6, $7)
           RETURNING id, product_id, interval, pricing_model, unit_amount, currency, created_at"#,
    )
    .bind(id)
    .bind(a.env_id())
    .bind(product_id)
    .bind(interval)
    .bind(&pricing_model)
    .bind(unit_amount)
    .bind(&currency)
    .fetch_one(&mut *conn)
    .await
    .map_err(err_db)?;
    conn.commit().await.map_err(err_db)?;

    Ok(Json(Price {
        id: row.get("id"),
        product_id: row.get("product_id"),
        interval: row.get("interval"),
        pricing_model: row.get("pricing_model"),
        unit_amount: row.get::<Decimal, _>("unit_amount").to_string(),
        currency: row.get("currency"),
        created_at: row.get("created_at"),
    }))
}

/// `GET /v1/prices` — list prices.
#[utoipa::path(
    get,
    path = "/v1/prices",
    responses(
        (status = 200, description = "List of prices", body = [Price]),
        (status = 403, description = "Missing bill8:billing:prices:read"),
    ),
    security(("bill8:billing:prices:read" = [])),
    tag = "billing",
)]
pub async fn list_prices(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Price>>, (StatusCode, String)> {
    let a = auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("bill8:billing:prices:read"),
    ).await?;

    let mut conn = state.pool.acquire().await.map_err(err_db)?;
    set_tenant_context(&mut conn, a.env_id()).await.map_err(err_db)?;

    let rows = sqlx::query(
        "SELECT id, product_id, interval, pricing_model, unit_amount, currency, created_at FROM billing.prices ORDER BY created_at DESC LIMIT 500",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(err_db)?;

    Ok(Json(
        rows.iter()
            .map(|r| Price {
                id: r.get("id"),
                product_id: r.get("product_id"),
                interval: r.get("interval"),
                pricing_model: r.get("pricing_model"),
                unit_amount: r.get::<Decimal, _>("unit_amount").to_string(),
                currency: r.get("currency"),
                created_at: r.get("created_at"),
            })
            .collect(),
    ))
}

/// `POST /v1/subscriptions` — subscribe a customer to a price (with
/// trial support and proration-consistent cycle math).
#[utoipa::path(
    post,
    path = "/v1/subscriptions",
    request_body = CreateSubscriptionRequest,
    responses(
        (status = 200, description = "Subscription created", body = Subscription),
        (status = 403, description = "Missing bill8:billing:subscriptions:write"),
    ),
    security(("bill8:billing:subscriptions:write" = [])),
    tag = "billing",
)]
pub async fn create_subscription(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateSubscriptionRequest>,
) -> Result<Json<Subscription>, (StatusCode, String)> {
    let a = auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("bill8:billing:subscriptions:write"),
    ).await?;

    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, a.env_id()).await.map_err(err_db)?;

    // Idempotency.
    if let Some(key) = req.idempotency_key.as_deref() {
        if let Ok(Some(row)) = sqlx::query(
            "SELECT id FROM billing.subscriptions WHERE env_id = $1 AND idempotency_key = $2",
        )
        .bind(a.env_id())
        .bind(key)
        .fetch_optional(&mut *conn)
        .await
        {
            let id: Uuid = row.get("id");
            conn.commit().await.map_err(err_db)?;
            return get_subscription_row(&state, a.env_id(), id).await;
        }
    }

    // Resolve the price's interval for the cycle math.
    let price = sqlx::query(
        "SELECT interval FROM billing.prices WHERE id = $1",
    )
    .bind(req.price_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(err_db)?
    .ok_or_else(|| (StatusCode::NOT_FOUND, "price not found".into()))?;
    let interval: Option<String> = price.get("interval");
    let period = interval.clone().unwrap_or_else(|| "month".into());

    let now = chrono::Utc::now();
    let anchor = req.billing_cycle_anchor.unwrap_or(now);
    let current_period_end = proration::next_cycle_start(anchor, &period);

    let status = if req.trial_end.is_some() { "trialing" } else { "incomplete" };
    let id = Uuid::new_v4();
    let row = sqlx::query(
        r#"INSERT INTO billing.subscriptions
               (id, env_id, customer_id, status, trial_end, billing_cycle_anchor,
                current_period_end, idempotency_key)
           VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
           RETURNING id, env_id, customer_id, status, trial_end, billing_cycle_anchor, current_period_end, created_at"#,
    )
    .bind(id)
    .bind(a.env_id())
    .bind(req.customer_id)
    .bind(status)
    .bind(req.trial_end)
    .bind(anchor)
    .bind(current_period_end)
    .bind(req.idempotency_key.as_deref())
    .fetch_one(&mut *conn)
    .await
    .map_err(err_db)?;

    sqlx::query(
        r#"INSERT INTO billing.subscription_items (env_id, subscription_id, price_id, quantity)
           VALUES ($1, $2, $3, $4)"#,
    )
    .bind(a.env_id())
    .bind(id)
    .bind(req.price_id)
    .bind(req.quantity as i32)
    .execute(&mut *conn)
    .await
    .map_err(err_db)?;

    sqlx::query(
        r#"INSERT INTO billing.outbox (env_id, event_type, payload)
           VALUES ($1, 'bill8:billing:subscription:created', $2)"#,
    )
    .bind(a.env_id())
    .bind(serde_json::json!({ "subscription_id": id, "customer_id": req.customer_id }))
    .execute(&mut *conn)
    .await
    .map_err(err_db)?;

    conn.commit().await.map_err(err_db)?;

    let sub = row_to_subscription(&row);
    emit_event(
        &state.pool,
        a.env_id(),
        "bill8:billing:subscription:created",
        serde_json::json!({ "subscription_id": id }),
    )
    .await;
    Ok(Json(sub))
}

/// `GET /v1/subscriptions` — list subscriptions.
#[utoipa::path(
    get,
    path = "/v1/subscriptions",
    responses(
        (status = 200, description = "List of subscriptions", body = [Subscription]),
        (status = 403, description = "Missing bill8:billing:subscriptions:read"),
    ),
    security(("bill8:billing:subscriptions:read" = [])),
    tag = "billing",
)]
pub async fn list_subscriptions(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Subscription>>, (StatusCode, String)> {
    let a = auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("bill8:billing:subscriptions:read"),
    ).await?;

    let mut conn = state.pool.acquire().await.map_err(err_db)?;
    set_tenant_context(&mut conn, a.env_id()).await.map_err(err_db)?;

    let rows = sqlx::query(
        "SELECT id, env_id, customer_id, status, trial_end, billing_cycle_anchor, current_period_end, created_at FROM billing.subscriptions ORDER BY created_at DESC LIMIT 200",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(err_db)?;

    Ok(Json(rows.iter().map(row_to_subscription).collect()))
}

/// `GET /v1/subscriptions/{id}` — read one subscription.
#[utoipa::path(
    get,
    path = "/v1/subscriptions/{id}",
    responses(
        (status = 200, description = "The subscription", body = Subscription),
        (status = 404, description = "Subscription not found"),
    ),
    security(("bill8:billing:subscriptions:read" = [])),
    tag = "billing",
)]
pub async fn get_subscription(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Subscription>, (StatusCode, String)> {
    let a = auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("bill8:billing:subscriptions:read"),
    ).await?;
    get_subscription_row(&state, a.env_id(), id).await
}

/// `POST /v1/usage-records` — meter a usage event (deterministic
/// aggregation at invoice time; duplicate keys counted once).
#[utoipa::path(
    post,
    path = "/v1/usage-records",
    request_body = CreateUsageRecordRequest,
    responses(
        (status = 200, description = "Usage recorded", body = UsageRecord),
        (status = 403, description = "Missing bill8:billing:usage-records:write"),
    ),
    security(("bill8:billing:usage-records:write" = [])),
    tag = "billing",
)]
pub async fn create_usage_record(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateUsageRecordRequest>,
) -> Result<Json<UsageRecord>, (StatusCode, String)> {
    let a = auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("bill8:billing:usage-records:write"),
    ).await?;

    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, a.env_id()).await.map_err(err_db)?;

    // Duplicate idempotency key → return the original (counted once).
    if let Some(key) = req.idempotency_key.as_deref() {
        if let Ok(Some(row)) = sqlx::query(
            "SELECT id, subscription_item_id, quantity, recorded_at, created_at FROM billing.usage_records WHERE env_id = $1 AND idempotency_key = $2",
        )
        .bind(a.env_id())
        .bind(key)
        .fetch_optional(&mut *conn)
        .await
        {
            conn.commit().await.map_err(err_db)?;
            return Ok(Json(row_to_usage(&row)));
        }
    }

    let id = Uuid::new_v4();
    let row = sqlx::query(
        r#"INSERT INTO billing.usage_records (id, env_id, subscription_item_id, quantity, idempotency_key)
           VALUES ($1, $2, $3, $4, $5)
           RETURNING id, subscription_item_id, quantity, recorded_at, created_at"#,
    )
    .bind(id)
    .bind(a.env_id())
    .bind(req.subscription_item_id)
    .bind(req.quantity as i32)
    .bind(req.idempotency_key.as_deref())
    .fetch_one(&mut *conn)
    .await
    .map_err(err_db)?;

    conn.commit().await.map_err(err_db)?;
    Ok(Json(row_to_usage(&row)))
}

/// `GET /v1/invoices` — list invoices.
#[utoipa::path(
    get,
    path = "/v1/invoices",
    responses(
        (status = 200, description = "List of invoices", body = [Invoice]),
        (status = 403, description = "Missing bill8:billing:invoices:read"),
    ),
    security(("bill8:billing:invoices:read" = [])),
    tag = "billing",
)]
pub async fn list_invoices(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Invoice>>, (StatusCode, String)> {
    let a = auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("bill8:billing:invoices:read"),
    ).await?;

    let mut conn = state.pool.acquire().await.map_err(err_db)?;
    set_tenant_context(&mut conn, a.env_id()).await.map_err(err_db)?;

    let rows = sqlx::query(
        "SELECT id, env_id, customer_id, subscription_id, amount_due, amount_paid, currency, status, period_start, period_end, created_at FROM billing.invoices ORDER BY created_at DESC LIMIT 200",
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(err_db)?;

    Ok(Json(rows.iter().map(row_to_invoice).collect()))
}

async fn get_subscription_row(
    state: &AppState,
    env_id: Uuid,
    id: Uuid,
) -> Result<Json<Subscription>, (StatusCode, String)> {
    let mut conn = state.pool.acquire().await.map_err(err_db)?;
    set_tenant_context(&mut conn, env_id).await.map_err(err_db)?;
    let row = sqlx::query(
        "SELECT id, env_id, customer_id, status, trial_end, billing_cycle_anchor, current_period_end, created_at FROM billing.subscriptions WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(err_db)?
    .ok_or_else(|| (StatusCode::NOT_FOUND, "subscription not found".into()))?;
    Ok(Json(row_to_subscription(&row)))
}

fn row_to_subscription(row: &sqlx::postgres::PgRow) -> Subscription {
    Subscription {
        id: row.get("id"),
        env_id: row.get("env_id"),
        customer_id: row.get("customer_id"),
        status: row.get("status"),
        trial_end: row.get("trial_end"),
        billing_cycle_anchor: row.get("billing_cycle_anchor"),
        current_period_end: row.get("current_period_end"),
        created_at: row.get("created_at"),
    }
}

fn row_to_usage(row: &sqlx::postgres::PgRow) -> UsageRecord {
    UsageRecord {
        id: row.get("id"),
        subscription_item_id: row.get("subscription_item_id"),
        quantity: row.get::<i32, _>("quantity") as i64,
        recorded_at: row.get("recorded_at"),
        created_at: row.get("created_at"),
    }
}

fn row_to_invoice(row: &sqlx::postgres::PgRow) -> Invoice {
    Invoice {
        id: row.get("id"),
        env_id: row.get("env_id"),
        customer_id: row.get("customer_id"),
        subscription_id: row.get("subscription_id"),
        amount_due: row.get::<Decimal, _>("amount_due").to_string(),
        amount_paid: row.get::<Decimal, _>("amount_paid").to_string(),
        currency: row.get("currency"),
        status: row.get("status"),
        period_start: row.get("period_start"),
        period_end: row.get("period_end"),
        created_at: row.get("created_at"),
    }
}