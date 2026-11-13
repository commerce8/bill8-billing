// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Bill8:Billing
// SPDX-License-Identifier: Apache-2.0

//! # Bill8:Billing - recurring billing
//!
//! Products, prices, subscriptions, invoices, usage metering, coupons,
//! credits, and credit notes. Proration nets to the cent (half-up);
//! invoices are immutable once finalized; usage aggregation is
//! deterministic. See `DESIGN.md`.

#![forbid(unsafe_code)]

pub mod auth;
pub mod config;
pub mod consumers;
pub mod handlers;
pub mod openapi;

// The utoipa dump for registration::print - mirrored from main.rs; the
// lib crate compiles registration::print too (test harnesses embedding
// via #[path] never invoke it, so the pointer is referenced, not run).
static OPENAPI_DUMP: fn() -> serde_json::Value = || {
    use utoipa::OpenApi as _;
    serde_json::to_value(openapi::BillingOpenApi::openapi()).unwrap_or(serde_json::Value::Null)
};

pub mod policy;
pub mod proration;
pub mod registration;
pub mod server;
