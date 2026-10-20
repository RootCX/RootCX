//! One collection lifecycle for application grants and delegated agent tools.
//! Authorization, provider transaction, audit and projection stay together so
//! callers cannot omit a grant recheck or commit a mutation without its audit.

use serde_json::Value as JsonValue;
use sqlx::PgPool;
use uuid::Uuid;

use super::collection_reads::{self, EqualityMode, PAGE_OPTION_KEYS};
use super::enforcement::{
    ContextState, InvocationContext, TIMEOUT_AGENT_TOOL_MS, TIMEOUT_INTERACTIVE_MS,
};
use super::{cross_app, cross_app_mutations};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Origin {
    Collection,
    QueryTool,
    DelegatedAgent,
}

/// Core-only execution policy. Constructors retain the existing Adapter rules;
/// no payload can choose an identity, invocation, timeout or audit origin.
pub(crate) struct OperationContext {
    state: Option<ContextState>,
    invocation: InvocationContext,
    idempotency_key: Option<String>,
    origin: Origin,
}

impl OperationContext {
    pub(crate) fn collection(
        state: Option<ContextState>,
        invocation: &InvocationContext,
        idempotency_key: Option<&str>,
    ) -> Self {
        Self {
            state,
            invocation: invocation.clone(),
            idempotency_key: idempotency_key.map(str::to_owned),
            origin: Origin::Collection,
        }
    }

    pub(crate) fn query_tool(state: ContextState) -> Self {
        Self {
            state: Some(state),
            invocation: InvocationContext::default(),
            idempotency_key: None,
            origin: Origin::QueryTool,
        }
    }
}

/// Only the Core-dispatched agent can select native delegated RBAC. Workers,
/// workflows and generic HTTP tools continue through application grants.
pub(crate) async fn execute_agent(
    ctx: &crate::tools::ToolContext,
    provider_app: &str,
    op: &str,
    entity: &str,
    data: JsonValue,
) -> Result<JsonValue, String> {
    let Some(authority) = ctx.delegated_authority() else {
        return Err("delegated agent authority required".into());
    };
    // Revalidation remains the native authority mechanism, including origin,
    // every parent, the human and the admitted task ceiling. Never trust args.
    let permissions = authority.revalidate(&ctx.pool).await?;
    crate::tools::check_permission(
        &permissions,
        if matches!(op, "findAll" | "findPage") {
            "tool:query_data"
        } else {
            "tool:mutate_data"
        },
    )?;
    let state = ContextState {
        user_id: Some(authority.human()),
        is_delegated: true,
        effective_perms: permissions,
        connection_id: None,
        audit_actor_id: Some(ctx.user_id),
        audit_delegator_id: ctx.invoker_user_id,
        public_execution: None,
        approved_action: None,
    };
    execute(
        &ctx.pool,
        &ctx.app_id,
        provider_app,
        op,
        entity,
        data,
        OperationContext {
            state: Some(state),
            invocation: InvocationContext::default(),
            idempotency_key: None,
            origin: Origin::DelegatedAgent,
        },
    )
    .await
}

async fn authorize_agent_collection(
    pool: &PgPool,
    state: &ContextState,
    app: &str,
    entity: &str,
    action: &str,
) -> Result<(), crate::api_error::ApiError> {
    use crate::api_error::ApiError;
    let action = if matches!(action, cross_app::ACTION_LIST | cross_app::ACTION_READ) {
        "read"
    } else {
        action
    };
    let key = format!("app:{app}:{entity}.{action}");
    let permits = |key: &str| super::authority::has_permission(&state.effective_perms, key);
    let permitted = permits(&key)
        || permits(&format!("{key}.own"))
        || (action == "read" && permits(&format!("{key}.shared")));
    if !permitted {
        return Err(ApiError::Forbidden(
            "delegated collection permission denied".into(),
        ));
    }
    crate::routes::crud::validate_app_id(app)?;
    let installed: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM rootcx_system.apps a
         JOIN rootcx_system.app_installations i ON i.app_id = a.id AND i.active
         WHERE a.id = $1 AND a.status IN ('installed', 'system'))",
    )
    .bind(app)
    .fetch_one(pool)
    .await?;
    if !installed
        || !crate::manifest::entity_exists(pool, app, entity)
            .await
            .map_err(|_| ApiError::NotFound("collection unavailable".into()))?
    {
        return Err(ApiError::NotFound("collection unavailable".into()));
    }
    Ok(())
}

