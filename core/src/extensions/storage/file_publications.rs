use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::put,
};
use serde::Deserialize;
use sqlx::{Executor, PgPool, Postgres, Transaction};
use uuid::Uuid;

use super::public_download::detect_media_type;
use crate::{RuntimeError, api_error::ApiError, auth::identity::Identity, routes::SharedRuntime};

pub(super) async fn bootstrap(pool: &PgPool) -> Result<(), RuntimeError> {
    let mut tx = pool.begin().await.map_err(RuntimeError::Schema)?;
    tx.execute(include_str!("file_publications.sql"))
        .await
        .map_err(RuntimeError::Schema)?;
    tx.commit().await.map_err(RuntimeError::Schema)
}

pub(super) fn routes() -> Router<SharedRuntime> {
    Router::new()
        .route(
            "/api/v1/apps/{app_id}/file-channels/{channel}",
            put(enable_channel)
                .get(channel_status)
                .delete(disable_channel),
        )
        .route(
            "/api/v1/apps/{app_id}/file-publications/{channel}/{key}",
            put(publish).delete(withdraw),
        )
}

// Bound both the public URL space and permission names. Dots are only meaningful in file keys.
fn validate_segment(value: &str, file_key: bool) -> Result<(), ApiError> {
    let max_length = if file_key { 180 } else { 48 };
    let valid_start = value
        .bytes()
        .next()
        .is_some_and(|b| b.is_ascii_lowercase() || (file_key && b.is_ascii_digit()));
    let valid_chars = value.bytes().all(|b| {
        b.is_ascii_lowercase()
            || b.is_ascii_digit()
            || matches!(b, b'_' | b'-')
            || (file_key && b == b'.')
    });
    if !valid_start || !valid_chars || value.len() > max_length {
        return Err(ApiError::BadRequest("invalid publication name".into()));
    }
    Ok(())
}

enum Authority {
    Approver,
    Publisher,
}

struct Change {
    tx: Transaction<'static, Postgres>,
    installation: Uuid,
    actor: Uuid,
}

