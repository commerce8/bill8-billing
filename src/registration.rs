// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Bill8:Billing
// SPDX-License-Identifier: Apache-2.0

//! Service self-registration data for Bill8:Billing.

use sentin8::registration::{
    AdminScope, AdminUi, AdminUiPage, ErrorCode, M2mGrant, PgRole, PolicySetDecl,
    Registration, SqlMigration,
};
use utoipa::OpenApi;

const PG_ROLES: &[PgRole] = &[
    PgRole {
        scope: "bill8:billing:read,bill8:billing:write",
        role: "bill8_billing_service",
        bypass_rls: false,
        database: "bill8",
        cross_schema_grants: &[],
    },
];

const M2M_GRANTS: &[M2mGrant] = &[
    M2mGrant {
        workload: "bill8:billing",
        scopes: &[
            "sentin8:bus:produce",
            "sentin8:bus:consume",
            "bill8:billing:admin",
            "sentin8:stat:measure",
        ],
    },
    M2mGrant {
        workload: "billing",
        scopes: &[
            "sentin8:bus:produce",
            "sentin8:bus:consume",
            "bill8:billing:admin",
            "sentin8:stat:measure",
        ],
    },
    M2mGrant {
        workload: "billing-audit",
        scopes: &["sentin8:sys:audit:write"],
    },
    M2mGrant {
        workload: "bill8:billing-policy",
        scopes: &["sentin8:bus:consume", "sentin8:stat:measure"],
    },
    M2mGrant {
        workload: "billing-policy",
        scopes: &["sentin8:stat:measure"],
    },
    M2mGrant {
        workload: "bill8-billing-tls",
        scopes: &["sentin8:sys:instances:read"],
    },
    // Cross-service: invoice charges through the payments PUBLIC API.
    M2mGrant {
        workload: "bill8-billing-payments",
        scopes: &["bill8:payments:charges:write"],
    },
];

const ADMIN_SCOPES: &[AdminScope] = &[
    AdminScope::Typed { role: "bill8:billing:admin", scope: "bill8:billing:products:read" },
    AdminScope::Typed { role: "bill8:billing:admin", scope: "bill8:billing:products:write" },
    AdminScope::Typed { role: "bill8:billing:admin", scope: "bill8:billing:prices:read" },
    AdminScope::Typed { role: "bill8:billing:admin", scope: "bill8:billing:prices:write" },
    AdminScope::Typed { role: "bill8:billing:admin", scope: "bill8:billing:subscriptions:read" },
    AdminScope::Typed { role: "bill8:billing:admin", scope: "bill8:billing:subscriptions:write" },
    AdminScope::Typed { role: "bill8:billing:admin", scope: "bill8:billing:invoices:read" },
    AdminScope::Typed { role: "bill8:billing:admin", scope: "bill8:billing:invoices:write" },
    AdminScope::Typed { role: "bill8:billing:admin", scope: "bill8:billing:usage-records:write" },
    AdminScope::Typed { role: "bill8:billing:admin", scope: "bill8:billing:policy:read" },
    AdminScope::Typed { role: "bill8:billing:admin", scope: "bill8:billing:policy:write" },
];

const ERROR_CODES: &[ErrorCode] = &[
    ErrorCode { enum_name: "BillingSubscriptionNotFound", error_code: "billing_subscription_not_found", status_code: 404 },
    ErrorCode { enum_name: "BillingPriceNotFound", error_code: "billing_price_not_found", status_code: 404 },
    ErrorCode { enum_name: "BillingInvoiceImmutable", error_code: "billing_invoice_immutable", status_code: 422 },
];

const ADMIN_UI_PAGES: &[AdminUiPage] = &[
    AdminUiPage { route: "overview", label: "Overview", ui_endpoint: "/v1/admin/ui/overview", data_endpoint: "/v1/admin/overview", icon: "0xe1b1" },
    AdminUiPage { route: "products", label: "Products", ui_endpoint: "/v1/admin/ui/products", data_endpoint: "/v1/admin/products", icon: "0xe349" },
    AdminUiPage { route: "prices", label: "Prices", ui_endpoint: "/v1/admin/ui/prices", data_endpoint: "/v1/admin/prices", icon: "0xe263" },
    AdminUiPage { route: "subscriptions", label: "Subscriptions", ui_endpoint: "/v1/admin/ui/subscriptions", data_endpoint: "/v1/admin/subscriptions", icon: "0xe8d2" },
    AdminUiPage { route: "invoices", label: "Invoices", ui_endpoint: "/v1/admin/ui/invoices", data_endpoint: "/v1/admin/invoices", icon: "0xe8be" },
];