/// The consumer must come from Core-bound provenance, never request data.
/// Module grants retain their frozen projection; agents use native row/column
/// policies. SQL errors stay in Core audit categories.
pub(crate) async fn execute(
    pool: &PgPool,
    consumer_app: &str,
    provider_app: &str,
    op: &str,
    entity: &str,
    data: JsonValue,
    context: OperationContext,
) -> Result<JsonValue, String> {
    let OperationContext {
        state,
        invocation,
        idempotency_key,
        origin,
    } = context;
    let (trigger_ref, timeout_ms) = match origin {
        Origin::Collection => ("cross_app_collection", TIMEOUT_INTERACTIVE_MS),
        Origin::QueryTool | Origin::DelegatedAgent => ("agent_tool", TIMEOUT_AGENT_TOOL_MS),
    };
    let authorization_kind = if origin == Origin::DelegatedAgent {
        "agent_delegation"
    } else {
        "application_grant"
    };
    let correlation_id = Uuid::new_v4();
    let action = match op {
        "find" | "list" | "findAll" | "findPage" => cross_app::ACTION_LIST,
        "findOne" => cross_app::ACTION_READ,
        "create" | "insert" | "bulk_create" => "create",
        "update" => "update",
        "delete" => "delete",
        _ => {
            let (principal, responsible_human) = state.as_ref().map_or((None, None), |st| {
                (st.audit_actor_id.or(st.user_id), st.audit_delegator_id)
            });
            if cross_app::record_read_audit(
                pool,
                None,
                consumer_app,
                provider_app,
                entity,
                "unsupported",
                principal,
                responsible_human,
                "denied",
                Some("unsupported_operation"),
                None,
                correlation_id,
                authorization_kind,
            )
            .await
            .is_err()
            {
                return Err("cross-app audit unavailable".into());
            }
            return Err("unsupported cross-app collection operation".into());
        }
    };
    let Some(st) = state else {
        if cross_app::record_read_audit(
            pool,
            None,
            consumer_app,
            provider_app,
            entity,
            action,
            None,
            None,
            "denied",
            Some("no_principal"),
            None,
            correlation_id,
            authorization_kind,
        )
        .await
        .is_err()
        {
            return Err("cross-app audit unavailable".into());
        }
        return Err("cross-app collection access denied: no active principal".into());
    };
    // QueryDataTool historically confines a missing responsible human through
    // RLS. Collection operations instead reject anonymous principals outright.
    if st.user_id.is_none() && !(origin == Origin::QueryTool && action == cross_app::ACTION_LIST) {
        if cross_app::record_read_audit(
            pool,
            None,
            consumer_app,
            provider_app,
            entity,
            action,
            None,
            st.audit_delegator_id,
            "denied",
            Some("anonymous"),
            None,
            correlation_id,
            authorization_kind,
        )
        .await
        .is_err()
        {
            return Err("cross-app audit unavailable".into());
        }
        return Err(
            "cross-app collection access denied: anonymous principals are not allowed".into(),
        );
    }
    let audit_actor = st.audit_actor_id.or(st.user_id);
    let audit_delegator = st.audit_delegator_id;
    let is_mutation = matches!(action, "create" | "update" | "delete");
    let requested_fields = if is_mutation {
        Vec::new()
    } else {
        remote_requested_fields(op, &data)
    };
    let written_fields = remote_written_fields(op, &data);
    let authorization = if origin == Origin::DelegatedAgent {
        authorize_agent_collection(pool, &st, provider_app, entity, action)
            .await
            .map(|()| None)
    } else {
        cross_app::authorize_cross_app_operation(
            pool,
            consumer_app,
            provider_app,
            entity,
            action,
            &requested_fields,
            &written_fields,
        )
        .await
        .map(Some)
    };
    let authority = match authorization {
        Ok(authority) => authority,
        Err(error) => {
            let (denial_category, public_error) = match error {
                crate::api_error::ApiError::NotFound(_) => (
                    "target_unavailable",
                    "cross-app collection is not available",
                ),
                _ => (
                    if origin == Origin::DelegatedAgent {
                        "delegated_permission"
                    } else {
                        "grant_or_scope"
                    },
                    if origin == Origin::DelegatedAgent {
                        "delegated collection permission denied"
                    } else {
                        "cross-app collection access is not authorized"
                    },
                ),
            };
            if cross_app::record_read_audit(
                pool,
                None,
                consumer_app,
                provider_app,
                entity,
                action,
                audit_actor,
                audit_delegator,
                "denied",
                Some(denial_category),
                None,
                correlation_id,
                authorization_kind,
            )
            .await
            .is_err()
            {
                return Err("cross-app audit unavailable".into());
            }
            return Err(public_error.into());
        }
    };
    if cross_app::record_read_audit(
        pool,
        authority.as_ref(),
        consumer_app,
        provider_app,
        entity,
        action,
        audit_actor,
        audit_delegator,
        "started",
        None,
        None,
        correlation_id,
        authorization_kind,
    )
    .await
    .is_err()
    {
        return Err("cross-app audit unavailable".into());
    }

    let types = match crate::manifest::field_type_map(pool, provider_app, entity).await {
        Ok(types) => types,
        Err(_) => {
            let _ = cross_app::record_read_audit(
                pool,
                authority.as_ref(),
                consumer_app,
                provider_app,
                entity,
                action,
                audit_actor,
                audit_delegator,
                "error",
                Some("target_unavailable"),
                None,
                correlation_id,
                authorization_kind,
            )
            .await;
            return Err("cross-app collection is not available".into());
        }
    };
    let access = super::enforcement::DataAccess::app(provider_app, &st, trigger_ref, timeout_ms)
        .invoked_by(&invocation)
        .audited(super::enforcement::Audit {
            actor: audit_actor,
            delegator: audit_delegator,
        })
        .across_apps(authority.as_ref());
    let access = if origin == Origin::DelegatedAgent {
        access.for_delegated_collection(entity, action, &types)
    } else {
        access
    };
    let mut tx = match access.begin(pool).await {
        Ok(tx) => tx,
        Err(_) => {
            let _ = cross_app::record_read_audit(
                pool,
                authority.as_ref(),
                consumer_app,
                provider_app,
                entity,
                action,
                audit_actor,
                audit_delegator,
                "error",
                Some("target_unavailable"),
                None,
                correlation_id,
                authorization_kind,
            )
            .await;
            return Err("cross-app collection is unavailable".into());
        }
    };
    let execution = match op {
        _ if is_mutation => {
            cross_app_mutations::execute(
                &mut tx,
                &types,
                provider_app,
                entity,
                if op == "insert" { "create" } else { op },
                data,
                idempotency_key.as_deref(),
            )
            .await
        }
        "findAll" | "findOne" => {
            let mode = match (op, origin) {
                ("findOne", _) => EqualityMode::One,
                (_, Origin::QueryTool | Origin::DelegatedAgent) => EqualityMode::NewestFirst,
                _ => EqualityMode::All,
            };
            collection_reads::equality(&mut tx, &types, provider_app, entity, data, mode).await
        }
        _ => {
            collection_reads::page(
                &mut tx,
                &types,
                provider_app,
                entity,
                data,
                op != "findPage",
            )
            .await
        }
    };
    let result = match execution {
        Ok(value) => value,
        Err(_) => {
            let _ = tx.rollback().await;
            let _ = cross_app::record_read_audit(
                pool,
                authority.as_ref(),
                consumer_app,
                provider_app,
                entity,
                action,
                audit_actor,
                audit_delegator,
                "error",
                Some("query_validation_or_execution"),
                None,
                correlation_id,
                authorization_kind,
            )
            .await;
            return Err("cross-app collection query failed".into());
        }
    };
    let row_count = if is_mutation || op == "findAll" {
        result.as_array().map_or(1, |rows| rows.len() as i64)
    } else if action == cross_app::ACTION_READ {
        i64::from(!result.is_null())
    } else {
        result
            .get("data")
            .and_then(JsonValue::as_array)
            .map(|rows| rows.len() as i64)
            .unwrap_or_else(|| if result.is_null() { 0 } else { 1 })
    };
    if is_mutation {
        // The business change and its success audit have one commit. Audit
        // failure must roll back the write, not merely hide its response.
        let audit = async {
            sqlx::query("RESET ROLE").execute(&mut *tx).await?;
            cross_app::insert_read_audit(
                &mut tx,
                authority.as_ref(),
                consumer_app,
                provider_app,
                entity,
                action,
                audit_actor,
                audit_delegator,
                "success",
                None,
                Some(row_count),
                correlation_id,
                authorization_kind,
            )
            .await
        }
        .await;
        if audit.is_err() {
            let _ = tx.rollback().await;
            let _ = cross_app::record_read_audit(
                pool,
                authority.as_ref(),
                consumer_app,
                provider_app,
                entity,
                action,
                audit_actor,
                audit_delegator,
                "error",
                Some("audit_unavailable"),
                None,
                correlation_id,
                authorization_kind,
            )
            .await;
            return Err("cross-app audit unavailable; mutation rolled back".into());
        }
    }
    if tx.commit().await.is_err() {
        let _ = cross_app::record_read_audit(
            pool,
            authority.as_ref(),
            consumer_app,
            provider_app,
            entity,
            action,
            audit_actor,
            audit_delegator,
            "error",
            Some("target_commit"),
            None,
            correlation_id,
            authorization_kind,
        )
        .await;
        return Err("cross-app collection is unavailable".into());
    }

    if !is_mutation
        && cross_app::record_read_audit(
            pool,
            authority.as_ref(),
            consumer_app,
            provider_app,
            entity,
            action,
            audit_actor,
            audit_delegator,
            "success",
            None,
            Some(row_count),
            correlation_id,
            authorization_kind,
        )
        .await
        .is_err()
    {
        return Err("cross-app audit unavailable".into());
    }

    let Some(authority) = authority else {
        return Ok(result);
    };
    Ok(if action == "delete" {
        result
    } else if is_mutation || op == "findAll" || action == cross_app::ACTION_READ {
        cross_app::project_records(result, &authority.field_snapshot)
    } else {
        cross_app::project_query_result(result, &authority.field_snapshot)
    })
}

