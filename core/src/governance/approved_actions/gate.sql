CREATE OR REPLACE FUNCTION rootcx_system.check_approved_action_access(p_required TEXT)
RETURNS BOOLEAN AS $$
DECLARE
    v_approval UUID := nullif(current_setting('rootcx.approved_action_id', true), '')::uuid;
    v_user UUID := nullif(current_setting('rootcx.user_id', true), '')::uuid;
    v_key TEXT;
BEGIN
    IF v_approval IS NULL OR v_user IS NULL
       OR coalesce(current_setting('rootcx.publication_id', true), '') <> ''
       OR current_setting('rootcx.human_data_request', true) = '1'
       OR current_setting('rootcx.invocation_kind', true) <> 'action' THEN
        RETURN FALSE;
    END IF;
    SELECT 'app:' || g.app_id || ':action:' || g.action_id INTO v_key
      FROM rootcx_system.action_approvals g
      JOIN rootcx_system.backend_releases r
        ON r.app_id = g.app_id AND r.revision = g.revision AND r.digest = g.backend_digest
      JOIN rootcx_system.app_installations i
        ON i.id = g.installation_id AND i.app_id = g.app_id AND i.active
      JOIN rootcx_system.apps a
        ON a.id = g.app_id AND a.manifest = g.manifest AND a.status IN ('installed', 'system')
     WHERE g.id = v_approval AND g.revoked_at IS NULL
       AND g.app_id = current_setting('rootcx.app_id', true)
       AND g.action_id = current_setting('rootcx.action_id', true)
       AND p_required = ANY(g.permissions);
    IF v_key IS NULL OR NOT rootcx_system.has_permission(v_user, v_key) THEN
        RETURN FALSE;
    END IF;
    IF current_setting('rootcx.is_delegated', true) = '1' THEN
        RETURN rootcx_system.match_permission(
            string_to_array(coalesce(current_setting('rootcx.effective_perms', true), ''), ','), v_key
        );
    END IF;
    RETURN TRUE;
END;
$$ LANGUAGE plpgsql STABLE SECURITY DEFINER SET search_path = pg_catalog, rootcx_system;
REVOKE ALL ON FUNCTION rootcx_system.check_approved_action_access(TEXT) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION rootcx_system.check_approved_action_access(TEXT) TO rootcx_app_executor;
REVOKE ALL ON rootcx_system.backend_releases,
    rootcx_system.action_approvals, rootcx_system.action_executions FROM rootcx_app_executor;
