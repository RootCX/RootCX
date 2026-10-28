CREATE TABLE rootcx_system.agent_harness_state (
    app_id TEXT NOT NULL REFERENCES rootcx_system.apps(id) ON DELETE CASCADE,
    user_id UUID NOT NULL REFERENCES rootcx_system.users(id) ON DELETE CASCADE,
    authority TEXT NOT NULL,
    session_id TEXT NOT NULL,
    namespace TEXT NOT NULL,
    key TEXT NOT NULL,
    value JSONB NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (app_id, user_id, authority, session_id, namespace, key)
);
REVOKE ALL ON rootcx_system.agent_harness_state FROM PUBLIC;
DO $$ BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'rootcx_app_executor') THEN
        REVOKE ALL ON rootcx_system.agent_harness_state FROM rootcx_app_executor;
    END IF;
END $$;
