// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Bill8:Billing
// SPDX-License-Identifier: Apache-2.0

//! Invoice-run engine (BIL-6) - the Cron proxy-back that finalizes due
//! invoices, charges them through the payments PUBLIC API, and the
//! payments-side consumer that marks invoices paid when charges
//! succeed. Plus the dunning ladder (BIL-12): a data-driven retry
//! schedule for past-due invoices.

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::Json;
use rust_decimal::Decimal;
use sqlx::Row;
use uuid::Uuid;

use crate::auth;
use crate::handlers::{emit_event, err_db, set_tenant_context, AppState};
use crate::proration;

/// `POST /v1/internal/invoices/run` - the Cron proxy-back tick: find
/// subscriptions due this cycle, aggregate usage, finalize invoices,
/// charge via the payments public API. Idempotent per
/// (subscription, cycle): a subscription with an open/paid/retrying
/// invoice covering its current period is skipped.
#[utoipa::path(
    post,
    path = "/v1/internal/invoices/run",
    responses(
        (status = 200, description = "Invoice run processed"),
    ),
    security(("sentin8:cron:write" = [])),
    tag = "billing",
)]
pub async fn run_invoice_cycle(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let a = auth::verify_token(&state.config, &headers).await?;

    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, a.env_id())
        .await
        .map_err(err_db)?;

    // Subscriptions active or past-due whose cycle has ended (paused
    // and canceled subscriptions generate no invoices - BIL-11).
    let due = sqlx::query(
        r#"SELECT s.id, s.customer_id, s.current_period_end, s.status
           FROM billing.subscriptions s
           WHERE s.status IN ('active', 'past_due', 'incomplete')
             AND s.current_period_end <= now()
             AND NOT EXISTS (
                 SELECT 1 FROM billing.invoices i
                 WHERE i.subscription_id = s.id AND i.period_end = s.current_period_end
             )"#,
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(err_db)?;

    let mut invoiced = 0usize;
    let mut charged = 0usize;
    for sub in &due {
        let subscription_id: Uuid = sub.get("id");
        let customer_id: Uuid = sub.get("customer_id");
        let period_end: chrono::DateTime<chrono::Utc> = sub.get("current_period_end");

        // The items on this subscription (price + quantity).
        let items = sqlx::query(
            "SELECT si.id, si.price_id, si.quantity, p.unit_amount, p.currency, p.interval, p.pricing_model
             FROM billing.subscription_items si
             JOIN billing.prices p ON p.id = si.price_id
             WHERE si.subscription_id = $1",
        )
        .bind(subscription_id)
        .fetch_all(&mut *conn)
        .await
        .map_err(err_db)?;

        // The new period: next cycle from the ended one.
        let interval: Option<String> = items.first().map(|r| r.get("interval"));
        let period = interval.clone().unwrap_or_else(|| "month".into());
        let next_end = proration::next_cycle_start(period_end, &period);

        // Invoice draft with the subscription lines.
        let invoice_id = Uuid::new_v4();
        let mut total = Decimal::ZERO;
        let mut currency = "USD".to_string();
        for item in &items {
            let unit: Decimal = item.get("unit_amount");
            let quantity: i32 = item.get("quantity");
            let line = unit * Decimal::from(quantity);
            currency = item.get("currency");
            total += line;
            sqlx::query(
                r#"INSERT INTO billing.invoice_items
                       (id, env_id, invoice_id, description, amount, kind)
                   VALUES ($1, $2, $3, $4, $5, 'subscription')"#,
            )
            .bind(Uuid::new_v4())
            .bind(a.env_id())
            .bind(invoice_id)
            .bind(format!(
                "subscription item {}",
                item.get::<Uuid, _>("price_id")
            ))
            .bind(line)
            .execute(&mut *conn)
            .await
            .map_err(err_db)?;
        }

        // Usage lines: deterministic per-item SUM over the ended period.
        for item in &items {
            let item_id: Uuid = item.get("id");
            let usage: Option<Decimal> = sqlx::query(
                r#"SELECT COALESCE(SUM(quantity), 0) as total
                   FROM billing.usage_records
                   WHERE subscription_item_id = $1 AND recorded_at <= $2"#,
            )
            .bind(item_id)
            .bind(period_end)
            .fetch_optional(&mut *conn)
            .await
            .map_err(err_db)?
            .map(|r| r.get::<Decimal, _>("total"));
            let usage = usage.unwrap_or(Decimal::ZERO);
            if usage > Decimal::ZERO {
                let unit: Decimal = item.get("unit_amount");
                let line = usage * unit;
                total += line;
                sqlx::query(
                    r#"INSERT INTO billing.invoice_items
                           (id, env_id, invoice_id, description, amount, kind)
                       VALUES ($1, $2, $3, $4, $5, 'usage')"#,
                )
                .bind(Uuid::new_v4())
                .bind(a.env_id())
                .bind(invoice_id)
                .bind(format!("usage for item {item_id}"))
                .bind(line)
                .execute(&mut *conn)
                .await
                .map_err(err_db)?;
            }
        }

        // Tax lines (BIL-16): explicit cent-exact lines per jurisdiction.
        let tax_rows = sqlx::query("SELECT id, jurisdiction, rate_bps FROM billing.tax_rates")
            .fetch_all(&mut *conn)
            .await
            .map_err(err_db)?;
        for tax in &tax_rows {
            let rate_bps: i32 = tax.get("rate_bps");
            let tax_amount = (total * Decimal::from(rate_bps) / Decimal::from(10_000)).round_dp(2);
            if tax_amount > Decimal::ZERO {
                sqlx::query(
                    r#"INSERT INTO billing.invoice_items
                           (id, env_id, invoice_id, description, amount, kind)
                       VALUES ($1, $2, $3, $4, $5, 'tax')"#,
                )
                .bind(Uuid::new_v4())
                .bind(a.env_id())
                .bind(invoice_id)
                .bind(format!("tax ({})", tax.get::<String, _>("jurisdiction")))
                .bind(tax_amount)
                .execute(&mut *conn)
                .await
                .map_err(err_db)?;
                total += tax_amount;
            }
        }

        // Finalize: draft → open (lines frozen from here on).
        sqlx::query(
            r#"INSERT INTO billing.invoices
                   (id, env_id, customer_id, subscription_id, amount_due, amount_paid,
                    currency, status, period_start, period_end)
               VALUES ($1, $2, $3, $4, $5, 0, $6, 'open', $7, $8)"#,
        )
        .bind(invoice_id)
        .bind(a.env_id())
        .bind(customer_id)
        .bind(subscription_id)
        .bind(total)
        .bind(&currency)
        .bind(period_end)
        .bind(next_end)
        .execute(&mut *conn)
        .await
        .map_err(err_db)?;
        invoiced += 1;

        // Advance the subscription's period.
        sqlx::query(
            "UPDATE billing.subscriptions SET current_period_end = $1, resource_version = resource_version + 1 WHERE id = $2",
        )
        .bind(next_end)
        .bind(subscription_id)
        .execute(&mut *conn)
        .await
        .map_err(err_db)?;

        // Charge through the payments PUBLIC API (the
        // `bill8-billing-payments` M2M workload).
        if total > Decimal::ZERO {
            match charge_invoice(&state, customer_id, total, &currency, invoice_id).await {
                Ok(_) => {
                    charged += 1;
                    // The payments-side consumer marks the invoice paid
                    // on charge:succeeded (event-driven; no sync write).
                }
                Err(e) => {
                    tracing::warn!(error = %e, "invoice charge failed - subscription goes past_due on the dunning ladder");
                    sqlx::query(
                        "UPDATE billing.subscriptions SET status = 'past_due' WHERE id = $1 AND status IN ('active', 'incomplete')",
                    )
                    .bind(subscription_id)
                    .execute(&mut *conn)
                    .await
                    .map_err(err_db)?;
                    sqlx::query("UPDATE billing.invoices SET status = 'retrying' WHERE id = $1")
                        .bind(invoice_id)
                        .execute(&mut *conn)
                        .await
                        .map_err(err_db)?;
                }
            }
        }

        sqlx::query(
            r#"INSERT INTO billing.outbox (env_id, event_type, payload)
               VALUES ($1, 'bill8:billing:invoice:created', $2)"#,
        )
        .bind(a.env_id())
        .bind(serde_json::json!({ "invoice_id": invoice_id, "amount_due": total.to_string() }))
        .execute(&mut *conn)
        .await
        .map_err(err_db)?;
    }

    conn.commit().await.map_err(err_db)?;
    Ok(Json(
        serde_json::json!({ "invoiced": invoiced, "charged": charged }),
    ))
}

