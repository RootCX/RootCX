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

ALTER TABLE rootcx_system.source_runs ADD COLUMN IF NOT EXISTS phase TEXT NOT NULL DEFAULT 'preparing';

CREATE TABLE IF NOT EXISTS rootcx_system.source_conversations (
    id UUID PRIMARY KEY,
    app_id TEXT NOT NULL REFERENCES rootcx_system.source_projects(app_id),
    user_id UUID NOT NULL,
    title TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
ALTER TABLE rootcx_system.source_runs ADD COLUMN IF NOT EXISTS conversation_id UUID REFERENCES rootcx_system.source_conversations(id);
ALTER TABLE rootcx_system.source_runs ADD COLUMN IF NOT EXISTS activity JSONB NOT NULL DEFAULT '[]';
CREATE INDEX IF NOT EXISTS source_conversations_owner ON rootcx_system.source_conversations(app_id,user_id,created_at DESC);
CREATE INDEX IF NOT EXISTS source_runs_conversation ON rootcx_system.source_runs(conversation_id,created_at);
DROP INDEX IF EXISTS rootcx_system.source_runs_one_active;
CREATE UNIQUE INDEX IF NOT EXISTS source_runs_one_active ON rootcx_system.source_runs(app_id)
    WHERE status IN ('coding','publishing','needs_recovery');
REVOKE ALL ON rootcx_system.source_conversations FROM PUBLIC;

CREATE OR REPLACE FUNCTION rootcx_system.record_source_activity() RETURNS trigger LANGUAGE plpgsql AS $$
BEGIN
    IF NEW.message <> OLD.message AND jsonb_array_length(OLD.activity) < 100 THEN
        NEW.activity := OLD.activity || jsonb_build_array(jsonb_build_object('message',NEW.message,'phase',NEW.phase,'at',now()));
    END IF;
    RETURN NEW;
END $$;
CREATE OR REPLACE TRIGGER source_run_activity BEFORE UPDATE ON rootcx_system.source_runs
    FOR EACH ROW EXECUTE FUNCTION rootcx_system.record_source_activity();

-- Recover the existing server-side exchanges as one conversation per person/app.
INSERT INTO rootcx_system.source_conversations(id,app_id,user_id,title,created_at)
SELECT md5(app_id || ':' || requested_by::text)::uuid,app_id,requested_by,'Conversation précédente',min(created_at)
FROM rootcx_system.source_runs WHERE conversation_id IS NULL GROUP BY app_id,requested_by
ON CONFLICT(id) DO NOTHING;
UPDATE rootcx_system.source_runs SET conversation_id=md5(app_id || ':' || requested_by::text)::uuid
WHERE conversation_id IS NULL;
