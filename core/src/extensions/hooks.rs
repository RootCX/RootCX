use std::collections::HashMap;

use async_trait::async_trait;
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::Value as JsonValue;
use sqlx::PgPool;
use tracing::info;

use crate::RuntimeError;
use crate::api_error::ApiError;
use crate::auth::identity::Identity;
use crate::governance::authority::{has_permission, resolve_permissions};
use crate::manifest::quote_ident;
use crate::routes::{self, SharedRuntime};

use super::RuntimeExtension;

async fn exec(pool: &PgPool, sql: &str) -> Result<(), RuntimeError> {
    sqlx::query(sql)
        .execute(pool)
        .await
        .map_err(RuntimeError::Schema)?;
    Ok(())
}

pub struct HooksExtension;

#[async_trait]
impl RuntimeExtension for HooksExtension {
    fn name(&self) -> &str {
        "hooks"
    }

    async fn bootstrap(&self, pool: &PgPool) -> Result<(), RuntimeError> {
        info!("bootstrapping hooks extension");

        // Config table — stores hook definitions
        exec(
            pool,
            r#"
            CREATE TABLE IF NOT EXISTS rootcx_system.entity_hooks (
                id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
                app_id       TEXT NOT NULL,
                entity       TEXT NOT NULL,
                operation    TEXT NOT NULL CHECK (operation IN ('INSERT', 'UPDATE', 'DELETE')),
                action_type  TEXT NOT NULL CHECK (action_type IN ('job', 'agent', 'workflow')),
                action_config JSONB NOT NULL DEFAULT '{}',
                active       BOOLEAN NOT NULL DEFAULT true,
                created_by   UUID,
                created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
                updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
            )"#,
        )
        .await?;

        exec(
            pool,
            "ALTER TABLE rootcx_system.entity_hooks ADD COLUMN IF NOT EXISTS created_by UUID",
        )
        .await?;

        exec(
            pool,
            "CREATE INDEX IF NOT EXISTS idx_hooks_lookup ON rootcx_system.entity_hooks (app_id, entity, operation) WHERE active = true",
        )
        .await?;

        // Trigger function -- checks entity_hooks config, enqueues to pgmq if match
        exec(
            pool,
            r#"
            CREATE OR REPLACE FUNCTION rootcx_system.hooks_trigger_fn()
            RETURNS TRIGGER AS $$
            DECLARE
                hook RECORD;
                rec_id TEXT;
                record_data JSONB;
                old_data JSONB;
                v_msg JSONB;
                sensitive TEXT[];
                built BOOLEAN := false;
            BEGIN
                rec_id := CASE WHEN TG_OP = 'DELETE' THEN OLD.id::TEXT ELSE NEW.id::TEXT END;

                FOR hook IN
                    SELECT id, action_type, action_config, created_by
                    FROM rootcx_system.entity_hooks
                    WHERE app_id = TG_TABLE_SCHEMA
                      AND entity = TG_TABLE_NAME
                      AND operation = TG_OP
                      AND active = true
                LOOP
                    -- Built on first match only: with no hook registered — the
                    -- case on nearly every table of every app — this trigger
                    -- does no lookup and no row serialisation at all.
                    --
                    -- The strip belongs here, at the producer: this function is
                    -- SECURITY DEFINER, so it sees the row before RLS and before
                    -- the read paths' projection, and its payload fans out into
                    -- jobs, LLM prompts and workflow node params. Stripping once
                    -- here beats stripping at each consumer.
                    IF NOT built THEN
                        SELECT COALESCE(fields, ARRAY[]::text[]) INTO sensitive
                        FROM rootcx_system.sensitive_fields
                        WHERE app_id = TG_TABLE_SCHEMA AND entity = TG_TABLE_NAME;
                        sensitive := COALESCE(sensitive, ARRAY[]::text[]);

                        record_data := CASE WHEN TG_OP IN ('INSERT', 'UPDATE') THEN to_jsonb(NEW) - sensitive END;
                        old_data := CASE WHEN TG_OP IN ('UPDATE', 'DELETE') THEN to_jsonb(OLD) - sensitive END;
                        built := true;
                    END IF;

                    v_msg := jsonb_build_object(
                        'app_id', TG_TABLE_SCHEMA,
                        'payload', jsonb_build_object(
                            '_hook', true,
                            'hook_id', hook.id,
                            'entity', TG_TABLE_NAME,
                            'operation', TG_OP,
                            'record_id', rec_id,
                            'record', record_data,
                            'old_record', old_data,
                            'action_type', hook.action_type,
                            'action_config', hook.action_config
                        )
                    );
                    IF hook.created_by IS NOT NULL THEN
                        v_msg := v_msg || jsonb_build_object('user_id', hook.created_by::text);
                    END IF;
                    PERFORM pgmq.send('jobs', v_msg);
                END LOOP;

                RETURN CASE WHEN TG_OP = 'DELETE' THEN OLD ELSE NEW END;
            END;
            $$ LANGUAGE plpgsql SECURITY DEFINER"#,
        )
        .await?;

        // Helper to enable hooks on a table
        exec(
            pool,
            r#"
            CREATE OR REPLACE FUNCTION rootcx_system.enable_hooks(target_table REGCLASS)
            RETURNS VOID AS $$
            DECLARE trigger_name TEXT;
            BEGIN
                trigger_name := regexp_replace('hooks_' || target_table::TEXT, '[^a-zA-Z0-9_]', '_', 'g');
                EXECUTE format(
                    'CREATE OR REPLACE TRIGGER %I
                     AFTER INSERT OR UPDATE OR DELETE ON %s
                     FOR EACH ROW EXECUTE FUNCTION rootcx_system.hooks_trigger_fn()',
                    trigger_name, target_table::TEXT);
            END;
            $$ LANGUAGE plpgsql"#,
        )
        .await?;

        info!("hooks extension ready");
        Ok(())
    }

