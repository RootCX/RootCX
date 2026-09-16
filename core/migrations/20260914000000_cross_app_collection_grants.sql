-- Core-owned installation generations and collection-only cross-app grants.
-- Grants are deliberately separate from user-to-user delegations: a data
-- relationship has a provider, a consumer installation and a frozen schema
-- projection, not merely two user ids.

CREATE TABLE IF NOT EXISTS rootcx_system.app_installations (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    app_id          TEXT NOT NULL,
    generation      BIGINT NOT NULL,
    active          BOOLEAN NOT NULL DEFAULT TRUE,
    installed_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    uninstalled_at  TIMESTAMPTZ,
    UNIQUE (app_id, generation)
);

CREATE UNIQUE INDEX IF NOT EXISTS app_installations_one_active
    ON rootcx_system.app_installations (app_id) WHERE active;

-- Existing installations receive generation 1.  This is a data migration, not
-- an inferred grant: no cross-app authority is created by this backfill.
INSERT INTO rootcx_system.app_installations (app_id, generation)
SELECT a.id, 1
  FROM rootcx_system.apps a
 WHERE NOT EXISTS (
     SELECT 1 FROM rootcx_system.app_installations i WHERE i.app_id = a.id
 );

CREATE TABLE IF NOT EXISTS rootcx_system.cross_app_collection_grants (
    id                          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    consumer_app                TEXT NOT NULL,
    provider_app                TEXT NOT NULL,
    consumer_installation_id    UUID NOT NULL REFERENCES rootcx_system.app_installations(id),
    provider_installation_id    UUID NOT NULL REFERENCES rootcx_system.app_installations(id),
    entity                      TEXT NOT NULL,
    actions                     TEXT[] NOT NULL,
    field_snapshot              TEXT[] NOT NULL,
    write_field_snapshot        TEXT[] NOT NULL DEFAULT '{}',
    status                      TEXT NOT NULL DEFAULT 'pending'
        CHECK (status IN ('pending', 'active', 'disabled', 'revoked', 'expired')),
    version                     BIGINT NOT NULL DEFAULT 1 CHECK (version > 0),
    expires_at                  TIMESTAMPTZ,
    requested_by                UUID NOT NULL REFERENCES rootcx_system.users(id),
    approved_by                 UUID REFERENCES rootcx_system.users(id),
    revoked_at                  TIMESTAMPTZ,
    reason                      TEXT NOT NULL DEFAULT '',
    created_at                  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at                  TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (consumer_app <> provider_app),
    CHECK (actions <@ ARRAY['list', 'read', 'create', 'update', 'delete']::TEXT[]),
    CHECK (cardinality(actions) > 0),
    CHECK (array_position(actions, NULL) IS NULL),
    CHECK (cardinality(field_snapshot) > 0),
    CHECK (array_position(field_snapshot, NULL) IS NULL),
    CHECK (array_position(write_field_snapshot, NULL) IS NULL),
    CHECK ((actions && ARRAY['create', 'update']::TEXT[])
           = (cardinality(write_field_snapshot) > 0)),
    CHECK (NOT (write_field_snapshot && ARRAY['id', 'created_at', 'updated_at']::TEXT[])),
    CHECK (expires_at IS NULL OR expires_at > created_at)
);

CREATE UNIQUE INDEX IF NOT EXISTS cross_app_collection_grants_one_active
    ON rootcx_system.cross_app_collection_grants
        (consumer_installation_id, provider_installation_id, entity)
    WHERE status IN ('pending', 'active');

CREATE INDEX IF NOT EXISTS cross_app_collection_grants_lookup
    ON rootcx_system.cross_app_collection_grants
        (consumer_app, provider_app, entity, status, version DESC);

