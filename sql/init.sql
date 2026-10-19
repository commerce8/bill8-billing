-- SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
-- SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
-- SPDX-FileCopyrightText: Copyright Contributors to the Bill8:Billing
-- SPDX-License-Identifier: Apache-2.0

-- Bill8:Billing - the service's own schema `billing`: products,
-- prices, subscriptions, invoices, usage, coupons, credits, credit notes.

CREATE SCHEMA IF NOT EXISTS billing;

CREATE TABLE IF NOT EXISTS billing.products (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id      UUID NOT NULL,
    name        TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'archived')),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    resource_version INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX IF NOT EXISTS idx_billing_products_env ON billing.products (env_id);

CREATE TABLE IF NOT EXISTS billing.prices (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id          UUID NOT NULL,
    product_id      UUID NOT NULL REFERENCES billing.products(id),
    interval        TEXT CHECK (interval IS NULL OR interval IN ('day', 'week', 'month', 'year')),
    pricing_model   TEXT NOT NULL DEFAULT 'flat' CHECK (pricing_model IN ('flat', 'tiered', 'volume', 'packaged')),
    unit_amount     DECIMAL(20,2) NOT NULL CHECK (unit_amount >= 0),
    currency        TEXT NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_billing_prices_env ON billing.prices (env_id);

CREATE TABLE IF NOT EXISTS billing.subscriptions (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id              UUID NOT NULL,
    customer_id         UUID NOT NULL,
    status              TEXT NOT NULL DEFAULT 'incomplete'
                        CHECK (status IN ('incomplete', 'trialing', 'active', 'past_due', 'paused', 'canceled')),
    trial_end           TIMESTAMPTZ,
    billing_cycle_anchor TIMESTAMPTZ,
    current_period_end  TIMESTAMPTZ NOT NULL,
    idempotency_key     TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    resource_version    INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX IF NOT EXISTS idx_billing_subscriptions_env ON billing.subscriptions (env_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_billing_subscriptions_idempotency
    ON billing.subscriptions (env_id, idempotency_key) WHERE idempotency_key IS NOT NULL;

CREATE TABLE IF NOT EXISTS billing.subscription_items (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id          UUID NOT NULL,
    subscription_id UUID NOT NULL REFERENCES billing.subscriptions(id),
    price_id        UUID NOT NULL REFERENCES billing.prices(id),
    quantity        INTEGER NOT NULL DEFAULT 1 CHECK (quantity > 0),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_billing_subscription_items_env ON billing.subscription_items (env_id);
CREATE INDEX IF NOT EXISTS idx_billing_subscription_items_sub ON billing.subscription_items (subscription_id);

CREATE TABLE IF NOT EXISTS billing.invoices (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id          UUID NOT NULL,
    customer_id     UUID NOT NULL,
    subscription_id UUID,
    amount_due      DECIMAL(20,2) NOT NULL DEFAULT 0,
    amount_paid     DECIMAL(20,2) NOT NULL DEFAULT 0,
    currency        TEXT NOT NULL,
    status          TEXT NOT NULL DEFAULT 'draft'
                    CHECK (status IN ('draft', 'open', 'paid', 'void', 'retrying')),
    period_start    TIMESTAMPTZ NOT NULL,
    period_end      TIMESTAMPTZ NOT NULL,
    dunning_attempts INTEGER NOT NULL DEFAULT 0,
    idempotency_key TEXT,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    resource_version INTEGER NOT NULL DEFAULT 1
);
CREATE INDEX IF NOT EXISTS idx_billing_invoices_env ON billing.invoices (env_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_billing_invoices_idempotency
    ON billing.invoices (env_id, idempotency_key) WHERE idempotency_key IS NOT NULL;

CREATE TABLE IF NOT EXISTS billing.invoice_items (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id      UUID NOT NULL,
    invoice_id  UUID NOT NULL REFERENCES billing.invoices(id),
    description TEXT NOT NULL,
    amount      DECIMAL(20,2) NOT NULL,
    kind        TEXT NOT NULL
                CHECK (kind IN ('subscription', 'usage', 'proration', 'coupon', 'credit', 'tax')),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_billing_invoice_items_env ON billing.invoice_items (env_id);
CREATE INDEX IF NOT EXISTS idx_billing_invoice_items_invoice ON billing.invoice_items (invoice_id);

CREATE TABLE IF NOT EXISTS billing.usage_records (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id              UUID NOT NULL,
    subscription_item_id UUID NOT NULL REFERENCES billing.subscription_items(id),
    quantity            INTEGER NOT NULL CHECK (quantity > 0),
    recorded_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    idempotency_key     TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_billing_usage_records_env ON billing.usage_records (env_id);
CREATE INDEX IF NOT EXISTS idx_billing_usage_records_item ON billing.usage_records (subscription_item_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_billing_usage_records_idempotency
    ON billing.usage_records (env_id, idempotency_key) WHERE idempotency_key IS NOT NULL;

CREATE TABLE IF NOT EXISTS billing.coupons (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id          UUID NOT NULL,
    code            TEXT NOT NULL,
    discount_kind   TEXT NOT NULL CHECK (discount_kind IN ('percentage', 'fixed')),
    value           TEXT NOT NULL,
    redemption_limit INTEGER,
    redeemed        INTEGER NOT NULL DEFAULT 0,
    expires_at      TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_billing_coupons_env ON billing.coupons (env_id);

CREATE TABLE IF NOT EXISTS billing.credits (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id      UUID NOT NULL,
    customer_id UUID NOT NULL,
    remaining   DECIMAL(20,2) NOT NULL CHECK (remaining >= 0),
    currency    TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_billing_credits_env ON billing.credits (env_id);

CREATE TABLE IF NOT EXISTS billing.credit_notes (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id      UUID NOT NULL,
    invoice_id  UUID NOT NULL REFERENCES billing.invoices(id),
    amount      DECIMAL(20,2) NOT NULL CHECK (amount > 0),
    status      TEXT NOT NULL DEFAULT 'issued' CHECK (status IN ('issued', 'void')),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_billing_credit_notes_env ON billing.credit_notes (env_id);

CREATE TABLE IF NOT EXISTS billing.tax_rates (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id          UUID NOT NULL,
    jurisdiction    TEXT NOT NULL,
    inclusive       BOOLEAN NOT NULL DEFAULT false,
    rate_bps        INTEGER NOT NULL CHECK (rate_bps >= 0),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_billing_tax_rates_env ON billing.tax_rates (env_id);

-- Dunning ladder configuration (BIL-12): data, not code. Each env row
-- sets the retry count + backoff seconds; exhaustion cancels the
-- subscription (guarded terminal transition).
CREATE TABLE IF NOT EXISTS billing.dunning_configs (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id          UUID NOT NULL UNIQUE,
    max_retries     INTEGER NOT NULL DEFAULT 3 CHECK (max_retries >= 0),
    backoff_seconds INTEGER NOT NULL DEFAULT 86400 CHECK (backoff_seconds > 0),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_billing_dunning_configs_env ON billing.dunning_configs (env_id);

-- ── Row-level security ────────────────────────────────────────────────
ALTER TABLE billing.products ENABLE ROW LEVEL SECURITY;
ALTER TABLE billing.products FORCE ROW LEVEL SECURITY;
ALTER TABLE billing.prices ENABLE ROW LEVEL SECURITY;
ALTER TABLE billing.prices FORCE ROW LEVEL SECURITY;
ALTER TABLE billing.subscriptions ENABLE ROW LEVEL SECURITY;
ALTER TABLE billing.subscriptions FORCE ROW LEVEL SECURITY;
ALTER TABLE billing.subscription_items ENABLE ROW LEVEL SECURITY;
ALTER TABLE billing.subscription_items FORCE ROW LEVEL SECURITY;
ALTER TABLE billing.invoices ENABLE ROW LEVEL SECURITY;
ALTER TABLE billing.invoices FORCE ROW LEVEL SECURITY;
ALTER TABLE billing.invoice_items ENABLE ROW LEVEL SECURITY;
ALTER TABLE billing.invoice_items FORCE ROW LEVEL SECURITY;
ALTER TABLE billing.usage_records ENABLE ROW LEVEL SECURITY;
ALTER TABLE billing.usage_records FORCE ROW LEVEL SECURITY;
ALTER TABLE billing.coupons ENABLE ROW LEVEL SECURITY;
ALTER TABLE billing.coupons FORCE ROW LEVEL SECURITY;
ALTER TABLE billing.credits ENABLE ROW LEVEL SECURITY;
ALTER TABLE billing.credits FORCE ROW LEVEL SECURITY;
ALTER TABLE billing.credit_notes ENABLE ROW LEVEL SECURITY;
ALTER TABLE billing.credit_notes FORCE ROW LEVEL SECURITY;
ALTER TABLE billing.tax_rates ENABLE ROW LEVEL SECURITY;
ALTER TABLE billing.tax_rates FORCE ROW LEVEL SECURITY;

CREATE POLICY tenant_isolation ON billing.products USING (env_id = current_setting('app.env_id', true)::uuid);
CREATE POLICY tenant_isolation ON billing.prices USING (env_id = current_setting('app.env_id', true)::uuid);
CREATE POLICY tenant_isolation ON billing.subscriptions USING (env_id = current_setting('app.env_id', true)::uuid);
CREATE POLICY tenant_isolation ON billing.subscription_items USING (env_id = current_setting('app.env_id', true)::uuid);
CREATE POLICY tenant_isolation ON billing.invoices USING (env_id = current_setting('app.env_id', true)::uuid);
CREATE POLICY tenant_isolation ON billing.invoice_items USING (env_id = current_setting('app.env_id', true)::uuid);
CREATE POLICY tenant_isolation ON billing.usage_records USING (env_id = current_setting('app.env_id', true)::uuid);
CREATE POLICY tenant_isolation ON billing.coupons USING (env_id = current_setting('app.env_id', true)::uuid);
CREATE POLICY tenant_isolation ON billing.credits USING (env_id = current_setting('app.env_id', true)::uuid);
CREATE POLICY tenant_isolation ON billing.credit_notes USING (env_id = current_setting('app.env_id', true)::uuid);
CREATE POLICY tenant_isolation ON billing.tax_rates USING (env_id = current_setting('app.env_id', true)::uuid);

-- ── Producer tables ────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS billing.outbox (
    id                  UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id              UUID NOT NULL,
    event_type          TEXT NOT NULL,
    stream_family       TEXT NOT NULL,
    payload             JSONB NOT NULL,
    publisher_id_token  TEXT,
    publisher_subject   TEXT,
    relayed             BOOLEAN NOT NULL DEFAULT false,
    relayed_at          TIMESTAMPTZ,
    relay_attempts      INT NOT NULL DEFAULT 0,
    next_retry_at       TIMESTAMPTZ,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_billing_outbox_unrelayed ON billing.outbox (created_at) WHERE relayed = false;

CREATE OR REPLACE FUNCTION billing.notify_outbox_changed() RETURNS trigger AS $$
BEGIN
    PERFORM pg_notify('billing_outbox_changed', NEW.id::text);
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS billing_outbox_notify ON billing.outbox;
CREATE TRIGGER billing_outbox_notify AFTER INSERT ON billing.outbox
    FOR EACH ROW EXECUTE FUNCTION billing.notify_outbox_changed();

CREATE TABLE IF NOT EXISTS billing.audit_journal (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id          UUID NOT NULL,
    action          TEXT NOT NULL,
    target_resource_type TEXT,
    target_resource_id    TEXT,
    actor_user_id   TEXT,
    actor_subject_type TEXT,
    severity        TEXT NOT NULL DEFAULT 'info',
    correlation_id  TEXT,
    before_state    JSONB,
    after_state     JSONB,
    details         JSONB,
    pushed          BOOLEAN NOT NULL DEFAULT false,
    pushed_at      TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_billing_audit_journal_unpushed
    ON billing.audit_journal (created_at) WHERE pushed = false;
ALTER TABLE billing.audit_journal ENABLE ROW LEVEL SECURITY;
ALTER TABLE billing.audit_journal FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS audit_journal_dml ON billing.audit_journal;
CREATE POLICY audit_journal_dml ON billing.audit_journal
    USING (sentin8_context_initialized() AND env_id = sentin8_current_env_id())
    WITH CHECK (sentin8_context_initialized() AND env_id = sentin8_current_env_id());
-- ── Role grants (schema-ownership convention, bank8-ledger's form) ─────
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'bill8_billing_service') THEN
        ALTER ROLE bill8_billing_service SET search_path = billing, public;
        GRANT USAGE ON SCHEMA billing TO bill8_billing_service;
        GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA billing TO bill8_billing_service;
        GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA billing TO bill8_billing_service;
    END IF;
END $$;