    async fn on_table_created(
        &self,
        pool: &PgPool,
        manifest: &rootcx_types::AppManifest,
        schema: &str,
        table: &str,
    ) -> Result<(), RuntimeError> {
        sync_sensitive_fields(pool, manifest, schema, table).await?;
        let sql = format!(
            "SELECT rootcx_system.enable_hooks('{}.{}'::regclass)",
            quote_ident(schema),
            quote_ident(table)
        );
        exec(pool, &sql).await
    }

    async fn on_app_installed(
        &self,
        pool: &PgPool,
        manifest: &rootcx_types::AppManifest,
        installed_by: uuid::Uuid,
    ) -> Result<(), RuntimeError> {
        // Before the trigger early-return: every app needs its projection
        // reconciled, whether or not it declares a trigger.
        prune_sensitive_fields(pool, manifest).await?;

        let trigger = match &manifest.trigger {
            Some(t) => t,
            None => return Ok(()),
        };

        for operation in validate_trigger(manifest, trigger).map_err(protocol_error)? {
            let config = serde_json::json!({ "app_id": manifest.app_id });
            // `created_by` is the installer: a hook with no owner has no
            // responsible human, so `assert_can_fire` denies it at dispatch and
            // the declared trigger silently never runs.
            sqlx::query(
                r#"
                INSERT INTO rootcx_system.entity_hooks (app_id, entity, operation, action_type, action_config, created_by)
                VALUES ($1, $2, $3, 'agent', $4, $5)
                ON CONFLICT DO NOTHING
                "#,
            )
            .bind(&trigger.app_id)
            .bind(&trigger.entity)
            .bind(&operation)
            .bind(&config)
            .bind(installed_by)
            .execute(pool)
            .await
            .map_err(RuntimeError::Schema)?;

            info!(
                app_id = %manifest.app_id,
                entity = %trigger.entity,
                operation = %operation,
                "trigger hook registered from manifest"
            );
        }

        Ok(())
    }