impl Change {
    async fn begin(
        rt: &SharedRuntime,
        actor: Uuid,
        app: &str,
        channel: &str,
        authority: Authority,
    ) -> Result<Self, ApiError> {
        validate_segment(channel, false)?;
        let mut tx = rt.pool().begin().await?;
        let installation = sqlx::query_scalar(
            "SELECT id FROM rootcx_system.app_installations WHERE app_id=$1 AND active FOR SHARE",
        )
        .bind(app)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| ApiError::NotFound("installation not found".into()))?;
        match authority {
            Authority::Approver => {
                if !crate::governance::publications::can_manage_tx(&mut tx, actor, app).await? {
                    return Err(ApiError::Forbidden(
                        "file channel approval permission required".into(),
                    ));
                }
            }
            Authority::Publisher => {
                let allowed: bool = sqlx::query_scalar(
                    "SELECT rootcx_system.has_permission($1,$2) AND rootcx_system.has_permission($1,$3)"
                )
                .bind(actor)
                .bind(format!("app:{app}:storage.write"))
                .bind(format!("app:{app}:storage.publish.{channel}"))
                .fetch_one(&mut *tx).await?;
                if !allowed {
                    return Err(ApiError::Forbidden(
                        "file publication permission required".into(),
                    ));
                }
                let active = sqlx::query("SELECT 1 FROM rootcx_system.file_channels WHERE installation_id=$1 AND channel=$2 FOR SHARE")
                    .bind(installation).bind(channel).fetch_optional(&mut *tx).await?;
                if active.is_none() {
                    return Err(ApiError::Forbidden(
                        "file channel has not been approved".into(),
                    ));
                }
            }
        }
        Ok(Self {
            tx,
            installation,
            actor,
        })
    }

    async fn finish(
        mut self,
        operation: &str,
        channel: &str,
        key: Option<&str>,
        file: Option<Uuid>,
    ) -> Result<StatusCode, ApiError> {
        sqlx::query("INSERT INTO rootcx_system.audit_log(table_schema,table_name,record_id,operation,new_record,actor_uid)
                     VALUES ('rootcx_system','file_publications',$1,$2,jsonb_build_object('channel',$3::text,'key',$4::text,'fileId',$5::uuid),$6)")
            .bind(self.installation.to_string()).bind(operation).bind(channel).bind(key).bind(file).bind(self.actor)
            .execute(&mut *self.tx).await?;
        self.tx.commit().await?;
        Ok(StatusCode::NO_CONTENT)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Approval {
    installation_id: Uuid,
}

async fn channel_status(
    State(rt): State<SharedRuntime>,
    identity: Identity,
    Path((app, channel)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let mut change =
        Change::begin(&rt, identity.user_id, &app, &channel, Authority::Approver).await?;
    let enabled: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM rootcx_system.file_channels WHERE installation_id=$1 AND channel=$2)")
        .bind(change.installation).bind(channel).fetch_one(&mut *change.tx).await?;
    change.tx.commit().await?;
    Ok(Json(
        serde_json::json!({"installationId": change.installation, "enabled": enabled}),
    ))
}

async fn enable_channel(
    State(rt): State<SharedRuntime>,
    identity: Identity,
    Path((app, channel)): Path<(String, String)>,
    Json(approval): Json<Approval>,
) -> Result<StatusCode, ApiError> {
    let mut change =
        Change::begin(&rt, identity.user_id, &app, &channel, Authority::Approver).await?;
    if change.installation != approval.installation_id {
        return Err(ApiError::Conflict(
            "reviewed installation has changed".into(),
        ));
    }
    sqlx::query("INSERT INTO rootcx_system.file_channels (installation_id, channel) VALUES ($1,$2) ON CONFLICT DO NOTHING")
        .bind(change.installation).bind(&channel).execute(&mut *change.tx).await?;
    sqlx::query("INSERT INTO rootcx_system.rbac_permissions (key,description,source_app) VALUES ($1,'Publish and withdraw app files in this channel',$2) ON CONFLICT DO NOTHING")
        .bind(format!("app:{app}:storage.publish.{channel}")).bind(app).execute(&mut *change.tx).await?;
    change.finish("ENABLE", &channel, None, None).await
}

async fn disable_channel(
    State(rt): State<SharedRuntime>,
    identity: Identity,
    Path((app, channel)): Path<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let mut change =
        Change::begin(&rt, identity.user_id, &app, &channel, Authority::Approver).await?;
    sqlx::query("DELETE FROM rootcx_system.file_channels WHERE installation_id=$1 AND channel=$2")
        .bind(change.installation)
        .bind(&channel)
        .execute(&mut *change.tx)
        .await?;
    change.finish("DISABLE", &channel, None, None).await
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Publish {
    file_id: Uuid,
}

async fn publish(
    State(rt): State<SharedRuntime>,
    identity: Identity,
    Path((app, channel, key)): Path<(String, String, String)>,
    Json(input): Json<Publish>,
) -> Result<StatusCode, ApiError> {
    validate_segment(&key, true)?;
    let mut change =
        Change::begin(&rt, identity.user_id, &app, &channel, Authority::Publisher).await?;
    // Read only a bounded prefix of the immutable object, regardless of its upload size.
    let prefix: Vec<u8> = sqlx::query_scalar("SELECT CASE WHEN f.content IS NOT NULL THEN substring(f.content FROM 1 FOR 512)
        ELSE lo_get(f.content_oid,0,512) END FROM rootcx_system.files f JOIN rootcx_system.app_installations i
        ON i.app_id=f.app_id AND f.created_at>=i.installed_at WHERE i.id=$1 AND f.id=$2 AND f.uploaded_by=$3 FOR KEY SHARE OF f")
        .bind(change.installation).bind(input.file_id).bind(identity.user_id).fetch_optional(&mut *change.tx).await?
        .ok_or_else(|| ApiError::NotFound("publishable upload not found".into()))?;
    let media_type = detect_media_type(&prefix)
        .ok_or_else(|| ApiError::BadRequest("unsupported public file format".into()))?;
    sqlx::query("INSERT INTO rootcx_system.file_publications(installation_id,channel,key,file_id,media_type)
        VALUES ($1,$2,$3,$4,$5) ON CONFLICT (installation_id,channel,key) DO UPDATE SET file_id=EXCLUDED.file_id, media_type=EXCLUDED.media_type")
        .bind(change.installation).bind(&channel).bind(&key).bind(input.file_id).bind(media_type).execute(&mut *change.tx).await?;
    change
        .finish("PUBLISH", &channel, Some(&key), Some(input.file_id))
        .await
}

async fn withdraw(
    State(rt): State<SharedRuntime>,
    identity: Identity,
    Path((app, channel, key)): Path<(String, String, String)>,
) -> Result<StatusCode, ApiError> {
    validate_segment(&key, true)?;
    let mut change =
        Change::begin(&rt, identity.user_id, &app, &channel, Authority::Publisher).await?;
    let file = sqlx::query_scalar("DELETE FROM rootcx_system.file_publications WHERE installation_id=$1 AND channel=$2 AND key=$3 RETURNING file_id")
        .bind(change.installation).bind(&channel).bind(&key).fetch_optional(&mut *change.tx).await?;
    change.finish("WITHDRAW", &channel, Some(&key), file).await
}