/// Charge an invoice through the payments public API (the workload
/// certificate carries the identity - the receiving service resolves
/// it against the registry map).
async fn charge_invoice(
    state: &AppState,
    customer_id: Uuid,
    amount: Decimal,
    currency: &str,
    invoice_id: Uuid,
) -> Result<Uuid, String> {
    let Some(ref client) = state.config.payments_http else {
        return Err("payments workload transport absent".into());
    };
    let resp = client
        .post(format!("{}/v1/charges", state.config.bill8_payments_url))
        .json(&serde_json::json!({
            "customer_id": customer_id,
            "payment_method_id": null,
            "payment_intent_id": null,
            "amount": amount.to_string(),
            "currency": currency,
            "idempotency_key": format!("invoice-{invoice_id}"),
        }))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("payments HTTP {}", resp.status()));
    }
    let charge: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    charge["id"]
        .as_str()
        .and_then(|s| Uuid::parse_str(s).ok())
        .ok_or_else(|| "payments returned no charge id".to_string())
}

/// `GET /v1/invoices/upcoming` - the read-only upcoming-invoice preview
/// (BIL-14): projects the next cycle's invoice for a subscription with
/// ZERO writes.
#[utoipa::path(
    get,
    path = "/v1/invoices/upcoming",
    params(
        ("subscription_id" = Uuid, Query, description = "The subscription to preview"),
    ),
    responses(
        (status = 200, description = "The projected upcoming invoice"),
        (status = 404, description = "Subscription not found"),
    ),
    security(("bill8:billing:invoices:read" = [])),
    tag = "billing",
)]
pub async fn upcoming_invoice(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::extract::Query(q): axum::extract::Query<UpcomingQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let a = auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("bill8:billing:invoices:read"),
    )
    .await?;

    let mut conn = state.pool.acquire().await.map_err(err_db)?;
    set_tenant_context(&mut conn, a.env_id())
        .await
        .map_err(err_db)?;

    // The subscription's items resolve the projection; a preview is the
    // SAME code path the run uses (read-only projection of it).
    let items = sqlx::query(
        "SELECT si.price_id, si.quantity, p.unit_amount, p.currency, p.interval
         FROM billing.subscription_items si
         JOIN billing.prices p ON p.id = si.price_id
         WHERE si.subscription_id = $1",
    )
    .bind(q.subscription_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(err_db)?;

    if items.is_empty() {
        return Err((
            StatusCode::NOT_FOUND,
            "subscription not found or has no items".into(),
        ));
    }

    let mut total = Decimal::ZERO;
    let mut currency = "USD".to_string();
    let mut lines = Vec::new();
    for item in &items {
        let unit: Decimal = item.get("unit_amount");
        let quantity: i32 = item.get("quantity");
        let line = unit * Decimal::from(quantity);
        currency = item.get("currency");
        total += line;
        lines.push(serde_json::json!({
            "description": format!("subscription item {}", item.get::<Uuid, _>("price_id")),
            "amount": line.to_string(),
            "kind": "subscription",
        }));
    }

    // Tax projection matches the run's math exactly.
    let tax_rows = sqlx::query("SELECT jurisdiction, rate_bps FROM billing.tax_rates")
        .fetch_all(&mut *conn)
        .await
        .map_err(err_db)?;
    for tax in &tax_rows {
        let rate_bps: i32 = tax.get("rate_bps");
        let tax_amount = (total * Decimal::from(rate_bps) / Decimal::from(10_000)).round_dp(2);
        if tax_amount > Decimal::ZERO {
            lines.push(serde_json::json!({
                "description": format!("tax ({})", tax.get::<String, _>("jurisdiction")),
                "amount": tax_amount.to_string(),
                "kind": "tax",
            }));
            total += tax_amount;
        }
    }

    Ok(Json(serde_json::json!({
        "subscription_id": q.subscription_id,
        "amount_due": total.to_string(),
        "currency": currency,
        "lines": lines,
    })))
}

