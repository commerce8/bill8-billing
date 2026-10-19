// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Bill8:Billing
// SPDX-License-Identifier: Apache-2.0

//! Auth token verification for Bill8:Billing (steps 1-4).

use axum::http::{HeaderMap, StatusCode};

use crate::config::BillingConfig;

pub use sentin8::auth::client::AuthContext;

/// Verify the Bearer token and return the auth context.
///
/// # Errors
/// Returns `401` with a reason when the bearer token fails verification.
pub async fn verify_token(
    config: &BillingConfig,
    headers: &HeaderMap,
) -> Result<AuthContext, (StatusCode, String)> {
    sentin8::auth::client::verify_bearer_token(&config.sentin8_auth_client, headers).await
}
