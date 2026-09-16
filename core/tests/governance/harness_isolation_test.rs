use crate::harness::TestRuntime;

#[tokio::test]
async fn fresh_fixture_preserves_extensions_but_never_previous_test_data() {
    let first = TestRuntime::boot().await;
    sqlx::query("CREATE TABLE public.previous_test_secret (value text)")
        .execute(first.pool()).await.unwrap();
    sqlx::query("INSERT INTO public.previous_test_secret VALUES ('must disappear')")
        .execute(first.pool()).await.unwrap();
    first.shutdown().await;

    let second = TestRuntime::boot().await;
    let leaked: bool = sqlx::query_scalar(
        "SELECT to_regclass('public.previous_test_secret') IS NOT NULL",
    ).fetch_one(second.pool()).await.unwrap();
    let extensions: Vec<String> = sqlx::query_scalar(
        "SELECT extname FROM pg_extension WHERE extname IN ('pgmq', 'pg_cron') ORDER BY extname",
    ).fetch_all(second.pool()).await.unwrap();
    second.shutdown().await;
    assert!(!leaked, "a fresh Core fixture must not inherit another test's data");
    assert_eq!(extensions, ["pg_cron", "pgmq"], "the real extensions must survive database recreation");
}
