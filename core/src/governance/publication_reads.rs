use std::collections::HashMap;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    routing::get,
};
use serde_json::{Value, json};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use super::{
    collection_reads::{self, PAGE_OPTION_KEYS, PublicationRead},
    cross_app,
    publications::{self, ApprovedPublication, PublicExecution},
};
use crate::{api_error::ApiError, routes::SharedRuntime};

static PUBLIC_READS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(32);

pub(crate) async fn execute(
    pool: &PgPool,
    execution: &PublicExecution,
    provider_app: &str,
    op: &str,
    entity: &str,
    data: Value,
) -> Result<Value, String> {
    let _permit = PUBLIC_READS
        .try_acquire()
        .map_err(|_| "public read capacity exceeded")?;
    let (action, mode) = match op {
        "findOne" | "read" => ("read", PublicationRead::One),
        "findAll" => ("list", PublicationRead::All),
        "find" if provider_app == execution.consumer_app => ("list", PublicationRead::All),
        "find" | "list" | "findPage" => ("list", PublicationRead::Page),
        _ => return Err("publications permit collection reads only".into()),
    };
    let mut candidates = execution.publications.iter().filter(|p| {
        p.provider_app == provider_app
            && p.definition.entity == entity
            && p.definition.actions.iter().any(|a| a == action)
    });
    let publication = candidates
        .next()
        .ok_or("collection is not published for this execution")?;
    if candidates.next().is_some()
        || publication.consumer_app != execution.consumer_app
        || publication.consumer_installation_id != execution.installation_id
    {
        return Err("ambiguous or mismatched publication authority".into());
    }
    let mut audit = ReadAudit {
        execution,
        publication,
        action,
        correlation_id: Uuid::new_v4(),
        grant_id: None,
        grant_version: None,
        fields: publication.definition.fields.clone(),
    };
    audit.record_attempt(pool, "started").await?;
    let result = read(pool, &mut audit, op, mode, data).await;
    if result.is_err() {
        audit.record_attempt(pool, "denied").await?;
    }
    result
}

async fn read(
    pool: &PgPool,
    audit: &mut ReadAudit<'_>,
    op: &str,
    mode: PublicationRead,
    data: Value,
) -> Result<Value, String> {
    let publication = audit.publication;
    let definition = &publication.definition;
    let object = data
        .as_object()
        .ok_or("collection query must be an object")?;
    let empty = serde_json::Map::new();
    let empty_filter = json!({});
    let (filter, options) = match mode {
        PublicationRead::Page => {
            let has_options =
                op == "findPage" || PAGE_OPTION_KEYS.iter().any(|key| object.contains_key(*key));
            if has_options {
                if object
                    .keys()
                    .any(|key| !PAGE_OPTION_KEYS.contains(&key.as_str()))
                {
                    return Err("put collection filters under where".into());
                }
                (object.get("where").unwrap_or(&empty_filter), object)
            } else {
                (&data, &empty)
            }
        }
        _ => (&data, &empty),
    };
    let types =
        crate::manifest::field_type_map(pool, &publication.provider_app, &definition.entity)
            .await
            .map_err(|_| "published collection is unavailable")?;
    let order_by = options
        .get("orderBy")
        .map(|value| value.as_str().ok_or("orderBy must be a string"))
        .transpose()?;
    let requested_fields = cross_app::query_field_names(Some(filter), order_by);
    // A read-by-id locates a record without adding its identifier to the public
    // response. Other predicates may use only explicitly published fields.
    let id_lookup = matches!(mode, PublicationRead::One)
        && filter
            .as_object()
            .is_some_and(|o| o.len() == 1 && o.contains_key("id"));
    for field in &requested_fields {
        if !types.contains_key(field)
            || (!definition.fields.contains(field) && !(id_lookup && field == "id"))
        {
            return Err("query field is outside the publication".into());
        }
    }
    if id_lookup {
        filter["id"]
            .as_str()
            .and_then(|id| id.parse::<Uuid>().ok())
            .ok_or("record id must be a UUID")?;
    }
    let grant = if publication.consumer_app != publication.provider_app {
        let mut fields = requested_fields;
        fields.extend(cross_app::query_field_names(
            Some(&definition.where_clause),
            None,
        ));
        let grant = cross_app::authorize_cross_app_read(
            pool,
            &publication.consumer_app,
            &publication.provider_app,
            &definition.entity,
            audit.action,
            &fields,
        )
        .await
        .map_err(|_| "public cross-app read requires an active grant for this scope")?;
        if grant.consumer_installation_id != publication.consumer_installation_id
            || grant.provider_installation_id != publication.provider_installation_id
        {
            return Err("publication and grant installations differ".into());
        }
        audit
            .fields
            .retain(|field| grant.field_snapshot.contains(field));
        audit.grant_id = Some(grant.grant_id);
        audit.grant_version = Some(grant.grant_version);
        Some(grant)
    } else {
        None
    };
    if audit.fields.is_empty() {
        return Err("publication has no readable fields within the approved grant".into());
    }
    audit.fields.sort();
    let mut tx = super::enforcement::begin_publication_tx(
        pool,
        audit.execution,
        publication,
        grant.as_ref(),
    )
    .await
    .map_err(|_| "publication is no longer authorized")?;
    let result = collection_reads::published(
        &mut tx,
        &types,
        &publication.provider_app,
        &definition.entity,
        &audit.fields,
        &definition.where_clause,
        filter,
        options,
        mode,
    )
    .await
    .map_err(|_| "public collection query is invalid or exceeds its limits")?;
    let count = match mode {
        PublicationRead::One => i64::from(!result.is_null()),
        PublicationRead::All => result.as_array().map_or(0, |rows| rows.len() as i64),
        PublicationRead::Page => result["data"]
            .as_array()
            .map_or(0, |rows| rows.len() as i64),
    };
    sqlx::query("RESET ROLE")
        .execute(&mut *tx)
        .await
        .map_err(|_| "public audit unavailable")?;
    audit.record(&mut tx, "success", Some(count)).await?;
    tx.commit()
        .await
        .map_err(|_| "public read could not commit")?;
    Ok(result)
}

