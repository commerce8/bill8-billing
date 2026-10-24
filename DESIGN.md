# Bill8:Billing - Detailed Design Document

## 1. Scope & Context

Bill8:Billing owns: the billing domain schema (domain tables + the
leaf schema `billing` for outbox/audit journal), the product / price /
subscription / invoice / usage-record / coupon / credit REST API, the
proration and metering engines, the invoice-run proxy-back handler for
Sentin8:Cron, and the `bill8:billing:*` event family.

It does NOT own: money movement (Bill8:Ledger), charge execution
(Bill8:Payments), workflow orchestration (Sentin8:Flow), or
scheduling (Sentin8:Cron - billing supplies the due-invoice execution).

Billing owns the **subscription lifecycle richness**: trials, billing
anchors, pause/resume, dunning - and the **invoice richness**:
pricing models (flat/tiered/volume/packaged usage), read-only previews
(upcoming invoice, proration quotes), credit notes (invoice-level
adjustments - charge-level refunds belong to payments), explicit tax
lines, and invoice PDF/email delivery (documents via Sentin8:Storage,
email via Sentin8:Post).

```mermaid
graph TD
    BFF["Sentin8:BFF /proxy/billing/*"] --> BI["Bill8:Billing"]
    BI -->|"charges"| PAY["Bill8:Payments"]
    BI -->|"balance transactions"| L["Bill8:Ledger"]
    BI <-->|"workflows"| FLOW["Sentin8:Flow"]
    CRON["Sentin8:Cron"] -->|"invoice runs"| BI
    BI -->|"billing.outbox → relay"| BUS["Sentin8:Bus"]
    BUS -->|"charge:succeeded"| BI
    BI <-->|"schema billing"| PG[("bill8 database")]
```

## 2. Runtime Architecture

Shell-booted; `build_state` adds config, Bus client, AppState, and a
consumer for `bill8:payments:charge:succeeded` (mark invoice paid).

**Subscription creation with proration**:

```mermaid
sequenceDiagram
    participant C as Client (BFF)
    participant BI as Bill8:Billing
    participant DB as PostgreSQL
    C->>BI: POST /v1/subscriptions (price, quantity)
    BI->>BI: Shield gate
    BI->>DB: BEGIN + set_tenant_context
    BI->>DB: resolve price + plan + coupon/credit
    BI->>DB: proration (if mid-cycle change)
    BI->>DB: subscription + items + draft invoice
    BI->>DB: COMMIT + outbox subscription:created
    BI-->>C: 200 subscription (active)
```

**Invoice run (Cron proxy-back)**:

```mermaid
sequenceDiagram
    participant CR as Sentin8:Cron
    participant BI as Bill8:Billing
    participant DB as PostgreSQL
    participant P as Bill8:Payments
    CR->>BI: POST /v1/internal/invoices/run (M2M)
    BI->>DB: find subscriptions due this cycle
    loop each subscription
        BI->>DB: aggregate usage records (deterministic sum)
        BI->>DB: finalize invoice (draft → open)
        BI->>P: POST /v1/charges (invoice amount)
        P-->>BI: charge id
    end
    BI->>DB: outbox invoice:paid / invoice:open
```

## 3. Data Model

```mermaid
erDiagram
    products ||--o{ prices : "priced by"
    customers ||--o{ subscriptions : "holds"
    prices ||--o{ subscriptions : "subscribes to"
    subscriptions ||--o{ subscription_items : "composed of"
    subscriptions ||--o{ invoices : "billed by"
    invoices ||--o{ invoice_items : "itemized by"
    subscriptions ||--o{ usage_records : "metered by"
    customers ||--o{ coupons : "redeems"
    customers ||--o{ credits : "holds"
```

| Table | Purpose |
|---|---|
| `products` | Catalog of sellable things |
| `prices` | Recurring or one-off price points (pricing model on the price) |
| `subscriptions` | A customer's plan instance (trial_end, billing_cycle_anchor, paused state) |
| `subscription_items` | Quantified lines on a subscription |
| `invoices` | Generated bills |
| `invoice_items` | Line items on an invoice |
| `usage_records` | Metered consumption events |
| `coupons` | Discounts (percentage / amount) |
| `credits` | Prepaid value balances |
| `credit_notes` | Invoice-level adjustments (issued → void) |
| `tax_rates` | Jurisdiction tax rates (inclusive/exclusive) |
| `dunning_configs` | Per-env retry ladders for past-due invoices |

