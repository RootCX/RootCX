//! Projection admission and historical no-share policy compatibility.
//!
//! Uses the real RBAC bootstrap entry point called by Runtime::boot (whose error
//! is propagated before workers start), without starting a second runtime against
//! the harness database.
//!
//! Mutation targets:
//! - Fall back to the manifest on invalid projection: malformed-projection test
//!   loses its visible error, despite the unchanged valid stored manifest.
//! - Return None on missing/invalid owner columns: physical-drift bootstrap
//!   succeeds or silently removes the four .own policies.
//! - Commit per-table reconciliation: physical-drift failure changes the earlier
//!   table's policy OIDs/expressions instead of rolling the app transaction back.
//! - Change no-share gate grouping, casts, WITH CHECK, or publication ceilings:
//!   pg_policies differs from the independent, literal historical SQL below.

use crate::harness::{self, TestRuntime};
use reqwest::StatusCode;
use rootcx_core::{
    RuntimeError,
    extensions::{RuntimeExtension, rbac::RbacExtension},
};
use serde_json::{Value, json};

const APP: &str = "projection_contract";

fn manifest() -> Value {
    json!({
        "appId": APP, "name": "Projection contract", "version": "1.0.0",
        "dataContract": [
            {"entityName": "a_plain", "fields": [
                {"name": "label", "type": "text"}
            ]},
            {"entityName": "z_owned", "fields": [
                {"name": "owner_id", "type": "uuid", "owner": true},
                {"name": "label", "type": "text"}
            ]}
        ]
    })
}

async fn projection(rt: &TestRuntime) -> (i32, Value) {
    sqlx::query_as(
        "SELECT version, entities FROM rootcx_system.row_access_contracts WHERE app_id=$1",
    )
    .bind(APP)
    .fetch_one(rt.pool())
    .await
    .unwrap()
}

async fn stored_manifest(rt: &TestRuntime) -> Value {
    sqlx::query_scalar("SELECT manifest FROM rootcx_system.apps WHERE id=$1")
        .bind(APP)
        .fetch_one(rt.pool())
        .await
        .unwrap()
}

/// Includes OIDs and stored expression trees, so dropping/recreating even
/// equivalent policies outside the app transaction fails the atomicity check.
async fn policy_catalog(rt: &TestRuntime) -> Value {
    sqlx::query_scalar(
        "SELECT coalesce(jsonb_agg(to_jsonb(p) ORDER BY c.relname, p.polname), '[]'::jsonb)
         FROM pg_policy p JOIN pg_class c ON c.oid=p.polrelid
         JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1",
    )
    .bind(APP)
    .fetch_one(rt.pool())
    .await
    .unwrap()
}

type Policy = (
    String,
    String,
    String,
    Vec<String>,
    String,
    Option<String>,
    Option<String>,
);

async fn policy_expressions(rt: &TestRuntime, schema: &str) -> Vec<Policy> {
    sqlx::query_as(
        "SELECT tablename::text, policyname::text, permissive::text,
                roles::text[], cmd::text, qual, with_check
         FROM pg_policies WHERE schemaname=$1 ORDER BY tablename, policyname",
    )
    .bind(schema)
    .fetch_all(rt.pool())
    .await
    .unwrap()
}

