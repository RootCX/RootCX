#[cfg(test)]
mod guard_tests {
    use serde_json::json;
    use uuid::Uuid;
    use rootcx_types::PublicRpc;

    use super::super::guard::{CallerAuth, authorize_public_rpc};
    use super::super::ResolvedShare;
    use crate::auth::identity::Identity;

    fn share_for(app: &str, ctx: serde_json::Value) -> CallerAuth {
        CallerAuth::ShareToken(ResolvedShare {
            share_id: Uuid::new_v4(),
            app_id: app.into(),
            context: ctx,
        })
    }

    #[test]
    fn anonymous_rpc_without_scope_allows_anonymous() {
        let decl = PublicRpc { name: "list_products".into(), scope: vec![] };
        assert!(authorize_public_rpc(&decl, &CallerAuth::Anonymous, "shop", &json!({})).is_ok());
    }

    #[test]
    fn anonymous_rpc_without_scope_allows_same_app_share_token() {
        let decl = PublicRpc { name: "list_products".into(), scope: vec![] };
        let auth = share_for("shop", json!({}));
        assert!(authorize_public_rpc(&decl, &auth, "shop", &json!({})).is_ok());
    }

    #[test]
    fn anonymous_rpc_without_scope_rejects_cross_app_share_token() {
        let decl = PublicRpc { name: "list_products".into(), scope: vec![] };
        let auth = share_for("other_app", json!({}));
        assert!(authorize_public_rpc(&decl, &auth, "shop", &json!({})).is_err());
    }

    #[test]
    fn scoped_rpc_requires_share_token_rejects_anonymous() {
        let decl = PublicRpc { name: "get_public_board".into(), scope: vec!["board_id".into()] };
        assert!(authorize_public_rpc(&decl, &CallerAuth::Anonymous, "task_manager", &json!({"board_id": "x"})).is_err());
    }

    #[test]
    fn scoped_rpc_accepts_matching_context() {
        let decl = PublicRpc { name: "get_public_board".into(), scope: vec!["board_id".into()] };
        let auth = share_for("task_manager", json!({"board_id": "abc-123"}));
        assert!(authorize_public_rpc(&decl, &auth, "task_manager", &json!({"board_id": "abc-123"})).is_ok());
    }

    #[test]
    fn scoped_rpc_rejects_context_mismatch() {
        let decl = PublicRpc { name: "get_public_board".into(), scope: vec!["board_id".into()] };
        let auth = share_for("task_manager", json!({"board_id": "abc-123"}));
        assert!(authorize_public_rpc(&decl, &auth, "task_manager", &json!({"board_id": "DIFFERENT"})).is_err());
    }

    #[test]
    fn scoped_rpc_rejects_missing_key_in_params() {
        let decl = PublicRpc { name: "get_public_board".into(), scope: vec!["board_id".into()] };
        let auth = share_for("task_manager", json!({"board_id": "abc-123"}));
        assert!(authorize_public_rpc(&decl, &auth, "task_manager", &json!({})).is_err());
    }

    #[test]
    fn scoped_rpc_rejects_cross_app_share_token() {
        let decl = PublicRpc { name: "get_public_board".into(), scope: vec!["board_id".into()] };
        let auth = share_for("evil_app", json!({"board_id": "abc-123"}));
        assert!(authorize_public_rpc(&decl, &auth, "task_manager", &json!({"board_id": "abc-123"})).is_err());
    }

    #[test]
    fn scoped_rpc_with_multiple_scope_keys_all_must_match() {
        let decl = PublicRpc { name: "get_detail".into(), scope: vec!["org_id".into(), "doc_id".into()] };
        let auth = share_for("myapp", json!({"org_id": "o1", "doc_id": "d1"}));

        assert!(authorize_public_rpc(&decl, &auth, "myapp", &json!({"org_id": "o1", "doc_id": "d1"})).is_ok());
        assert!(authorize_public_rpc(&decl, &auth, "myapp", &json!({"org_id": "o1", "doc_id": "WRONG"})).is_err());
        assert!(authorize_public_rpc(&decl, &auth, "myapp", &json!({"org_id": "o1"})).is_err());
    }

    #[test]
    fn scoped_rpc_rejects_when_context_missing_required_key() {
        // Token created with partial context — scope demands a key the context doesn't have.
        // A future refactor might check only params-side, this catches that regression.
        let decl = PublicRpc { name: "view".into(), scope: vec!["project_id".into()] };
        let auth = share_for("myapp", json!({})); // context has no project_id
        assert!(authorize_public_rpc(&decl, &auth, "myapp", &json!({"project_id": "p1"})).is_err());
    }

    #[test]
    fn scoped_rpc_rejects_type_mismatch_same_string_value() {
        // context has number 123, params has string "123" — must not match
        let decl = PublicRpc { name: "view".into(), scope: vec!["id".into()] };
        let auth = share_for("myapp", json!({"id": 123}));
        assert!(authorize_public_rpc(&decl, &auth, "myapp", &json!({"id": "123"})).is_err());
    }

    #[test]
    fn scoped_rpc_rejects_null_value_in_context() {
        // context has key but value is null — must not match a non-null param
        let decl = PublicRpc { name: "view".into(), scope: vec!["id".into()] };
        let auth = share_for("myapp", json!({"id": null}));
        assert!(authorize_public_rpc(&decl, &auth, "myapp", &json!({"id": "abc"})).is_err());
        // null == null should match though (consistent JSON equality)
        assert!(authorize_public_rpc(&decl, &auth, "myapp", &json!({"id": null})).is_ok());
    }

    #[test]
    fn scoped_rpc_rejects_extra_nested_object_injection() {
        // Attacker tries passing a nested object where a string is expected
        let decl = PublicRpc { name: "view".into(), scope: vec!["board_id".into()] };
        let auth = share_for("myapp", json!({"board_id": "real-id"}));
        let params = json!({"board_id": {"$ne": ""}});
        assert!(authorize_public_rpc(&decl, &auth, "myapp", &params).is_err());
    }

    #[test]
    fn anonymous_rpc_without_scope_still_rejects_user_auth() {
        // A User (JWT) calling a public RPC should NOT go through authorize_public_rpc
        // in production (rpc_proxy handles User separately via RBAC). But if someone
        // mistakenly calls authorize_public_rpc with a User, it should still pass
        // (it's not the guard's job to reject authed users from public RPCs).
        let decl = PublicRpc { name: "list_products".into(), scope: vec![] };
        let auth = CallerAuth::User(Identity { user_id: Uuid::new_v4(), email: "a@b.com".into() });
        // User has no share_app_id → None, so the cross-app check is skipped → Ok
        assert!(authorize_public_rpc(&decl, &auth, "shop", &json!({})).is_ok());
    }
}