**Producer tables** (platform contracts, `billing` leaf schema):
`billing.outbox` (DBX producer path - architecture-principles §6.7) and
`billing.audit_journal` (the 15-column contract of
`sentin8_audit::audit_journal_sql`, AUDJOUR-1 - env-pinned RLS, drained
to Sys `POST /v1/audit` by the shell's boot-8c pusher).

**State machines**:

```mermaid
stateDiagram-v2
    [*] --> trialing: create with trial
    [*] --> incomplete: create without trial
    trialing --> active: trial ends (first invoice paid)
    incomplete --> active: first payment
    active --> past_due: payment failed
    past_due --> active: payment recovered
    active --> paused: pause
    paused --> active: resume (prorated)
    active --> canceled: cancel
    past_due --> canceled: dunning exhausted
    canceled --> [*]
```

```mermaid
stateDiagram-v2
    [*] --> draft: create
    draft --> open: finalize (freeze lines + tax computed)
    open --> paid: charge succeeded
    open --> void: void
    open --> retrying: dunning ladder step
    retrying --> paid: retry charge succeeded
    paid --> [*]
```

## 4. API Surface

| Endpoint group | Prefix | Authorization | Events |
|---|---|---|---|
| Products | `/v1/products` | `bill8:billing:products:{read,write}` | `product:created/updated` |
| Prices | `/v1/prices` | `bill8:billing:prices:{read,write}` | `price:created/updated` |
| Subscriptions | `/v1/subscriptions` | `bill8:billing:subscriptions:{read,write}` | `subscription:created/updated/canceled` |
| Subscription items | `/v1/subscription-items` | `bill8:billing:subscription-items:{read,write}` | `subscription_item:created/updated` |
| Invoices | `/v1/invoices` | `bill8:billing:invoices:{read,write}` | `invoice:created/paid/voided` |
| Usage records | `/v1/usage-records` | `bill8:billing:usage-records:write` | `usage_record:created` |
| Coupons | `/v1/coupons` | `bill8:billing:coupons:{read,write}` | `coupon:created/updated` |
| Credits | `/v1/credits` | `bill8:billing:credits:{read,write}` | `credit:created/updated` |
| Credit notes | `/v1/credit-notes`, `/v1/credit-notes/{id}/void` | `bill8:billing:credit-notes:{read,write}` | `credit_note:issued/voided` |
| Entitlements | `/v1/entitlements` | `bill8:billing:entitlements:read` | - |
| Previews | `/v1/invoices/upcoming`, `/v1/subscriptions/preview` | `bill8:billing:invoices:read` | - (read-only, zero writes) |
| Tax rates | `/v1/tax-rates` | `bill8:billing:tax-rates:{read,write}` | `tax_rate:created/updated` |
| Invoice document | `/v1/invoices/{id}/pdf` (signed link) | `bill8:billing:invoices:read` | `invoice:emailed` |
| Internal | `/v1/internal/*` | M2M | - |

**Dunning ladder** - a per-env retry schedule (`dunning_configs`):
each step re-charges the past-due invoice and notifies via
Sentin8:Post; the ladder is data (count, backoff), never ad hoc;
exhaustion cancels the subscription (guarded terminal transition).

## 5. Cross-Cutting Concerns

- Shield-gated; `env_id` RLS + holder scoping on customer-held tables.
- Cert-bound DCR (`billing`, `billing-policy`, `billing-audit`).
- Idempotency on invoice runs and usage-record aggregation.
- Consumes `charge:succeeded` to mark invoices paid (event-driven).

## 6. Failure Modes & Recovery

```mermaid
flowchart TD
    CHARGE_FAIL["invoice charge fails"] -->|"subscription → past_due"| RETRY["next invoice run retries"]
    PRORATE["proration drift"] -->|"deterministic half-up"| CENT["reconciles to the cent"]
    RELAY_FAIL["Bus outage"] -->|"producer"| OUTBOX["billing.outbox persists - retry ≤10"]
    CRASH["crash mid-tx"] -->|"atomic"| RB["subscription + invoice + outbox roll back"]
```

## 7. Concurrency & Performance Model

- Pool cap 3; usage aggregation is a single deterministic SUM per
  invoice, not per event.
- Invoice-run is cron-triggered and processes subscriptions in bounded
  batches.

## 8. Invariants, Insecure-Defaults Rejected, Unsafe Rationale

- Invoices immutable once finalized; only status transitions guarded.
- Proration nets to the cent (half-up).
- Subscription items sum to the billed amount.
- `#![forbid(unsafe_code)]`.

## 9. Testing Strategy

```mermaid
graph TD
    U["Unit - proration math, metering aggregation,<br/>coupon/credit application"] --> I["Integration - DB-backed subscription/invoice suites"]
    I --> E["E2E - e2e-ts UI + BFF proxy only"]
```

## 10. Open Questions

- See `backlog.md` `BIL-*`.

---

**Architecture & Design:** bill8:billing and its underlying concepts were designed by [Hamid Jahanzad](https://www.linkedin.com/in/hamidjahanzad/).