#[tokio::test]
async fn bootstrap_rejects_malformed_versioned_projection_without_manifest_fallback() {
    let rt = TestRuntime::boot().await;
    rt.install_manifest(&manifest()).await;
    let valid_manifest = stored_manifest(&rt).await;
    let (version, valid_entities) = projection(&rt).await;
    assert_eq!(version, 1, "fixture must exercise the versioned projection");
    let policies_before = policy_catalog(&rt).await;
    assert_eq!(policies_before.as_array().unwrap().len(), 22);

    let mut invalid_owner = valid_entities.clone();
    invalid_owner[1]["fields"][0]["type"] = json!("number");
    for (case, corrupt, diagnostic) in [
        (
            "not an entity list",
            json!({"entities": []}),
            "invalid projection",
        ),
        (
            "missing fields",
            json!([{"entityName": "z_owned"}]),
            "invalid projection",
        ),
        (
            "invalid owner type",
            invalid_owner,
            "owner field 'owner_id'",
        ),
    ] {
        sqlx::query("UPDATE rootcx_system.row_access_contracts SET entities=$2 WHERE app_id=$1")
            .bind(APP)
            .bind(&corrupt)
            .execute(rt.pool())
            .await
            .unwrap();

        let error = RbacExtension
            .bootstrap(rt.pool())
            .await
            .expect_err("invalid durable projection must visibly fail bootstrap");
        assert!(
            matches!(&error, RuntimeError::Invalid(_)),
            "{case}: {error}"
        );
        assert!(error.to_string().contains(diagnostic), "{case}: {error}");
        assert_eq!(
            stored_manifest(&rt).await,
            valid_manifest,
            "{case}: the successfully installed manifest remains unchanged and valid"
        );
        assert_eq!(
            projection(&rt).await,
            (1, corrupt),
            "{case}: bootstrap must not silently repair or replace the rejected projection"
        );
        assert_eq!(
            policy_catalog(&rt).await,
            policies_before,
            "{case}: failed bootstrap must preserve old policies, including their OIDs"
        );
    }
    sqlx::query("UPDATE rootcx_system.row_access_contracts SET entities=$2 WHERE app_id=$1")
        .bind(APP)
        .bind(&valid_entities)
        .execute(rt.pool())
        .await
        .unwrap();
    RbacExtension.bootstrap(rt.pool()).await.unwrap();
    rt.shutdown().await;
}

#[tokio::test]
async fn bootstrap_rejects_physical_owner_drift_and_rolls_back_earlier_tables() {
    let rt = TestRuntime::boot().await;
    rt.install_manifest(&manifest()).await;
    let valid_manifest = stored_manifest(&rt).await;
    let valid_projection = projection(&rt).await;

    for (case, wrong_type, diagnostic) in [
        ("missing", false, "is missing"),
        ("wrong type", true, "must be uuid or text, found integer"),
    ] {
        // Rename preserves every policy dependency on the original attribute.
        // DROP COLUMN CASCADE would erase the old .own policies before the test.
        sqlx::query(
            "ALTER TABLE projection_contract.z_owned
             RENAME COLUMN owner_id TO retained_owner",
        )
        .execute(rt.pool())
        .await
        .unwrap();
        if wrong_type {
            sqlx::query("ALTER TABLE projection_contract.z_owned ADD COLUMN owner_id integer")
                .execute(rt.pool())
                .await
                .unwrap();
        }
        // a_plain sorts before z_owned. A non-atomic implementation changes this
        // deliberately restrictive policy before discovering the invalid owner.
        sqlx::query("ALTER POLICY rootcx_rls_select ON projection_contract.a_plain USING (false)")
            .execute(rt.pool())
            .await
            .unwrap();
        let before = policy_catalog(&rt).await;
        assert_eq!(before.as_array().unwrap().len(), 22);

        let error = RbacExtension
            .bootstrap(rt.pool())
            .await
            .expect_err("physical owner drift must visibly fail bootstrap");
        assert!(
            matches!(&error, RuntimeError::Invalid(_)),
            "{case}: {error}"
        );
        let message = error.to_string();
        assert!(
            message.contains("owner column 'projection_contract.z_owned.owner_id'")
                && message.contains(diagnostic),
            "{case}: {message}"
        );
        assert_eq!(
            policy_catalog(&rt).await,
            before,
            "{case}: the entire app's policy reconciliation must roll back"
        );
        let own_policies: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_policies
             WHERE schemaname=$1 AND tablename='z_owned'
               AND policyname IN ('rootcx_rls_select_own','rootcx_rls_insert_own',
                                  'rootcx_rls_update_own','rootcx_rls_delete_own')",
        )
        .bind(APP)
        .fetch_one(rt.pool())
        .await
        .unwrap();
        assert_eq!(
            own_policies, 4,
            "{case}: no silent omission of .own policies"
        );
        assert_eq!(projection(&rt).await, valid_projection, "{case}");
        assert_eq!(stored_manifest(&rt).await, valid_manifest, "{case}");

        if wrong_type {
            sqlx::query("ALTER TABLE projection_contract.z_owned DROP COLUMN owner_id")
                .execute(rt.pool())
                .await
                .unwrap();
        }
        sqlx::query(
            "ALTER TABLE projection_contract.z_owned
             RENAME COLUMN retained_owner TO owner_id",
        )
        .execute(rt.pool())
        .await
        .unwrap();
        RbacExtension.bootstrap(rt.pool()).await.unwrap();
    }
    rt.shutdown().await;
}

