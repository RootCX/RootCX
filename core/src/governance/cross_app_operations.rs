//! One operation lifecycle for Core-bound worker and tool Adapters.
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

/// The consumer must come from Core-bound provenance, never request data.
/// Returns only the frozen projection; SQL errors stay in Core audit categories.
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
        Origin::QueryTool => ("agent_tool", TIMEOUT_AGENT_TOOL_MS),
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
    let authority = match cross_app::authorize_cross_app_operation(
        pool,
        consumer_app,
        provider_app,
        entity,
        action,
        &requested_fields,
        &written_fields,
    )
    .await
    {
        Ok(authority) => authority,
        Err(error) => {
            let (denial_category, public_error) = match error {
                crate::api_error::ApiError::NotFound(_) => (
                    "target_unavailable",
                    "cross-app collection is not available",
                ),
                _ => (
                    "grant_or_scope",
                    "cross-app collection access is not authorized",
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
        Some(&authority),
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
                Some(&authority),
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
            )
            .await;
            return Err("cross-app collection is not available".into());
        }
    };
    let mut tx = match super::enforcement::begin_app_tx_with_invocation_and_cross_app(
        pool,
        provider_app,
        &st,
        &invocation,
        audit_actor,
        audit_delegator,
        trigger_ref,
        timeout_ms,
        Some(&authority),
    )
    .await
    {
        Ok(tx) => tx,
        Err(_) => {
            let _ = cross_app::record_read_audit(
                pool,
                Some(&authority),
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
                (_, Origin::QueryTool) => EqualityMode::NewestFirst,
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
                Some(&authority),
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
                Some(&authority),
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
            )
            .await
        }
        .await;
        if audit.is_err() {
            let _ = tx.rollback().await;
            let _ = cross_app::record_read_audit(
                pool,
                Some(&authority),
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
            )
            .await;
            return Err("cross-app audit unavailable; mutation rolled back".into());
        }
    }
    if tx.commit().await.is_err() {
        let _ = cross_app::record_read_audit(
            pool,
            Some(&authority),
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
        )
        .await;
        return Err("cross-app collection is unavailable".into());
    }

    if !is_mutation
        && cross_app::record_read_audit(
            pool,
            Some(&authority),
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
        )
        .await
        .is_err()
    {
        return Err("cross-app audit unavailable".into());
    }

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
