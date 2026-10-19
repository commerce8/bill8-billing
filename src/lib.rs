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
pub mod policy;
pub mod proration;
pub mod registration;
pub mod server;