#[tokio::test]
async fn no_share_policies_match_literal_historical_sql_across_install_redeploy_and_boot() {
    let rt = TestRuntime::boot().await;
    // These reference tables are never registered as an app or compiled by Core.
    // PostgreSQL only deparses the literal historical policy definitions, avoiding
    // a tautological comparison between two runs of the current compiler.
    sqlx::raw_sql(HISTORICAL_POLICIES)
        .execute(rt.pool())
        .await
        .unwrap();
    let expected = policy_expressions(&rt, "projection_reference").await;
    assert_eq!(
        expected.len(),
        22,
        "nine unowned and thirteen directly owned policies"
    );
    let request = manifest();
    let backend = harness::make_tar_gz(&[("index.ts", b"serve({ rpc: {} });")]);

    for stage in [
        "install",
        "reinstall",
        "redeploy",
        "bootstrap",
        "bootstrap again",
    ] {
        match stage {
            "install" | "reinstall" => rt.install_manifest(&request).await,
            "redeploy" => {
                let (status, body) = rt.deploy(APP, &backend).await;
                assert_eq!(status, StatusCode::OK, "{stage}: {body}");
            }
            "bootstrap" => {
                // Ensure boot actually reconstructs definitions; simply skipping
                // the boot pass must not satisfy a compatibility comparison.
                let names: Vec<(String, String)> = sqlx::query_as(
                    "SELECT tablename::text, policyname::text
                     FROM pg_policies WHERE schemaname=$1",
                )
                .bind(APP)
                .fetch_all(rt.pool())
                .await
                .unwrap();
                assert_eq!(names.len(), 22, "{stage}");
                for (table, name) in names {
                    // Both names originate in this test's fixed compiler fixture.
                    sqlx::query(&format!(
                        "DROP POLICY \"{name}\" ON projection_contract.\"{table}\""
                    ))
                    .execute(rt.pool())
                    .await
                    .unwrap();
                }
                RbacExtension.bootstrap(rt.pool()).await.unwrap();
            }
            "bootstrap again" => RbacExtension.bootstrap(rt.pool()).await.unwrap(),
            _ => unreachable!(),
        }
        let actual = policy_expressions(&rt, APP).await;
        assert_eq!(
            actual, expected,
            "{stage}: exact pg_policies expressions, commands, roles and permissiveness \
             must match the historical no-share generator"
        );
        assert!(
            actual.iter().all(|policy| !policy.1.ends_with("_shared")),
            "{stage}: no share declaration means no shared policy"
        );
    }
    rt.shutdown().await;
}

/// Frozen reference from the pre-row_access RbacExtension generator:
/// collection_gate, owner_predicate (direct UUID), publication_gate and the
/// RLS_POLICIES command/USING/WITH CHECK mapping. Keep this literal and independent
/// of production helpers. Formatting is intentionally left to pg_policies so the
/// bytewise assertion tests Core's output, not a PostgreSQL printer version.
const HISTORICAL_POLICIES: &str = r#"
CREATE SCHEMA projection_reference;
CREATE TABLE projection_reference.a_plain (id uuid, label text);
CREATE TABLE projection_reference.z_owned (id uuid, owner_id uuid, label text);

CREATE POLICY rootcx_rls_select ON projection_reference.a_plain FOR SELECT USING (
    (SELECT rootcx_system.check_access('app:projection_contract:a_plain.read'))
    OR (SELECT rootcx_system.check_cross_app_access('app:projection_contract:a_plain.read')));
