# Bill8:Billing - Business Requirements & Functionality

> Audience: product owners, business analysts, and anyone evaluating what
> Bill8:Billing **does for the business**. For architecture read
> `README.md`; for wire-level detail read `DESIGN.md`.

## 1. Business Purpose

Bill8:Billing is the **recurring revenue engine**. It delivers:

- **Subscriptions** - a customer subscribes to a price and is billed per
  cycle.
- **Metered usage** - usage records aggregate deterministically onto
  invoices.
- **Correct proration** - a plan change prorates to the cent.
- **Discounts and credits** - coupons and prepaid credits adjust the
  amount owed.

## 2. Business Scope (What the Service Provides)

| # | Capability | What the business gets |
|---|---|---|
| 1 | Products | Catalog of sellable things |
| 2 | Prices | Recurring or one-off price points |
| 3 | Subscriptions | Customer plan instances |
| 4 | Subscription items | Quantified lines |
| 5 | Invoices | Generated bills |
| 6 | Invoice items | Line items on invoices |
| 7 | Usage records | Metered consumption |
| 8 | Coupons | Discounts |
| 9 | Credits | Prepaid value |
| 10 | Credit notes | Invoice-level corrections |
| 11 | Trials & anchors | Free periods and cycle alignment (calendar-month or custom anchor) |
| 12 | Pause & resume | Voluntary billing pause with correct proration on resume |
| 13 | Dunning | Scheduled retries + notifications for past-due invoices, with deterministic cancellation or recovery |
| 14 | Pricing models | Flat, tiered, volume, and packaged usage pricing |
| 15 | Previews | Upcoming-invoice and plan-change quotes with zero commitment |
| 16 | Tax | Explicit tax lines per jurisdiction, inclusive or exclusive |
| 17 | Invoice documents | PDF invoices and email delivery |

**What the service does NOT own (business boundary):**

| Out of scope | Owned by |
|---|---|
| Money movement | Bill8:Ledger |
| Charge execution | Bill8:Payments |
| Workflow orchestration | Sentin8:Flow |
| Scheduling | Sentin8:Cron |

## 3. Business Capabilities

| Capability | Business rule |
|---|---|
| Subscribe | A customer subscribes to a price |
| Offer a trial | A subscription can start with a free period that converts exactly once |
| Change plan | A plan change prorates against the remaining cycle |
| Align billing | Cycles align to the calendar month or a custom anchor date |
| Pause billing | A paused subscription generates no invoices and resumes with correct proration |
| Meter usage | Usage records aggregate onto the next invoice per the price's model |
| Recover failed payments | A past-due invoice retries on a configurable schedule with notifications, then cancels or recovers deterministically |
| Quote before committing | Upcoming-invoice and plan-change previews match the real outcome exactly, with no writes |
| Correct an invoice | A credit note adjusts a paid invoice within its paid amount |
| Charge tax | Invoices carry explicit, cent-exact tax lines per jurisdiction |
| Send invoices | A finalized invoice renders to PDF and delivers by email with retry |
| Apply discount | A coupon/credit reduces the invoice amount |

## 4. Business Events (Outbound)

| Event | Fires when |
|---|---|
| `product:created/updated` | Product changes |
| `price:created/updated` | Price changes |
| `subscription:created/updated/canceled/paused/resumed` | Subscription lifecycle changes |
| `subscription:trial_ended` | A trial converts |
| `invoice:created/paid/voided/retrying` | Invoice lifecycle changes |
| `invoice:emailed` | An invoice is emailed |
| `usage_record:created` | A usage event is metered |
| `coupon:created/updated` / `credit:created/updated` / `credit_note:issued/voided` | Discount/adjustment changes |
| `tax_rate:created/updated` | Tax configuration changes |

## 5. Business Rules & Invariants

1. Invoices are immutable once finalized.
2. Proration nets to the cent (half-up).
3. Subscription items sum to the billed amount.

## 6. Target-User Roles & Use Cases

| Persona | Does |
|---|---|
| Merchant | Define products/prices, manage subscriptions |
| Customer | View invoices, manage subscription |

## 7. KPIs / Success Criteria

| KPI | Measure |
|---|---|
| Invoice accuracy | proration + metering reconcile to the cent |
| Billing success | invoices paid / opened |

## 8. Dependencies & Reliance on Other Services

| Reliance | On |
|---|---|
| Charge execution | Bill8:Payments |
| Money movement | Bill8:Ledger |
| Orchestration | Sentin8:Flow |
| Scheduling | Sentin8:Cron |

## 9. Acceptance Criteria

1. A subscription generates invoices with correct proration and metered
   usage.
2. A coupon/credit correctly reduces the invoice amount.
3. A failed invoice charge drives the subscription to past-due and
   recovers on the next run.

## 10. Open Business Questions

- See `backlog.md` `BIL-*`.

---

**Design attribution:** bill8:billing and its underlying concepts were designed by Hamid Jahanzad <Hamid@Jahanzad.com>.