    fn routes(&self) -> Option<Router<SharedRuntime>> {
        Some(
            Router::new()
                .route(
                    "/api/v1/apps/{app_id}/hooks",
                    get(list_hooks).post(create_hook),
                )
                .route(
                    "/api/v1/apps/{app_id}/hooks/{hook_id}",
                    get(get_hook).delete(delete_hook),
                ),
        )
    }
}

// ── API types ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct CreateHookRequest {
    entity: String,
    operation: String,
    action_type: String,
    action_config: Option<JsonValue>,
    /// Optional: own the hook as another principal (a service account),
    /// gated by the act-as guard. Survives the creator's departure.
    #[serde(rename = "runAs", default)]
    run_as: Option<String>,
}

fn protocol_error(message: String) -> RuntimeError {
    RuntimeError::Schema(sqlx::Error::Protocol(message))
}

/// Validate a manifest-declared trigger and return its normalized operations.
///
/// The `app_id` check is the security-relevant half: a manifest names the table
/// it watches and installing an app is self-service, so an unchecked value would
/// let any app plant a hook — and therefore an agent prompt — on another app's
/// rows. A trigger may only watch its own app.
fn validate_trigger(
    manifest: &rootcx_types::AppManifest,
    trigger: &rootcx_types::TriggerConfig,
) -> Result<Vec<String>, String> {
    if trigger.app_id != manifest.app_id {
        return Err(format!(
            "trigger targets app '{}' but may only target its own app '{}'",
            trigger.app_id, manifest.app_id
        ));
    }

    trigger
        .on
        .iter()
        .map(|op| {
            let operation = op.to_uppercase();
            match operation.as_str() {
                "INSERT" | "UPDATE" | "DELETE" => Ok(operation),
                _ => Err(format!("invalid trigger operation: '{op}'")),
            }
        })
        .collect()
}

/// The entity's derived row shape: which fields never leave the Core, and which
/// column owns the row. Neither is set when the entity is absent from the manifest
/// or declares neither — which is what makes the projection additive, since nothing
/// projected means every trigger and policy behaves exactly as before.
fn row_shape<'m>(
    manifest: &'m rootcx_types::AppManifest,
    table: &str,
) -> (Vec<String>, Option<&'m str>) {
    let entity = manifest.data_contract.iter().find(|e| e.entity_name == table);
    let sensitive = entity
        .into_iter()
        .flat_map(|e| &e.fields)
        .filter(|f| f.sensitive)
        .map(|f| f.name.clone())
        .collect();
    (sensitive, entity.and_then(crate::manifest::owner_field))
}

/// Project the entity's row shape into `sensitive_fields`, so the row-level
/// triggers and the retroactive RLS pass each read one indexed row instead of
/// walking `apps.manifest` JSON. Upserted so a redeploy that adds or clears either
/// declaration reconciles, mirroring how permission keys are re-synced on install.
/// An entity declaring neither is deleted rather than stored empty, keeping the
/// table proportional to what actually declares something.
async fn sync_sensitive_fields(
    pool: &PgPool,
    manifest: &rootcx_types::AppManifest,
    schema: &str,
    table: &str,
) -> Result<(), RuntimeError> {
    let (fields, owner) = row_shape(manifest, table);

    let query = if fields.is_empty() && owner.is_none() {
        sqlx::query("DELETE FROM rootcx_system.sensitive_fields WHERE app_id = $1 AND entity = $2")
            .bind(schema)
            .bind(table)
    } else {
        sqlx::query(
            "INSERT INTO rootcx_system.sensitive_fields (app_id, entity, fields, owner_field) \
             VALUES ($1, $2, $3, $4) \
             ON CONFLICT (app_id, entity) \
             DO UPDATE SET fields = EXCLUDED.fields, owner_field = EXCLUDED.owner_field",
        )
        .bind(schema)
        .bind(table)
        .bind(&fields)
        .bind(owner)
    };

    query.execute(pool).await.map_err(RuntimeError::Schema)?;
    Ok(())
}