CREATE POLICY rootcx_rls_insert ON projection_reference.a_plain FOR INSERT WITH CHECK (
    (SELECT rootcx_system.check_access('app:projection_contract:a_plain.create'))
    OR (SELECT rootcx_system.check_cross_app_access('app:projection_contract:a_plain.create')));
CREATE POLICY rootcx_rls_update ON projection_reference.a_plain FOR UPDATE USING (
    (SELECT rootcx_system.check_access('app:projection_contract:a_plain.update'))
    OR (SELECT rootcx_system.check_cross_app_access('app:projection_contract:a_plain.update')))
WITH CHECK (
    (SELECT rootcx_system.check_access('app:projection_contract:a_plain.update'))
    OR (SELECT rootcx_system.check_cross_app_access('app:projection_contract:a_plain.update')));
CREATE POLICY rootcx_rls_delete ON projection_reference.a_plain FOR DELETE USING (
    (SELECT rootcx_system.check_access('app:projection_contract:a_plain.delete'))
    OR (SELECT rootcx_system.check_cross_app_access('app:projection_contract:a_plain.delete')));
CREATE POLICY rootcx_rls_select_publication_ceiling ON projection_reference.a_plain
AS RESTRICTIVE FOR SELECT USING (
    coalesce(current_setting('rootcx.publication_id', true), '') = ''
    OR (SELECT rootcx_system.check_publication_access('app:projection_contract:a_plain.read')));
CREATE POLICY rootcx_rls_insert_publication_ceiling ON projection_reference.a_plain
AS RESTRICTIVE FOR INSERT WITH CHECK (
    coalesce(current_setting('rootcx.publication_id', true), '') = '' OR FALSE);
CREATE POLICY rootcx_rls_update_publication_ceiling ON projection_reference.a_plain
AS RESTRICTIVE FOR UPDATE
USING (coalesce(current_setting('rootcx.publication_id', true), '') = '' OR FALSE)
WITH CHECK (coalesce(current_setting('rootcx.publication_id', true), '') = '' OR FALSE);
CREATE POLICY rootcx_rls_delete_publication_ceiling ON projection_reference.a_plain
AS RESTRICTIVE FOR DELETE USING (
    coalesce(current_setting('rootcx.publication_id', true), '') = '' OR FALSE);
CREATE POLICY rootcx_rls_select_publication ON projection_reference.a_plain FOR SELECT
USING ((SELECT rootcx_system.check_publication_access('app:projection_contract:a_plain.read')));

CREATE POLICY rootcx_rls_select ON projection_reference.z_owned FOR SELECT USING (
    (SELECT rootcx_system.check_access('app:projection_contract:z_owned.read'))
    OR (((SELECT rootcx_system.check_cross_app_access('app:projection_contract:z_owned.read'))
      OR (SELECT rootcx_system.check_cross_app_access('app:projection_contract:z_owned.read.own')))
      AND owner_id = (SELECT nullif(current_setting('rootcx.user_id', true), ''))::uuid));
CREATE POLICY rootcx_rls_insert ON projection_reference.z_owned FOR INSERT WITH CHECK (
    (SELECT rootcx_system.check_access('app:projection_contract:z_owned.create'))
    OR (((SELECT rootcx_system.check_cross_app_access('app:projection_contract:z_owned.create'))
      OR (SELECT rootcx_system.check_cross_app_access('app:projection_contract:z_owned.create.own')))
      AND owner_id = (SELECT nullif(current_setting('rootcx.user_id', true), ''))::uuid));
CREATE POLICY rootcx_rls_update ON projection_reference.z_owned FOR UPDATE USING (
    (SELECT rootcx_system.check_access('app:projection_contract:z_owned.update'))
    OR (((SELECT rootcx_system.check_cross_app_access('app:projection_contract:z_owned.update'))
      OR (SELECT rootcx_system.check_cross_app_access('app:projection_contract:z_owned.update.own')))
      AND owner_id = (SELECT nullif(current_setting('rootcx.user_id', true), ''))::uuid))
