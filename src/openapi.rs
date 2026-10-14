// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Bill8:Billing
// SPDX-License-Identifier: Apache-2.0

//! OpenAPI spec for the Bill8:Billing API.

use utoipa::OpenApi;

use crate::handlers;

/// The billing service's OpenAPI document.
#[derive(OpenApi)]
#[openapi(paths(
    handlers::create_product,
    handlers::create_price,
    handlers::list_prices,
    handlers::create_subscription,
    handlers::list_subscriptions,
    handlers::get_subscription,
    handlers::create_usage_record,
    handlers::list_invoices,
    handlers::invoice_runs::run_invoice_cycle,
    handlers::invoice_runs::dunning_retry,
    handlers::invoice_runs::upcoming_invoice,
    handlers::billing_richness::create_coupon,
    handlers::billing_richness::redeem_coupon,
    handlers::billing_richness::create_credit,
    handlers::billing_richness::create_credit_note,
    handlers::billing_richness::void_credit_note,
    handlers::billing_richness::create_tax_rate,
    handlers::billing_richness::pause_subscription,
    handlers::billing_richness::resume_subscription,
    handlers::billing_richness::proration_quote,
))]
pub struct BillingOpenApi;