/// Drop projections for entities the manifest no longer declares.
///
/// `sync_sensitive_fields` runs per declared entity, so it cannot see an entity
/// that was *removed* from the manifest. Left behind, the row would keep stripping
/// a column from a table that has been reshaped — or from a same-named entity
/// re-added later without the flag. Reconciled against the whole manifest, in the
/// same spirit as the permission-key re-sync on install.
async fn prune_sensitive_fields(
    pool: &PgPool,
    manifest: &rootcx_types::AppManifest,
) -> Result<(), RuntimeError> {
    let declared: Vec<String> = manifest
        .data_contract
        .iter()
        .map(|e| e.entity_name.clone())
        .collect();

    sqlx::query(
        "DELETE FROM rootcx_system.sensitive_fields \
         WHERE app_id = $1 AND entity <> ALL($2)",
    )
    .bind(&manifest.app_id)
    .bind(&declared)
    .execute(pool)
    .await
    .map_err(RuntimeError::Schema)?;
    Ok(())
}

// ── Authorization ────────────────────────────────────────────────────────

/// A hook fires with its owner's authority and receives the full row of every
/// matching write, so registering one is as privileged as reading the table it
/// watches. Mirrors `require_cron_perm`: `hook.write` to register or remove,
/// `hook.read` to inspect. Returns the caller's resolved permissions so the
/// caller can check ownership without a second lookup.
async fn require_hook_perm(
    pool: &PgPool,
    user_id: uuid::Uuid,
    app_id: &str,
    action: &str,
) -> Result<Vec<String>, ApiError> {
    let (_, perms) = resolve_permissions(pool, user_id).await?;
    if !has_permission(&perms, &format!("app:{app_id}:hook.{action}")) {
        return Err(ApiError::Forbidden(format!(
            "missing app:{app_id}:hook.{action}"
        )));
    }
    Ok(perms)
}

/// Whether the caller may reach hooks they do not own. Their payloads carry rows
/// visible to *their* owner, so this is a separate, elevated grant.
fn may_manage_others(perms: &[String], app_id: &str) -> bool {
    has_permission(perms, &format!("app:{app_id}:hook.manage_others"))
}

fn require_hook_owner(
    perms: &[String],
    app_id: &str,
    row_owner: Option<uuid::Uuid>,
    caller: uuid::Uuid,
) -> Result<(), ApiError> {
    match row_owner {
        Some(owner) if owner != caller && !may_manage_others(perms, app_id) => {
            Err(ApiError::Forbidden("not the hook owner".into()))
        }
        _ => Ok(()),
    }
}

fn hook_owner(row: &JsonValue) -> Option<uuid::Uuid> {
    row.get("created_by")?.as_str()?.parse().ok()
}

// ── Route handlers ───────────────────────────────────────────────────────