WITH CHECK (
    (SELECT rootcx_system.check_access('app:projection_contract:z_owned.update'))
    OR (((SELECT rootcx_system.check_cross_app_access('app:projection_contract:z_owned.update'))
      OR (SELECT rootcx_system.check_cross_app_access('app:projection_contract:z_owned.update.own')))
      AND owner_id = (SELECT nullif(current_setting('rootcx.user_id', true), ''))::uuid));
CREATE POLICY rootcx_rls_delete ON projection_reference.z_owned FOR DELETE USING (
    (SELECT rootcx_system.check_access('app:projection_contract:z_owned.delete'))
    OR (((SELECT rootcx_system.check_cross_app_access('app:projection_contract:z_owned.delete'))
      OR (SELECT rootcx_system.check_cross_app_access('app:projection_contract:z_owned.delete.own')))
      AND owner_id = (SELECT nullif(current_setting('rootcx.user_id', true), ''))::uuid));

CREATE POLICY rootcx_rls_select_own ON projection_reference.z_owned FOR SELECT USING (
    (SELECT rootcx_system.check_access('app:projection_contract:z_owned.read.own'))
    AND owner_id = (SELECT nullif(current_setting('rootcx.user_id', true), ''))::uuid);
CREATE POLICY rootcx_rls_insert_own ON projection_reference.z_owned FOR INSERT WITH CHECK (
    (SELECT rootcx_system.check_access('app:projection_contract:z_owned.create.own'))
    AND owner_id = (SELECT nullif(current_setting('rootcx.user_id', true), ''))::uuid);
CREATE POLICY rootcx_rls_update_own ON projection_reference.z_owned FOR UPDATE USING (
    (SELECT rootcx_system.check_access('app:projection_contract:z_owned.update.own'))
    AND owner_id = (SELECT nullif(current_setting('rootcx.user_id', true), ''))::uuid)
WITH CHECK (
    (SELECT rootcx_system.check_access('app:projection_contract:z_owned.update.own'))
    AND owner_id = (SELECT nullif(current_setting('rootcx.user_id', true), ''))::uuid);
CREATE POLICY rootcx_rls_delete_own ON projection_reference.z_owned FOR DELETE USING (
    (SELECT rootcx_system.check_access('app:projection_contract:z_owned.delete.own'))
    AND owner_id = (SELECT nullif(current_setting('rootcx.user_id', true), ''))::uuid);

CREATE POLICY rootcx_rls_select_publication_ceiling ON projection_reference.z_owned
AS RESTRICTIVE FOR SELECT USING (
    coalesce(current_setting('rootcx.publication_id', true), '') = ''
    OR ((SELECT rootcx_system.check_publication_access('app:projection_contract:z_owned.read'))
        AND (current_setting('rootcx.publication_release_ownership', true) = '1'
             OR (owner_id = (SELECT nullif(current_setting('rootcx.user_id', true), ''))::uuid))));
CREATE POLICY rootcx_rls_insert_publication_ceiling ON projection_reference.z_owned
AS RESTRICTIVE FOR INSERT WITH CHECK (
    coalesce(current_setting('rootcx.publication_id', true), '') = '' OR FALSE);
CREATE POLICY rootcx_rls_update_publication_ceiling ON projection_reference.z_owned
AS RESTRICTIVE FOR UPDATE
USING (coalesce(current_setting('rootcx.publication_id', true), '') = '' OR FALSE)
WITH CHECK (coalesce(current_setting('rootcx.publication_id', true), '') = '' OR FALSE);
CREATE POLICY rootcx_rls_delete_publication_ceiling ON projection_reference.z_owned
AS RESTRICTIVE FOR DELETE USING (
    coalesce(current_setting('rootcx.publication_id', true), '') = '' OR FALSE);
CREATE POLICY rootcx_rls_select_publication ON projection_reference.z_owned FOR SELECT USING (
    (SELECT rootcx_system.check_publication_access('app:projection_contract:z_owned.read'))
    AND (current_setting('rootcx.publication_release_ownership', true) = '1'
         OR (owner_id = (SELECT nullif(current_setting('rootcx.user_id', true), ''))::uuid)));
"#;
