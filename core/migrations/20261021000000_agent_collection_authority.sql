-- Distinguish native agent delegation from module installation grants without
-- rewriting the existing append-only history or inventing a grant for an agent.
ALTER TABLE rootcx_system.cross_app_read_audit
    ADD COLUMN authorization_kind TEXT NOT NULL DEFAULT 'application_grant'
    CHECK (authorization_kind IN ('application_grant', 'agent_delegation'));