fn remote_written_fields(op: &str, data: &JsonValue) -> Vec<String> {
    let records: Vec<&JsonValue> = match op {
        "create" | "insert" => vec![data],
        "update" => data.get("data").into_iter().collect(),
        "bulk_create" => data
            .as_array()
            .map(|rows| rows.iter().collect())
            .unwrap_or_default(),
        _ => Vec::new(),
    };
    let mut fields: Vec<String> = records
        .into_iter()
        .filter_map(JsonValue::as_object)
        .flat_map(|record| record.keys().cloned())
        .collect();
    fields.sort_unstable();
    fields.dedup();
    fields
}

fn remote_requested_fields(op: &str, data: &JsonValue) -> Vec<String> {
    let Some(object) = data.as_object() else {
        return Vec::new();
    };
    // Equality reads include columns whose names
    // happen to match list-query options.
    if matches!(op, "findOne" | "findAll") {
        return object.keys().cloned().collect();
    }
    let uses_query_options = PAGE_OPTION_KEYS.iter().any(|key| object.contains_key(*key));
    if op == "findPage" || uses_query_options {
        return cross_app::query_field_names(
            object.get("where"),
            object.get("orderBy").and_then(JsonValue::as_str),
        );
    }
    object.keys().cloned().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn remote_query_fields_ignore_pagination_controls() {
        let fields = remote_requested_fields(
            "find",
            &json!({
                "where": {"status": {"$eq": "published"}},
                "orderBy": "created_at",
                "limit": 25,
                "offset": 10,
            }),
        );
        assert_eq!(fields, vec!["created_at", "status"]);
    }

    #[test]
    fn remote_legacy_equality_queries_keep_their_field_scope() {
        let fields = remote_requested_fields("find", &json!({"status": "published"}));
        assert_eq!(fields, vec!["status"]);
    }

    #[test]
    fn remote_find_one_authorizes_every_equality_field() {
        for control in ["where", "orderBy", "order", "limit", "offset"] {
            let data = json!({control: 10, "secret": "guess"});
            for op in ["findOne", "findAll"] {
                let fields = remote_requested_fields(op, &data);
                assert!(fields.contains(&"secret".into()), "{op}/{control}");
                assert!(fields.contains(&control.into()), "{op}/{control}");
            }
        }
    }

    #[test]
    fn remote_query_options_reject_malformed_pagination_and_order() {
        let object = json!({"limit": "25"}).as_object().cloned().unwrap();
        assert_eq!(
            cross_app::parse_query_options(&object).unwrap_err(),
            "limit must be an integer"
        );

        let object = json!({"offset": -1}).as_object().cloned().unwrap();
        assert_eq!(
            cross_app::parse_query_options(&object).unwrap_err(),
            "offset must be non-negative"
        );

        let object = json!({"order": "sideways"}).as_object().cloned().unwrap();
        assert_eq!(
            cross_app::parse_query_options(&object).unwrap_err(),
            "order must be 'asc' or 'desc'"
        );
    }
}
