CREATE TABLE rootcx_system.app_deletions (
    id UUID PRIMARY KEY,
    app_id TEXT NOT NULL,
    installation_id UUID NOT NULL REFERENCES rootcx_system.app_installations(id),
    application_name TEXT NOT NULL,
    requested_by UUID NOT NULL,
    agent_app TEXT,
    state TEXT NOT NULL DEFAULT 'awaiting_confirmation'
        CHECK (state IN ('awaiting_confirmation','deleting','retry','succeeded')),
    phase TEXT NOT NULL DEFAULT 'quiesce'
        CHECK (phase IN ('quiesce','data','files','history','complete')),
    expires_at TIMESTAMPTZ NOT NULL DEFAULT now() + interval '10 minutes',
    confirmed_at TIMESTAMPTZ,
    attempts INTEGER NOT NULL DEFAULT 0,
    retry_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    error TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE UNIQUE INDEX app_deletions_one_committed_installation
    ON rootcx_system.app_deletions(installation_id) WHERE state <> 'awaiting_confirmation';
CREATE INDEX app_deletions_pending ON rootcx_system.app_deletions(retry_at)
    WHERE state IN ('deleting','retry');
CREATE TABLE rootcx_system.app_deletion_events (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    operation_id UUID NOT NULL REFERENCES rootcx_system.app_deletions(id),
    event TEXT NOT NULL,
    phase TEXT NOT NULL,
    actor_uid UUID NOT NULL,
    delegator_uid UUID,
    agent_app TEXT,
    detail TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
REVOKE ALL ON rootcx_system.app_deletions, rootcx_system.app_deletion_events FROM PUBLIC;