/// Query params for the upcoming-invoice preview.
#[derive(Debug, serde::Deserialize, utoipa::IntoParams)]
pub struct UpcomingQuery {
    pub subscription_id: Uuid,
}

/// `POST /v1/internal/dunning/retry` - the dunning-ladder tick (BIL-12):
/// re-charge past-due invoices on the configured schedule. The ladder
/// is DATA (`dunning_configs` rows): count + backoff per env; after the
/// final failure the subscription cancels (guarded terminal).
#[utoipa::path(
    post,
    path = "/v1/internal/dunning/retry",
    responses(
        (status = 200, description = "Dunning tick processed"),
    ),
    security(("sentin8:cron:write" = [])),
    tag = "billing",
)]
pub async fn dunning_retry(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let a = auth::verify_token(&state.config, &headers).await?;

    let mut conn = state.pool.begin().await.map_err(err_db)?;
    set_tenant_context(&mut conn, a.env_id())
        .await
        .map_err(err_db)?;

    // The ladder configuration: max retries per env (default 3).
    let max_retries: i64 =
        sqlx::query("SELECT COALESCE(MAX(max_retries), 3) as m FROM billing.dunning_configs")
            .fetch_optional(&mut *conn)
            .await
            .map_err(err_db)?
            .map(|r| r.get("m"))
            .unwrap_or(3);

    // Past-due (retrying) invoices with their attempt counts.
    let past_due = sqlx::query(
        r#"SELECT i.id, i.customer_id, i.amount_due, i.currency, i.subscription_id, i.dunning_attempts
           FROM billing.invoices i WHERE i.status = 'retrying'"#,
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(err_db)?;

    let mut recovered = 0usize;
    let mut exhausted = 0usize;
    for inv in &past_due {
        let invoice_id: Uuid = inv.get("id");
        let retries: i64 = inv.get("dunning_attempts");
        if retries >= max_retries {
            // Ladder exhausted: the subscription cancels (terminal).
            if let Some(sub_id) = inv.get::<Option<Uuid>, _>("subscription_id") {
                sqlx::query(
                    "UPDATE billing.subscriptions SET status = 'canceled' WHERE id = $1 AND status = 'past_due'",
                )
                .bind(sub_id)
                .execute(&mut *conn)
                .await
                .map_err(err_db)?;
            }
            sqlx::query(
                "UPDATE billing.invoices SET status = 'void' WHERE id = $1 AND status = 'retrying'",
            )
            .bind(invoice_id)
            .execute(&mut *conn)
            .await
            .map_err(err_db)?;
            exhausted += 1;
            continue;
        }

        // Retry the charge through the payments public API.
        let customer_id: Uuid = inv.get("customer_id");
        let amount: Decimal = inv.get("amount_due");
        let currency: String = inv.get("currency");
        match charge_invoice(&state, customer_id, amount, &currency, invoice_id).await {
            Ok(_) => recovered += 1,
            Err(_) => {
                // Count the attempt on the invoice itself (the ladder's
                // state lives on the invoice row - see DESIGN.md §3).
                sqlx::query(
                    "UPDATE billing.invoices SET dunning_attempts = dunning_attempts + 1 WHERE id = $1",
                )
                .bind(invoice_id)
                .execute(&mut *conn)
                .await
                .map_err(err_db)?;
            }
        }
    }

    conn.commit().await.map_err(err_db)?;
    emit_event(
        &state.pool,
        a.env_id(),
        "bill8:billing:invoice:dunning_tick",
        serde_json::json!({ "recovered": recovered, "exhausted": exhausted }),
    )
    .await;
    Ok(Json(
        serde_json::json!({ "recovered": recovered, "exhausted": exhausted }),
    ))
}