/// The scope-mirror Shield text.
pub const SCOPE_MIRROR_POLICIES: &str = "permit(principal, action == Action::\"bill8:billing:products:read\", resource) when { principal.scopes.contains(\"bill8:billing:products:read\") };\npermit(principal, action == Action::\"bill8:billing:products:read\", resource) when { principal.scopes.contains(\"sentin8:sys:admin\") };\npermit(principal, action == Action::\"bill8:billing:products:write\", resource) when { principal.scopes.contains(\"bill8:billing:products:write\") };\npermit(principal, action == Action::\"bill8:billing:products:write\", resource) when { principal.scopes.contains(\"sentin8:sys:admin\") };\npermit(principal, action == Action::\"bill8:billing:prices:read\", resource) when { principal.scopes.contains(\"bill8:billing:prices:read\") };\npermit(principal, action == Action::\"bill8:billing:prices:write\", resource) when { principal.scopes.contains(\"bill8:billing:prices:write\") };\npermit(principal, action == Action::\"bill8:billing:subscriptions:read\", resource) when { principal.scopes.contains(\"bill8:billing:subscriptions:read\") };\npermit(principal, action == Action::\"bill8:billing:subscriptions:read\", resource) when { principal.scopes.contains(\"sentin8:sys:admin\") };\npermit(principal, action == Action::\"bill8:billing:subscriptions:write\", resource) when { principal.scopes.contains(\"bill8:billing:subscriptions:write\") };\npermit(principal, action == Action::\"bill8:billing:subscriptions:write\", resource) when { principal.scopes.contains(\"sentin8:sys:admin\") };\npermit(principal, action == Action::\"bill8:billing:invoices:read\", resource) when { principal.scopes.contains(\"bill8:billing:invoices:read\") };\npermit(principal, action == Action::\"bill8:billing:invoices:write\", resource) when { principal.scopes.contains(\"bill8:billing:invoices:write\") };\npermit(principal, action == Action::\"bill8:billing:usage-records:write\", resource) when { principal.scopes.contains(\"bill8:billing:usage-records:write\") };\n";

const POLICIES: &[PolicySetDecl] = &[PolicySetDecl {
    policy_set_name: "scope-mirror",
    description: "Action-granular scope permits for bill8:billing over the LIVE effective scope set. Step 5 is the only gate.",
    policies: SCOPE_MIRROR_POLICIES,
    schema: None,
    mode: "enforced",
    default_enabled: true,
}];

/// The invoice consumer is declared in `vertical-registry.json` (the
/// billing↔payments event-driven coupling; no internal API).
const EVENT_TRIGGERS: &[sentin8::registration::EventTriggerDecl] = &[];

const SQL_1_INIT: &str = include_str!("../sql/init.sql");

fn openapi_spec() -> serde_json::Value {
    serde_json::to_value(crate::openapi::BillingOpenApi::openapi()).unwrap_or(serde_json::Value::Null)
}

/// Print the registration JSON to stdout.
pub fn print(section: Option<String>) -> anyhow::Result<()> {
    let reg = Registration {
        service_type: "bill8:billing",
        subdomain: "bill8-billing",
        health_path: "/v1/health",
        admin_scopes: ADMIN_SCOPES,
        error_codes: ERROR_CODES,
        consent_providers: &[],
        consent_flows: &[],
        policies: POLICIES,
        relations: &[],
        event_triggers: EVENT_TRIGGERS,
        schedules: &[],
        admin_ui: Some(AdminUi {
            label: "Billing",
            icon: "0xe8be",
            service_flag: "billing",
            pages: ADMIN_UI_PAGES,
        }),
        sql_migrations: vec![SqlMigration { version: 1, name: "init", sql: SQL_1_INIT }],
        pg_roles: PG_ROLES,
        audit_findings: None,
        service_actions: None,
        m2m_grants: Some(M2M_GRANTS),
        openapi_url: "/openapi.json",
        openapi: openapi_spec,
    };
    sentin8::registration::print_registration(section, &reg, openapi_spec)
}