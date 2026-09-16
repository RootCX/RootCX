CREATE TABLE rootcx_system.public_execution_principals (
    installation_id UUID PRIMARY KEY REFERENCES rootcx_system.app_installations(id),
    user_id UUID NOT NULL UNIQUE REFERENCES rootcx_system.users(id)
);

CREATE TABLE rootcx_system.publications (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    version BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    name TEXT NOT NULL,
    consumer_app TEXT NOT NULL,
    provider_app TEXT NOT NULL,
    consumer_installation_id UUID NOT NULL REFERENCES rootcx_system.app_installations(id),
    provider_installation_id UUID NOT NULL REFERENCES rootcx_system.app_installations(id),
    definition JSONB NOT NULL,
    status TEXT NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'disabled', 'revoked', 'expired')),
    approved_by UUID NOT NULL REFERENCES rootcx_system.users(id),
    reason TEXT NOT NULL CHECK (length(btrim(reason)) > 0),
    expires_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ,
    CHECK (jsonb_typeof(definition) = 'object'),
    CHECK (expires_at IS NULL OR expires_at > created_at)
);

CREATE UNIQUE INDEX publications_one_nonterminal
    ON rootcx_system.publications (consumer_installation_id, name)
    WHERE status IN ('active', 'disabled');
CREATE INDEX publications_lookup
    ON rootcx_system.publications (consumer_app, name, created_at DESC);

CREATE TABLE rootcx_system.publication_audit (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    publication_id UUID NOT NULL REFERENCES rootcx_system.publications(id),
    actor_id UUID NOT NULL REFERENCES rootcx_system.users(id),
    operation TEXT NOT NULL CHECK (operation IN ('approved', 'disabled', 'enabled', 'revoked', 'expired')),
    reason TEXT NOT NULL,
    snapshot JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX publication_audit_lookup
    ON rootcx_system.publication_audit (publication_id, created_at DESC);

CREATE TABLE rootcx_system.publication_read_audit (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    publication_id UUID NOT NULL REFERENCES rootcx_system.publications(id),
    publication_version BIGINT,
    principal_id UUID REFERENCES rootcx_system.users(id),
    consumer_app TEXT,
    provider_app TEXT,
    entity TEXT,
    action TEXT,
    grant_id UUID,
    grant_version BIGINT,
    projection TEXT[],
    outcome TEXT,
    row_count BIGINT,
    correlation_id UUID,
    created_at TIMESTAMPTZ DEFAULT now()
);
CREATE INDEX publication_read_audit_lookup
    ON rootcx_system.publication_read_audit (publication_id, created_at DESC);

CREATE FUNCTION rootcx_system.guard_publication_snapshot()
RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NOT (
           (OLD.status = 'active' AND NEW.status IN ('disabled', 'revoked', 'expired'))
           OR (OLD.status = 'disabled' AND NEW.status IN ('active', 'revoked', 'expired'))
       )
       OR (NEW.status IN ('active', 'disabled') AND OLD.expires_at <= statement_timestamp())
       OR NEW.version <> OLD.version + 1
       OR (to_jsonb(NEW) - ARRAY['status', 'version', 'revoked_at'])
          IS DISTINCT FROM (to_jsonb(OLD) - ARRAY['status', 'version', 'revoked_at']) THEN
        RAISE EXCEPTION 'publication snapshots are immutable and terminal' USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END;
$$;
CREATE TRIGGER publication_snapshot_immutable
    BEFORE UPDATE ON rootcx_system.publications
    FOR EACH ROW EXECUTE FUNCTION rootcx_system.guard_publication_snapshot();

-- Migrations precede RBAC bootstrap on a fresh Core. Match its base catalog;
-- the install hook also registers keys after a legacy RBAC schema upgrade.
CREATE TABLE IF NOT EXISTS rootcx_system.rbac_permissions (
    key TEXT PRIMARY KEY,
    description TEXT NOT NULL DEFAULT '',
    source_app TEXT
);
DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns
        WHERE table_schema = 'rootcx_system'
          AND table_name = 'rbac_permissions' AND column_name = 'app_id'
    ) THEN
        INSERT INTO rootcx_system.rbac_permissions (key, description)
        VALUES ('admin:publications.manage', 'Approve, inspect and revoke public data publications')
        ON CONFLICT (key) DO NOTHING;
        INSERT INTO rootcx_system.rbac_permissions (key, description, source_app)
        SELECT 'app:' || id || ':publications.approve', 'Approve public data from this app', id
        FROM rootcx_system.apps ON CONFLICT (key) DO NOTHING;
    END IF;
END;
$$;
