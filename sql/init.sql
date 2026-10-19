-- SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
-- SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
-- SPDX-FileCopyrightText: Copyright Contributors to the Hotel8:Loyalty
-- SPDX-License-Identifier: Apache-2.0

-- Hotel8:Loyalty - Loyalty program service tables.
CREATE SCHEMA IF NOT EXISTS loyalty;
DO $$ BEGIN
    CREATE TYPE loyalty.tier AS ENUM ('BASE', 'SILVER', 'GOLD', 'PLATINUM', 'DIAMOND');
EXCEPTION WHEN duplicate_object THEN NULL; END $$;

DO $$ BEGIN
    CREATE TYPE loyalty.txn_type AS ENUM ('EARN', 'REDEEM', 'ADJUST', 'EXPIRE', 'TRANSFER');
EXCEPTION WHEN duplicate_object THEN NULL; END $$;

CREATE TABLE IF NOT EXISTS loyalty.programs (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id       UUID NOT NULL,
    program_code TEXT NOT NULL,
    program_name TEXT NOT NULL,
    currency_code TEXT NOT NULL DEFAULT 'PTS',
    points_per_currency NUMERIC(8,2) NOT NULL DEFAULT 1,
    is_active    BOOLEAN NOT NULL DEFAULT true,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_loyalty_programs_env ON loyalty.programs (env_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_loyalty_programs_code ON loyalty.programs (env_id, program_code);

CREATE TABLE IF NOT EXISTS loyalty.members (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    program_id    UUID NOT NULL REFERENCES loyalty.programs(id),
    env_id        UUID NOT NULL,
    member_number TEXT NOT NULL,
    tier          loyalty.tier NOT NULL DEFAULT 'BASE',
    points_balance BIGINT NOT NULL DEFAULT 0,
    lifetime_points BIGINT NOT NULL DEFAULT 0,
    first_name    TEXT NOT NULL DEFAULT '',
    last_name     TEXT NOT NULL DEFAULT '',
    email         TEXT,
    phone         TEXT,
    sentin8_auth_user_id UUID,
    enrolled_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_loyalty_members_program ON loyalty.members (program_id);
CREATE INDEX IF NOT EXISTS idx_loyalty_members_env ON loyalty.members (env_id);
CREATE INDEX IF NOT EXISTS idx_loyalty_members_number ON loyalty.members (member_number);
CREATE INDEX IF NOT EXISTS idx_loyalty_members_email ON loyalty.members (email) WHERE email IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_loyalty_members_number_unique ON loyalty.members (program_id, member_number);

CREATE TABLE IF NOT EXISTS loyalty.point_transactions (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    member_id       UUID NOT NULL REFERENCES loyalty.members(id),
    env_id          UUID NOT NULL,
    transaction_type loyalty.txn_type NOT NULL,
    points          BIGINT NOT NULL,
    description     TEXT,
    reference_type  TEXT,
    reference_id    UUID,
    expires_at      TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_loyalty_pt_member ON loyalty.point_transactions (member_id);
CREATE INDEX IF NOT EXISTS idx_loyalty_pt_env ON loyalty.point_transactions (env_id);

CREATE TABLE IF NOT EXISTS loyalty.tier_rules (
    id             UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    program_id     UUID NOT NULL REFERENCES loyalty.programs(id),
    tier           loyalty.tier NOT NULL,
    min_points     BIGINT NOT NULL DEFAULT 0,
    upgrade_trigger TEXT,
    benefits        JSONB NOT NULL DEFAULT '{}',
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_loyalty_tr_program ON loyalty.tier_rules (program_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_loyalty_tr_unique ON loyalty.tier_rules (program_id, tier);

CREATE TABLE IF NOT EXISTS loyalty.redemption_catalog (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    program_id   UUID NOT NULL REFERENCES loyalty.programs(id),
    item_code    TEXT NOT NULL,
    item_name    TEXT NOT NULL,
    description  TEXT,
    points_cost  BIGINT NOT NULL DEFAULT 0,
    category     TEXT NOT NULL DEFAULT 'GENERAL',
    is_active    BOOLEAN NOT NULL DEFAULT true,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_loyalty_rc_program ON loyalty.redemption_catalog (program_id);
CREATE UNIQUE INDEX IF NOT EXISTS idx_loyalty_rc_code ON loyalty.redemption_catalog (program_id, item_code);

ALTER TABLE loyalty.programs ENABLE ROW LEVEL SECURITY;
ALTER TABLE loyalty.programs FORCE ROW LEVEL SECURITY;
ALTER TABLE loyalty.members ENABLE ROW LEVEL SECURITY;
ALTER TABLE loyalty.members FORCE ROW LEVEL SECURITY;
ALTER TABLE loyalty.point_transactions ENABLE ROW LEVEL SECURITY;
ALTER TABLE loyalty.point_transactions FORCE ROW LEVEL SECURITY;
ALTER TABLE loyalty.tier_rules ENABLE ROW LEVEL SECURITY;
ALTER TABLE loyalty.tier_rules FORCE ROW LEVEL SECURITY;
ALTER TABLE loyalty.redemption_catalog ENABLE ROW LEVEL SECURITY;
ALTER TABLE loyalty.redemption_catalog FORCE ROW LEVEL SECURITY;
-- env_id is the sole tenant key (architecture-principles.md §4.3).
DROP POLICY IF EXISTS loyalty_programs_rls ON loyalty.programs;
CREATE POLICY loyalty_programs_rls ON loyalty.programs USING (sentin8_context_initialized() AND env_id = sentin8_current_env_id());
DROP POLICY IF EXISTS loyalty_members_rls ON loyalty.members;
CREATE POLICY loyalty_members_rls ON loyalty.members USING (sentin8_context_initialized() AND env_id = sentin8_current_env_id());
DROP POLICY IF EXISTS loyalty_pt_rls ON loyalty.point_transactions;
CREATE POLICY loyalty_pt_rls ON loyalty.point_transactions USING (sentin8_context_initialized() AND env_id = sentin8_current_env_id());
DROP POLICY IF EXISTS loyalty_tr_rls ON loyalty.tier_rules;
CREATE POLICY loyalty_tr_rls ON loyalty.tier_rules USING (program_id IN (SELECT id FROM loyalty.programs));
DROP POLICY IF EXISTS loyalty_rc_rls ON loyalty.redemption_catalog;
CREATE POLICY loyalty_rc_rls ON loyalty.redemption_catalog USING (program_id IN (SELECT id FROM loyalty.programs));
-- ──────────────────────────────────────────────────────────────────────────
-- Local outbox (DBX, architecture-principles §6.7) - producer path:
-- rows are appended in the SAME transaction as the domain change; the
-- shell's post-commit relay pushes them to Bus POST /v1/produce
-- (M2M sentin8:bus:produce, idempotency_key = row id).
-- Generated from sentin8_outbox::outbox_table_sql("loyalty").
-- ──────────────────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS loyalty.outbox (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id          UUID NOT NULL,
    event_type      TEXT NOT NULL,
    stream_family   TEXT NOT NULL,
    payload         JSONB NOT NULL,
    publisher_id_token  TEXT,
    publisher_subject   TEXT,
    relayed         BOOLEAN NOT NULL DEFAULT false,
    relayed_at      TIMESTAMPTZ,
    relay_attempts  INT NOT NULL DEFAULT 0,
    next_retry_at   TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_outbox_unrelayed
    ON loyalty.outbox (created_at) WHERE relayed = false;
ALTER TABLE loyalty.outbox ENABLE ROW LEVEL SECURITY;
ALTER TABLE loyalty.outbox FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS outbox_select ON loyalty.outbox;
CREATE POLICY outbox_select ON loyalty.outbox FOR SELECT
    USING (sentin8_context_initialized() AND env_id = sentin8_current_env_id());
DROP POLICY IF EXISTS outbox_insert ON loyalty.outbox;
CREATE POLICY outbox_insert ON loyalty.outbox FOR INSERT
    WITH CHECK (sentin8_context_initialized() AND env_id = sentin8_current_env_id());
DROP POLICY IF EXISTS outbox_update ON loyalty.outbox;
CREATE POLICY outbox_update ON loyalty.outbox FOR UPDATE
    USING (sentin8_context_initialized() AND env_id = sentin8_current_env_id());
CREATE OR REPLACE FUNCTION loyalty.loyalty_notify_outbox() RETURNS TRIGGER
    LANGUAGE plpgsql AS $fn$
BEGIN
    PERFORM pg_notify('loyalty_outbox_changed', '');
    RETURN NEW;
END;
$fn$;
DROP TRIGGER IF EXISTS outbox_notify_trigger ON loyalty.outbox;
CREATE TRIGGER outbox_notify_trigger
    AFTER INSERT ON loyalty.outbox
    FOR EACH ROW
    EXECUTE FUNCTION loyalty.loyalty_notify_outbox();

-- ──────────────────────────────────────────────────────────────────────────
-- Local audit journal (architecture-principles §6.8): same-transaction
-- journal rows; the shell's post-commit pusher ships them to Sys
-- POST /v1/audit (M2M sentin8:sys:audit:write). A Sys outage only delays
-- delivery - rows persist until pushed.
-- Generated from sentin8_audit::audit_journal_sql("loyalty").
-- ──────────────────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS loyalty.audit_journal (
    id              UUID PRIMARY KEY,
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
    pushed_at       TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_loyalty_audit_journal_unpushed
    ON loyalty.audit_journal (created_at) WHERE pushed = false;
ALTER TABLE loyalty.audit_journal ENABLE ROW LEVEL SECURITY;
ALTER TABLE loyalty.audit_journal FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS audit_journal_dml ON loyalty.audit_journal;
CREATE POLICY audit_journal_dml ON loyalty.audit_journal
    USING (sentin8_context_initialized() AND env_id = sentin8_current_env_id())
    WITH CHECK (sentin8_context_initialized() AND env_id = sentin8_current_env_id());

-- ──────────────────────────────────────────────────────────────────────────
-- Per-service role grants + search_path (§4.3e / VDC §VDC.3):
-- hotel8_{leaf}_service connects via PostgreSQL 18 OAuth (scope→role map in
-- hotel8.sys.pg_oauth_scope_map). search_path puts the service's OWN schema
-- FIRST (outbox/audit_journal resolution for the shell's relay + journal
-- pusher; unqualified domain tables resolve here), then public (RLS helpers).
-- `sentin8-cli vertical enroll`'s never-clobber block preserves this role-level setting.
-- ──────────────────────────────────────────────────────────────────────────
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'hotel8_loyalty_service') THEN
        ALTER ROLE hotel8_loyalty_service SET search_path = loyalty, public;
        GRANT USAGE ON SCHEMA loyalty TO hotel8_loyalty_service;
        GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA loyalty TO hotel8_loyalty_service;
        GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA loyalty TO hotel8_loyalty_service;
    END IF;
END $$;


-- ═════════════════════════════════════════════════════════════════════════════
-- From migrations: extended
-- ═══════════════════════════════════════════════════════════════════════════
-- Migration 33: Hotel8:Loyalty extended for remaining H5 stories
-- H5.2.2: Program types (LOYALTY_NOW, POINTS_BASED, HYBRID)
-- H5.2.5: Reward types (FREE_NIGHT, ROOM_UPGRADE, DINING, SPA, AMENITY, DISCOUNT)
-- H5.3.5: Manual points adjustment
-- H5.5.4: Reward code/voucher generation on redemption

CREATE SCHEMA IF NOT EXISTS loyalty;
ALTER TABLE loyalty.programs
    ADD COLUMN IF NOT EXISTS program_type TEXT NOT NULL DEFAULT 'POINTS_BASED',
    ADD COLUMN IF NOT EXISTS tier_multiplier_enabled BOOLEAN NOT NULL DEFAULT true;

ALTER TABLE loyalty.redemption_catalog
    ADD COLUMN IF NOT EXISTS reward_type TEXT NOT NULL DEFAULT 'DISCOUNT',
    ADD COLUMN IF NOT EXISTS voucher_validity_days INTEGER NOT NULL DEFAULT 365;

CREATE TABLE IF NOT EXISTS loyalty.redemptions (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    member_id       UUID NOT NULL REFERENCES loyalty.members(id),
    program_id      UUID NOT NULL REFERENCES loyalty.programs(id),
    catalog_item_id UUID NOT NULL REFERENCES loyalty.redemption_catalog(id),
    points_spent    BIGINT NOT NULL,
    voucher_code    TEXT,
    status          TEXT NOT NULL DEFAULT 'ISSUED',
    redeemed_at     TIMESTAMPTZ,
    expires_at      TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_loyalty_rd_member ON loyalty.redemptions (member_id);
CREATE INDEX IF NOT EXISTS idx_loyalty_rd_voucher ON loyalty.redemptions (voucher_code) WHERE voucher_code IS NOT NULL;

ALTER TABLE loyalty.redemptions ENABLE ROW LEVEL SECURITY;
ALTER TABLE loyalty.redemptions FORCE ROW LEVEL SECURITY;
CREATE POLICY loyalty_rd_rls ON loyalty.redemptions USING (
    member_id IN (SELECT id FROM loyalty.members)
);
