// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Bill8:Billing
// SPDX-License-Identifier: Apache-2.0

//! Axum server setup for Bill8:Billing - state binding + routes.

use axum::routing::get;
use std::sync::Arc;

use crate::handlers;

/// Build a cert-presenting HTTP transport for `workload` (CERTFREE-4V,
/// auth DESIGN §4.3f): mint-or-load the Acme-issued pair and wrap it in
/// a reqwest client. The receiving service's TLS layer authenticates the
/// handshake and the enroll-published registry map authorizes the
/// scopes - no bearer token exists on this path.
///
/// # Errors
/// Returns the certificate-ensure or transport-build failure (the
/// caller logs and degrades).
async fn build_workload_http(auth_issuer: &str, workload: &str) -> anyhow::Result<reqwest::Client> {
    let cert = sentin8::auth::m2m::ensure_workload_certificate(
        auth_issuer,
        workload,
        &sentin8::auth::m2m::platform_ca_path(),
    )
    .await?;
    let mtls = sentin8::middleware::mtls::MtlsConfig::new(
        &cert.ca_cert_path,
        &cert.cert_path,
        &cert.key_path,
    );
    sentin8::middleware::mtls::build_mtls_client(&mtls)
        .map_err(|e| anyhow::anyhow!("workload '{workload}' transport: {e}"))
}

/// Bind the service state.
pub async fn build_state(core: sentin8::shell::ServiceCore) -> anyhow::Result<handlers::AppState> {
    let mut config = crate::config::BillingConfig::from_env();

    // Cert-presenting Bus client (CERTFREE-4V): the `billing` workload
    // CERTIFICATE is the identity - no DCR, no token (auth DESIGN
    // §4.3f); the mint retries while the fleet boots.
    let bus_client = loop {
        match sentin8::bus::client::BusClient::for_workload(
            &config.sentin8_bus_url,
            &config.sentin8_auth_issuer,
            "billing",
        )
        .await
        {
            Ok(c) => break c,
            Err(e) => {
                tracing::debug!(error = %e, "billing bus client: certificate not ready yet");
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            }
        }
    };

    // The service's own workload transport (CERTFREE-4V): the
    // `billing` certificate presents in the TLS handshake - the
    // receiving service's TLS layer authenticates it and the registry
    // map authorizes the scopes.
    match build_workload_http(&config.sentin8_auth_issuer, "billing").await {
        Ok(http) => config.m2m_http = Some(http),
        Err(e) => tracing::warn!(error = %e, "billing workload transport unavailable - degraded"),
    }

    // Cross-service payments workload transport (invoice charges).
    match build_workload_http(&config.sentin8_auth_issuer, "bill8-billing-payments").await {
        Ok(http) => config.payments_http = Some(http),
        Err(e) => {
            tracing::warn!(error = %e, "billing-payments transport unavailable - invoice charges degraded")
        }
    }

    let policy_engine = core.admin.policy.clone();

    let state = handlers::AppState {
        config: Arc::new(config),
        pool: core.pool.clone(),
        bus_client,
        policy: policy_engine,
    };

    // Spawn the invoice-paid consumer (payments family) - the
    // billing↔payments event-driven coupling (consumers.rs).
    crate::consumers::spawn_consumers(state.clone());

    Ok(state)
}

/// Mount the billing routes onto the shell router (state-typed).
pub fn routes(router: axum::Router<handlers::AppState>) -> axum::Router<handlers::AppState> {
    router
        .route("/v1/health", get(handlers::health))
        .route(
            "/v1/products",
            axum::routing::post(handlers::create_product),
        )
        .route(
            "/v1/prices",
            get(handlers::list_prices).post(handlers::create_price),
        )
        .route(
            "/v1/subscriptions",
            get(handlers::list_subscriptions).post(handlers::create_subscription),
        )
        .route("/v1/subscriptions/{id}", get(handlers::get_subscription))
        .route(
            "/v1/usage-records",
            axum::routing::post(handlers::create_usage_record),
        )
        .route("/v1/invoices", get(handlers::list_invoices))
        // Invoice runs + previews (BIL-6/12/14) + the Cron proxy-back ticks.
        .route(
            "/v1/internal/invoices/run",
            axum::routing::post(handlers::invoice_runs::run_invoice_cycle),
        )
        .route(
            "/v1/internal/dunning/retry",
            axum::routing::post(handlers::invoice_runs::dunning_retry),
        )
        .route(
            "/v1/invoices/upcoming",
            get(handlers::invoice_runs::upcoming_invoice),
        )
        // Billing richness (BIL-7/8/11/15/16).
        .route(
            "/v1/coupons",
            axum::routing::post(handlers::billing_richness::create_coupon),
        )
        .route(
            "/v1/coupons/{code}/redeem",
            axum::routing::post(handlers::billing_richness::redeem_coupon),
        )
        .route(
            "/v1/credits",
            axum::routing::post(handlers::billing_richness::create_credit),
        )
        .route(
            "/v1/credit-notes",
            axum::routing::post(handlers::billing_richness::create_credit_note),
        )
        .route(
            "/v1/credit-notes/{id}/void",
            axum::routing::post(handlers::billing_richness::void_credit_note),
        )
        .route(
            "/v1/tax-rates",
            axum::routing::post(handlers::billing_richness::create_tax_rate),
        )
        .route(
            "/v1/subscriptions/{id}/pause",
            axum::routing::post(handlers::billing_richness::pause_subscription),
        )
        .route(
            "/v1/subscriptions/{id}/resume",
            axum::routing::post(handlers::billing_richness::resume_subscription),
        )
        .route(
            "/v1/subscriptions/proration-quote",
            get(handlers::billing_richness::proration_quote),
        )
}
