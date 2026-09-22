CREATE TABLE IF NOT EXISTS rootcx_system.source_projects (
    app_id TEXT PRIMARY KEY REFERENCES rootcx_system.apps(id) ON DELETE RESTRICT,
    head_commit TEXT NOT NULL,
    deployed_commit TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE TABLE IF NOT EXISTS rootcx_system.source_runs (
    id UUID PRIMARY KEY,
    app_id TEXT NOT NULL REFERENCES rootcx_system.source_projects(app_id),
    requested_by UUID NOT NULL,
    request_id UUID NOT NULL,
    base_commit TEXT NOT NULL,
    prompt TEXT NOT NULL,
    status TEXT NOT NULL CHECK (status IN ('queued','coding','publishing','succeeded','failed','interrupted','needs_recovery')),
    commit_id TEXT,
    message TEXT NOT NULL DEFAULT '',
    error TEXT,
    resumed_by UUID,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE(app_id, requested_by, request_id)
);
CREATE UNIQUE INDEX IF NOT EXISTS source_runs_one_active ON rootcx_system.source_runs(app_id)
    WHERE status IN ('queued','coding','publishing','needs_recovery');
REVOKE ALL ON rootcx_system.source_projects, rootcx_system.source_runs FROM PUBLIC;