CREATE TABLE IF NOT EXISTS rootcx_system.cross_app_read_audit (
    id                          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    consumer_app                TEXT NOT NULL,
    provider_app                TEXT NOT NULL,
    consumer_installation_id    UUID,
    provider_installation_id    UUID,
    entity                      TEXT NOT NULL,
    action                      TEXT NOT NULL,
    grant_id                    UUID,
    grant_version               BIGINT,
    principal_id                UUID,
    responsible_human_id        UUID,
    outcome                     TEXT NOT NULL,
    denial_category             TEXT,
    row_count                   BIGINT,
    correlation_id              UUID NOT NULL,
    projection                  TEXT[] NOT NULL DEFAULT '{}',
    created_at                  TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (outcome IN ('started', 'success', 'denied', 'error')),
    CHECK (row_count IS NULL OR row_count >= 0)
);

CREATE INDEX IF NOT EXISTS cross_app_read_audit_lookup
    ON rootcx_system.cross_app_read_audit (consumer_app, provider_app, created_at DESC);

-- Immutable governance history.  The full serialized grant state is retained
-- on both sides of every transition so exports and reviews never have to
-- reconstruct an old field scope from mutable rows.
CREATE TABLE IF NOT EXISTS rootcx_system.cross_app_grant_audit (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    grant_id        UUID NOT NULL REFERENCES rootcx_system.cross_app_collection_grants(id) ON DELETE RESTRICT,
    operation       TEXT NOT NULL,
    actor_id        UUID REFERENCES rootcx_system.users(id),
    reason          TEXT NOT NULL DEFAULT '',
    before_state    JSONB,
    after_state     JSONB NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS cross_app_grant_audit_lookup
    ON rootcx_system.cross_app_grant_audit (grant_id, created_at DESC);

-- Provider RLS uses this Core-owned predicate as an additional CRUD gate.  It
-- does not replace provider row policies: the grant can satisfy the table-level
-- action permission, while any provider ownership/row predicate still applies.
-- Core binds the exact authorized operation in rootcx.cross_app_action before
-- applying the restricted role. Mutation transactions need SELECT policy access
-- for row targeting and RETURNING, but that never authorizes a standalone read.
-- Missing or mismatched operation context fails closed, even for multi-action
-- grants. Source, grant id and operation are never application-controlled.
-- Expiry is evaluated at statement start, not the start of a transaction that
-- may have waited for a grant lock before it could execute the provider query.
CREATE OR REPLACE FUNCTION rootcx_system.cross_app_grant_allows(
    p_required TEXT,
    p_target_app TEXT,
    p_source_app TEXT,
    p_grant_id UUID
)
RETURNS BOOLEAN
LANGUAGE sql
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, rootcx_system
AS $$
    SELECT p_grant_id IS NOT NULL
       AND p_target_app <> ''
       AND p_source_app <> ''
       AND EXISTS (
            SELECT 1
              FROM rootcx_system.cross_app_collection_grants g
              JOIN rootcx_system.app_installations ci
                ON ci.id = g.consumer_installation_id
              JOIN rootcx_system.app_installations pi
                ON pi.id = g.provider_installation_id
              JOIN rootcx_system.apps ca ON ca.id = g.consumer_app
              JOIN rootcx_system.apps pa ON pa.id = g.provider_app
              CROSS JOIN (
                  SELECT nullif(current_setting('rootcx.cross_app_action', true), '') AS action
              ) operation
             WHERE g.id = p_grant_id
               AND g.consumer_app = p_source_app
               AND g.provider_app = p_target_app
               AND operation.action = ANY(g.actions)
               AND operation.action IN ('list', 'read', 'create', 'update', 'delete')
               AND (
                   p_required = 'app:' || p_target_app || ':' || g.entity || '.' ||
                       CASE WHEN operation.action = 'list' THEN 'read' ELSE operation.action END
                   OR (
                       operation.action IN ('create', 'update', 'delete')
                       AND p_required = 'app:' || p_target_app || ':' || g.entity || '.read'
                   )
               )
               AND g.status = 'active'
               AND (g.expires_at IS NULL OR g.expires_at > statement_timestamp())
               AND ci.app_id = g.consumer_app AND ci.active = TRUE
               AND pi.app_id = g.provider_app AND pi.active = TRUE
               AND ca.status IN ('installed', 'system')
               AND pa.status IN ('installed', 'system')
       )
$$;