struct ReadAudit<'a> {
    execution: &'a PublicExecution,
    publication: &'a ApprovedPublication,
    action: &'a str,
    correlation_id: Uuid,
    grant_id: Option<Uuid>,
    grant_version: Option<i64>,
    fields: Vec<String>,
}

impl ReadAudit<'_> {
    async fn record_attempt(&self, pool: &PgPool, outcome: &str) -> Result<(), String> {
        let mut conn = pool
            .acquire()
            .await
            .map_err(|_| "public audit unavailable")?;
        self.record(&mut conn, outcome, None).await
    }

    async fn record(
        &self,
        conn: &mut PgConnection,
        outcome: &str,
        rows: Option<i64>,
    ) -> Result<(), String> {
        sqlx::query(
            "INSERT INTO rootcx_system.publication_read_audit
             (publication_id, publication_version, principal_id, consumer_app, provider_app,
              entity, action, grant_id, projection, outcome, row_count, correlation_id, grant_version)
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)",
        )
        .bind(self.publication.id)
        .bind(self.publication.version)
        .bind(self.execution.principal_id)
        .bind(&self.publication.consumer_app)
        .bind(&self.publication.provider_app)
        .bind(&self.publication.definition.entity)
        .bind(self.action)
        .bind(self.grant_id)
        .bind(&self.fields)
        .bind(outcome)
        .bind(rows)
        .bind(self.correlation_id)
        .bind(self.grant_version)
        .execute(conn)
        .await
        .map(|_| ())
        .map_err(|_| "public audit unavailable".into())
    }
}

async fn direct(
    rt: &SharedRuntime,
    app: &str,
    entity: &str,
    action: &str,
) -> Result<(PublicExecution, String, String), ApiError> {
    let manifest = crate::extensions::sharing::guard::load_manifest(rt.pool(), app)
        .await?
        .ok_or_else(|| ApiError::NotFound("publication not found".into()))?;
    let declaration = manifest
        .public
        .as_ref()
        .and_then(|p| {
            p.collections
                .iter()
                .find(|c| c.entity == entity && c.actions.iter().any(|a| a == action))
        })
        .ok_or_else(|| ApiError::NotFound("publication not found".into()))?;
    let name = declaration
        .publication
        .as_ref()
        .ok_or_else(|| ApiError::Forbidden("collection has no approved publication".into()))?;
    let execution = publications::resolve(rt.pool(), app, std::slice::from_ref(name)).await?;
    let publication = &execution.publications[0];
    let target = (
        publication.provider_app.clone(),
        publication.definition.entity.clone(),
    );
    Ok((execution, target.0, target.1))
}

async fn list(
    State(rt): State<SharedRuntime>,
    Path((app, entity)): Path<(String, String)>,
    Query(params): Query<HashMap<String, String>>,
) -> Result<Json<Value>, ApiError> {
    let (execution, provider, target) = direct(&rt, &app, &entity, "list").await?;
    let mut query = serde_json::Map::new();
    for (key, value) in params {
        let parsed = match key.as_str() {
            "where" => serde_json::from_str(&value)
                .map_err(|_| ApiError::BadRequest("where must be a JSON object".into()))?,
            "limit" | "offset" => json!(
                value
                    .parse::<i64>()
                    .map_err(|_| ApiError::BadRequest(format!("{key} must be an integer")))?
            ),
            "orderBy" | "order" => Value::String(value),
            _ => return Err(ApiError::BadRequest("unknown public query option".into())),
        };
        query.insert(key, parsed);
    }
    execute(
        rt.pool(),
        &execution,
        &provider,
        "findPage",
        &target,
        Value::Object(query),
    )
    .await
    .map(Json)
    .map_err(ApiError::Forbidden)
}

async fn get_record(
    State(rt): State<SharedRuntime>,
    Path((app, entity, id)): Path<(String, String, String)>,
) -> Result<Json<Value>, ApiError> {
    let (execution, provider, target) = direct(&rt, &app, &entity, "read").await?;
    let value = execute(
        rt.pool(),
        &execution,
        &provider,
        "read",
        &target,
        json!({"id": id}),
    )
    .await
    .map_err(ApiError::Forbidden)?;
    if value.is_null() {
        return Err(ApiError::NotFound("record not found".into()));
    }
    Ok(Json(value))
}

pub(crate) fn routes() -> Router<SharedRuntime> {
    Router::new()
        .route(
            "/api/v1/public/apps/{app_id}/collections/{entity}",
            get(list),
        )
        .route(
            "/api/v1/public/apps/{app_id}/collections/{entity}/{id}",
            get(get_record),
        )
}