async fn list_hooks(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Vec<JsonValue>>, ApiError> {
    let pool = routes::pool(&rt);
    let perms = require_hook_perm(&pool, identity.user_id, &app_id, "read").await?;

    let entity_filter = params.get("entity");
    let op_filter = params.get("operation");

    let mut sql = "SELECT to_jsonb(h.*) AS row FROM rootcx_system.entity_hooks h WHERE app_id = $1"
        .to_string();
    let mut binds: Vec<String> = vec![app_id.clone()];

    if let Some(entity) = entity_filter {
        binds.push(entity.clone());
        sql.push_str(&format!(" AND entity = ${}", binds.len()));
    }
    if let Some(op) = op_filter {
        binds.push(op.to_uppercase());
        sql.push_str(&format!(" AND operation = ${}", binds.len()));
    }

    // Own hooks only, unless elevated. Scoped in SQL rather than filtered after
    // the fetch, so a listing can never carry a row the caller may not see.
    if !may_manage_others(&perms, &app_id) {
        binds.push(identity.user_id.to_string());
        sql.push_str(&format!(" AND created_by = ${}::uuid", binds.len()));
    }

    sql.push_str(" ORDER BY created_at DESC");

    let mut query = sqlx::query_as::<_, (JsonValue,)>(&sql);
    for b in &binds {
        query = query.bind(b);
    }
    let rows: Vec<(JsonValue,)> = query.fetch_all(&pool).await?;
    Ok(Json(rows.into_iter().map(|(r,)| r).collect()))
}

async fn create_hook(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path(app_id): Path<String>,
    Json(body): Json<CreateHookRequest>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    require_hook_perm(&pool, identity.user_id, &app_id, "write").await?;

    let operation = body.operation.to_uppercase();
    if !["INSERT", "UPDATE", "DELETE"].contains(&operation.as_str()) {
        return Err(ApiError::BadRequest(
            "operation must be INSERT, UPDATE, or DELETE".into(),
        ));
    }
    if !["job", "agent", "workflow"].contains(&body.action_type.as_str()) {
        return Err(ApiError::BadRequest(
            "action_type must be 'job', 'agent', or 'workflow'".into(),
        ));
    }

    let config = body.action_config.unwrap_or(serde_json::json!({}));

    let owner = crate::governance::delegation::act_as::resolve_owner(
        &pool,
        identity.user_id,
        body.run_as.as_deref(),
    )
    .await?;

    let (row,): (JsonValue,) = sqlx::query_as(
        r#"
        INSERT INTO rootcx_system.entity_hooks (app_id, entity, operation, action_type, action_config, created_by)
        VALUES ($1, $2, $3, $4, $5, $6)
        RETURNING to_jsonb(rootcx_system.entity_hooks.*)
        "#,
    )
    .bind(&app_id)
    .bind(&body.entity)
    .bind(&operation)
    .bind(&body.action_type)
    .bind(&config)
    .bind(owner)
    .fetch_one(&pool)
    .await?;

    // Auto-create the owner -> agent delegation for hook-triggered agents
    if body.action_type == "agent" {
        let hook_id: Option<uuid::Uuid> = row
            .get("id")
            .and_then(|v| v.as_str())
            .and_then(|s| s.parse().ok());
        let target = config
            .get("app_id")
            .and_then(|v| v.as_str())
            .unwrap_or(&app_id);
        let agent_uid = crate::extensions::agents::agent_user_id(target);
        let _ =
            crate::governance::delegation::create(&pool, owner, agent_uid, "hook", hook_id).await;
    }

    Ok(Json(row))
}

/// Load a hook and authorize the caller against its owner. `NotFound` rather
/// than `Forbidden` when the caller may not see it, so the endpoint does not
/// confirm that someone else's hook id exists.
async fn load_authorized_hook(
    pool: &PgPool,
    identity: &Identity,
    app_id: &str,
    hook_id: &str,
    action: &str,
) -> Result<JsonValue, ApiError> {
    let perms = require_hook_perm(pool, identity.user_id, app_id, action).await?;

    let row: Option<(JsonValue,)> = sqlx::query_as(
        "SELECT to_jsonb(h.*) FROM rootcx_system.entity_hooks h WHERE id = $1::uuid AND app_id = $2",
    )
    .bind(hook_id)
    .bind(app_id)
    .fetch_optional(pool)
    .await?;

    let (row,) = row.ok_or_else(|| ApiError::NotFound("hook not found".into()))?;
    require_hook_owner(&perms, app_id, hook_owner(&row), identity.user_id)
        .map_err(|_| ApiError::NotFound("hook not found".into()))?;
    Ok(row)
}

async fn get_hook(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, hook_id)): Path<(String, String)>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    let row = load_authorized_hook(&pool, &identity, &app_id, &hook_id, "read").await?;
    Ok(Json(row))
}

