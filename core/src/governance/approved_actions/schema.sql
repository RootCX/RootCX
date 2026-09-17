CREATE TABLE IF NOT EXISTS rootcx_system.backend_releases (
    app_id TEXT PRIMARY KEY,
    revision UUID NOT NULL,
    digest TEXT
);
CREATE TABLE IF NOT EXISTS rootcx_system.action_approvals (
    id UUID PRIMARY KEY,
    app_id TEXT NOT NULL,
    action_id TEXT NOT NULL,
    installation_id UUID NOT NULL,
    revision UUID NOT NULL,
    backend_digest TEXT NOT NULL,
    manifest JSONB NOT NULL,
    permissions TEXT[] NOT NULL,
    approved_by UUID NOT NULL,
    approved_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at TIMESTAMPTZ,
    revoked_by UUID,
    revocation_reason TEXT
);
CREATE UNIQUE INDEX IF NOT EXISTS action_approvals_active
    ON rootcx_system.action_approvals(app_id, action_id) WHERE revoked_at IS NULL;
CREATE TABLE IF NOT EXISTS rootcx_system.action_executions (
    id UUID PRIMARY KEY,
    approval_id UUID NOT NULL REFERENCES rootcx_system.action_approvals(id),
    user_id UUID NOT NULL,
    actor_id UUID,
    delegator_id UUID,
    started_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at TIMESTAMPTZ,
    success BOOLEAN,
    error TEXT
);
REVOKE ALL ON rootcx_system.backend_releases,
    rootcx_system.action_approvals, rootcx_system.action_executions FROM PUBLIC;