async fn delete_hook(
    identity: Identity,
    State(rt): State<SharedRuntime>,
    Path((app_id, hook_id)): Path<(String, String)>,
) -> Result<Json<JsonValue>, ApiError> {
    let pool = routes::pool(&rt);
    load_authorized_hook(&pool, &identity, &app_id, &hook_id, "write").await?;

    let result =
        sqlx::query("DELETE FROM rootcx_system.entity_hooks WHERE id = $1::uuid AND app_id = $2")
            .bind(&hook_id)
            .bind(&app_id)
            .execute(&pool)
            .await?;

    if result.rows_affected() == 0 {
        return Err(ApiError::NotFound("hook not found".into()));
    }

    if let Ok(uid) = hook_id.parse::<uuid::Uuid>() {
        let _ = crate::governance::delegation::revoke_by_trigger(&pool, "hook", uid).await;
    }

    Ok(Json(serde_json::json!({ "ok": true })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uid(n: u128) -> uuid::Uuid {
        uuid::Uuid::from_u128(n)
    }

    fn perms(keys: &[&str]) -> Vec<String> {
        keys.iter().map(|k| (*k).to_string()).collect()
    }

    fn manifest_with(entity: &str, fields: &[(&str, bool)]) -> rootcx_types::AppManifest {
        let entity = rootcx_types::EntityContract {
            entity_name: entity.into(),
            fields: fields
                .iter()
                .map(|(name, sensitive)| rootcx_types::FieldContract {
                    name: (*name).into(),
                    field_type: "text".into(),
                    precision: None,
                    scale: None,
                    required: false,
                    default_value: None,
                    enum_values: None,
                    references: None,
                    is_primary_key: None,
                    on_delete: None,
                    sensitive: *sensitive,
                    owner: false,
                })
                .collect(),
            identity_kind: None,
            identity_key: None,
            indexes: vec![],
            checks: vec![],
        };
        rootcx_types::AppManifest {
            app_id: "crm".into(),
            name: "CRM".into(),
            version: "1.0.0".into(),
            description: String::new(),
            icon: None,
            app_type: Default::default(),
            permissions: None,
            data_contract: vec![entity],
            actions: vec![],
            config_schema: None,
            user_auth: None,
            webhooks: vec![],
            instructions: None,
            trigger: None,
            crons: vec![],
            public: None,
        }
    }

    fn trigger(app_id: &str, on: &[&str]) -> rootcx_types::TriggerConfig {
        rootcx_types::TriggerConfig {
            app_id: app_id.into(),
            entity: "accounts".into(),
            on: on.iter().map(|o| (*o).to_string()).collect(),
        }
    }

    /// Installing an app is self-service, so a trigger naming another app's id
    /// would let any app plant a hook — and an agent prompt — on that app's
    /// rows. Operations are normalized here too, so an unknown one is refused
    /// rather than stored and silently never matched.
    #[test]
    fn validate_trigger_accepts_only_own_app_and_known_operations() {
        let manifest = manifest_with("accounts", &[("email", false)]);
        for (cfg, expected, why) in [
            (
                trigger("crm", &["insert", "UPDATE"]),
                Ok(vec!["INSERT".to_string(), "UPDATE".to_string()]),
                "own app, mixed case normalized",
            ),
            (
                trigger("other_app", &["insert"]),
                Err("may only target its own app"),
                "cross-app target is refused",
            ),
            (
                trigger("crm", &["TRUNCATE"]),
                Err("invalid trigger operation"),
                "unknown operation is refused",
            ),
            (trigger("crm", &[]), Ok(vec![]), "no operation declared"),
        ] {
            match (validate_trigger(&manifest, &cfg), expected) {
                (Ok(got), Ok(want)) => assert_eq!(got, want, "{why}"),
                (Err(got), Err(needle)) => assert!(
                    got.contains(needle),
                    "{why}: expected an error containing {needle:?}, got {got:?}"
                ),
                (got, want) => panic!("{why}: got {got:?}, wanted {want:?}"),
            }
        }
    }

    /// Feeds the projection the row-level triggers and the retroactive RLS pass
    /// read. Projecting nothing means "strip nothing, confine nobody", so the
    /// absent-entity and no-flag cases are what keep an upgrade on a pre-flag app
    /// behaviourally identical.
    #[test]
    fn row_shape_projects_only_what_the_entity_declares() {
        let m = manifest_with(
            "accounts",
            &[("email", false), ("password_hash", true), ("token", true)],
        );
        assert_eq!(
            row_shape(&m, "accounts"),
            (vec!["password_hash".to_string(), "token".to_string()], None),
            "only flagged fields, in declaration order, and no owner",
        );
        assert_eq!(
            row_shape(&m, "other_entity"),
            (vec![], None),
            "an entity absent from the manifest projects nothing",
        );
        assert_eq!(
            row_shape(&manifest_with("accounts", &[("email", false)]), "accounts"),
            (vec![], None),
            "an entity declaring neither projects nothing",
        );

        let mut owned = manifest_with("accounts", &[("user_id", false), ("token", true)]);
        owned.data_contract[0].fields[0].owner = true;
        assert_eq!(
            row_shape(&owned, "accounts"),
            (vec!["token".to_string()], Some("user_id")),
            "the two declarations are independent and travel together",
        );
    }

    /// A hook fires with its owner's authority and carries rows only that owner
    /// may see, so reaching another principal's hook needs the elevated grant.
    /// The wildcard cases matter because `app:crm:*` is what an app installer
    /// holds — it must keep working without naming the key.
    #[test]
    fn hook_ownership_is_enforced_unless_explicitly_elevated() {
        let (caller, other) = (uid(1), uid(2));
        for (owner, held, allowed, why) in [
            (Some(caller), vec![], true, "own hook needs no extra grant"),
            (Some(other), vec![], false, "another user's hook is refused"),
            (
                Some(other),
                perms(&["app:crm:hook.manage_others"]),
                true,
                "explicit elevated grant",
            ),
            (
                Some(other),
                perms(&["app:crm:*"]),
                true,
                "app wildcard covers the elevated grant",
            ),
            (Some(other), perms(&["*"]), true, "admin wildcard"),
            (
                Some(other),
                perms(&["app:other_app:hook.manage_others"]),
                false,
                "grant on a different app must not carry over",
            ),
            (
                Some(other),
                perms(&["app:crm:hook.write"]),
                false,
                "managing your own hooks does not imply managing others'",
            ),
            // A hook whose owner was cleared (the user was deleted: created_by
            // is ON DELETE SET NULL) is ownerless. It fires with no identity and
            // is already denied at dispatch, so gating it here would only strand
            // it — `hook.read`/`hook.write` remain required to reach it.
            (None, vec![], true, "ownerless hook stays reachable"),
        ] {
            let got = require_hook_owner(&held, "crm", owner, caller).is_ok();
            assert_eq!(got, allowed, "{why} (owner={owner:?}, perms={held:?})");
        }
    }

    /// `created_by` arrives as a JSON string from `to_jsonb`. A malformed or
    /// absent value must read as "no owner" rather than panic, since it feeds
    /// the authorization decision above.
    #[test]
    fn hook_owner_parses_only_a_valid_uuid() {
        let id = uid(7);
        for (row, expected, why) in [
            (
                serde_json::json!({ "created_by": id.to_string() }),
                Some(id),
                "valid uuid",
            ),
            (
                serde_json::json!({ "created_by": null }),
                None,
                "null owner",
            ),
            (serde_json::json!({}), None, "missing key"),
            (
                serde_json::json!({ "created_by": "not-a-uuid" }),
                None,
                "malformed uuid",
            ),
            (
                serde_json::json!({ "created_by": 42 }),
                None,
                "non-string value",
            ),
        ] {
            assert_eq!(hook_owner(&row), expected, "{why}: {row}");
        }
    }
}
