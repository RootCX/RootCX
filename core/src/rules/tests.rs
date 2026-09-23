use rootcx_types::EntityContract;
use serde_json::json;

use super::*;

fn entity(extra: serde_json::Value) -> EntityContract {
    let mut value = json!({
        "entityName": "sample",
        "fields": [
            {"name": "qty", "type": "decimal", "precision": 12, "scale": 2},
            {"name": "price", "type": "decimal"},
            {"name": "n", "type": "number"},
            {"name": "year", "type": "number"},
            {"name": "name", "type": "text"},
            {"name": "email", "type": "text"},
            {"name": "status", "type": "text"},
            {"name": "flag", "type": "boolean"},
            {"name": "d1", "type": "date"},
            {"name": "d2", "type": "date"},
            {"name": "t1", "type": "timestamp"},
            {"name": "t2", "type": "timestamp"},
            {"name": "meta", "type": "json"},
            {"name": "ref_id", "type": "uuid"},
            {"name": "tags", "type": "[text]"},
            {"name": "secret", "type": "text", "sensitive": true}
        ]
    });
    value.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
    serde_json::from_value(value).unwrap()
}

fn compile_check(expr: &str) -> Result<CompiledCheck, String> {
    let rules = compile_entity(&entity(json!({"checks": [{"name": "rule", "expr": expr}]})))?;
    Ok(rules.checks.into_iter().find(|c| c.name == "rule").expect("declared check"))
}

fn sql_of(expr: &str) -> String {
    compile_check(expr).unwrap_or_else(|e| panic!("{expr}: {e}")).sql
}

fn refusal(expr: &str) -> String {
    match compile_check(expr) {
        Ok(check) => panic!("{expr:?} must be refused, compiled to {}", check.sql),
        Err(error) => error,
    }
}

/// Emitted SQL for representative rules, including every construct Kova uses.
pub(super) const GOLDENS: &[(&str, &str)] = &[
    ("qty > 0 AND scale(qty) <= 2", r#"(("qty" > 0) AND (pg_catalog.scale("qty") <= 2))"#),
    ("NOT flag = true", r#"(NOT ("flag" = TRUE))"#),
    ("status IN ('a', 'b''c')", r#"("status" IN ('a', 'b''c'))"#),
    ("status NOT IN ('failed', 'deleting')", r#"("status" NOT IN ('failed', 'deleting'))"#),
    ("year BETWEEN 2000 AND 9998 AND year = trunc(year)",
     r#"(("year" BETWEEN 2000 AND 9998) AND ("year" = pg_catalog.trunc("year")))"#),
    ("(status = 'refused') = (name IS NOT NULL AND length(btrim(name)) > 0)",
     r#"(("status" = 'refused') = (("name" IS NOT NULL) AND (pg_catalog.length(pg_catalog.btrim("name")) > 0)))"#),
    ("t2 - t1 <= interval '62 days'", r#"(("t2" - "t1") <= '62 days'::pg_catalog.interval)"#),
    ("d2 > d1 AND d1 >= '2026-01-31'", r#"(("d2" > "d1") AND ("d1" >= '2026-01-31'::pg_catalog.date))"#),
    ("t1 >= '2026-01-01T01:00:00+01:00'", r#"("t1" >= '2026-01-01T00:00:00Z'::pg_catalog.timestamptz)"#),
    ("email ~ '^[^[:space:]@]+@[^[:space:]@]+\\.[^[:space:]@]+$'",
     r#"("email" ~ '^[^[:space:]@]+@[^[:space:]@]+\.[^[:space:]@]+$')"#),
    ("jsonb_typeof(meta) = 'array' AND jsonb_array_length(meta) <= 5",
     r#"((pg_catalog.jsonb_typeof("meta") = 'array') AND ((CASE WHEN pg_catalog.jsonb_typeof("meta") = 'array' THEN pg_catalog.jsonb_array_length("meta") END) <= 5))"#),
    ("length(btrim(coalesce(name, ''))) > 0", r#"(pg_catalog.length(pg_catalog.btrim(COALESCE("name", ''))) > 0)"#),
    ("length(trim(name)) > 0", r#"(pg_catalog.length(pg_catalog.btrim("name")) > 0)"#),
    ("n = qty * 2 + price", r#"("n" = (("qty" * 2) + "price"))"#),
    ("n>=-1 AND -n < 5", r#"(("n" >= (- 1)) AND ((- "n") < 5))"#),
    ("ref_id = '6F1C2A3B-0000-4000-8000-00000000000A'", r#"("ref_id" = '6f1c2a3b-0000-4000-8000-00000000000a'::pg_catalog.uuid)"#),
    ("abs(n) > 30 OR n IS NULL", r#"((pg_catalog.abs("n") > 30) OR ("n" IS NULL))"#),
    ("upper(name) = name", r#"(pg_catalog.upper("name") = "name")"#),
    ("tags IS NOT NULL", r#"("tags" IS NOT NULL)"#),
];

#[test]
fn goldens_compile_to_exact_sql() {
    for (source, expected) in GOLDENS.iter() {
        assert_eq!(&sql_of(source), expected, "{source}");
    }
}

#[test]
fn precedence_matches_postgresql() {
    for (source, expected) in [
        ("NOT flag AND flag OR flag", r#"(((NOT "flag") AND "flag") OR "flag")"#),
        ("flag OR flag AND NOT flag", r#"("flag" OR ("flag" AND (NOT "flag")))"#),
        ("NOT name IS NULL", r#"(NOT ("name" IS NULL))"#),
        ("n = 1 IS NOT NULL", r#"(("n" = 1) IS NOT NULL)"#),
        ("n + 1 * 2 - 3 = 4", r#"((("n" + (1 * 2)) - 3) = 4)"#),
        ("n BETWEEN 1 AND 2 AND flag", r#"(("n" BETWEEN 1 AND 2) AND "flag")"#),
        ("n IN (1, 2) = flag", r#"(("n" IN (1, 2)) = "flag")"#),
        ("- n * 2 > 1", r#"(((- "n") * 2) > 1)"#),
        ("NOT NOT flag", r#"(NOT (NOT "flag"))"#),
    ] {
        assert_eq!(sql_of(source), expected, "{source}");
    }
}

#[test]
fn trim_and_bang_equal_are_canonical_spellings() {
    let a = compile_check("length(trim(name)) > 0 AND n != 1").unwrap();
    let b = compile_check("length(btrim(name)) > 0 AND n <> 1").unwrap();
    assert_eq!(a.sql, b.sql);
    assert_eq!(a.tag, b.tag, "equivalent spellings share one tag");
    assert!(a.tag.starts_with("r1-") && a.tag.len() == 19, "{}", a.tag);
    assert_ne!(a.tag, compile_check("length(btrim(name)) > 1 AND n <> 1").unwrap().tag);
}

#[test]
fn a_column_type_change_changes_the_tag() {
    let narrow = compile_entity(&entity(json!({"checks": [{"name": "r", "expr": "qty > 0"}]}))).unwrap();
    let mut wide = entity(json!({"checks": [{"name": "r", "expr": "qty > 0"}]}));
    wide.fields[0].precision = Some(14);
    let wide = compile_entity(&wide).unwrap();
    assert_ne!(narrow.checks[0].tag, wide.checks[0].tag);
}

#[test]
fn legacy_tags_match_what_older_cores_wrote() {
    let check = compile_check("qty   >  0").unwrap();
    assert_eq!(check.legacy_tag.as_deref(), Some(legacy_check_tag("qty > 0").as_str()));
    let rules = compile_entity(&entity(json!({"fields": [
        {"name": "status", "type": "text", "enum_values": ["a", "b"]}
    ]}))).unwrap();
    let status = rules.checks.iter().find(|c| c.name == "chk_sample_status").unwrap();
    assert_eq!(status.legacy_tag.as_deref(), Some(legacy_check_tag(r#""status" IN ('a', 'b')"#).as_str()));
    assert_eq!(status.sql, r#"("status" IN ('a', 'b'))"#);
}

/// Every refusal names what is wrong. Each entry is an attack or a construct
/// PostgreSQL could read differently from Core.
#[test]
fn adversarial_corpus_is_refused() {
    let nested = format!("{}flag{}", "(".repeat(1_000), ")".repeat(1_000));
    let huge = format!("n > {}", "1".repeat(1 << 20));
    let many = format!("n IN ({})", (0..101).map(|i| i.to_string()).collect::<Vec<_>>().join(", "));
    let long_string = format!("name = '{}'", "x".repeat(300));
    let cases: Vec<(String, &str)> = vec![
        ("true), NO FORCE ROW LEVEL SECURITY, DISABLE ROW LEVEL SECURITY, ADD CONSTRAINT zz CHECK (true".into(), "unexpected"),
        ("pg_read_file('/etc/passwd') IS NOT NULL".into(), "function 'pg_read_file' is not allowed"),
        ("query_to_xml('select 1', true, true, '') IS NOT NULL".into(), "function 'query_to_xml' is not allowed"),
        ("set_config('role', 'x', true) IS NULL".into(), "function 'set_config' is not allowed"),
        ("n::text = '1'".into(), "casts are not allowed"),
        ("CAST(n AS text) = '1'".into(), "not part of the rule language"),
        ("name COLLATE \"C\" = 'a'".into(), "not part of the rule language"),
        ("(SELECT 1) = 1".into(), "not part of the rule language"),
        ("EXISTS (SELECT 1)".into(), "not part of the rule language"),
        ("n = ANY (ARRAY[1])".into(), "not part of the rule language"),
        ("t1 < now()".into(), "function 'now' is not allowed"),
        ("t1 < CURRENT_TIMESTAMP".into(), "not part of the rule language"),
        ("name = current_user".into(), "not part of the rule language"),
        ("d1 > 'today'".into(), "is not a date"),
        ("t1 > 'now'".into(), "is not a timestamp"),
        ("t1 > '2026-01-01 00:00:00'".into(), "is not a timestamp"),
        ("d1 > 'infinity'".into(), "is not a date"),
        ("n > 'NaN'".into(), "cannot compare number with text"),
        ("qty > 'Infinity'".into(), "cannot compare decimal with text"),
        ("d1 < t1".into(), "cannot compare date with timestamp"),
        ("n / 0 > 1".into(), "operator '/' is not allowed"),
        ("n % 2 = 0".into(), "operator '%' is not allowed"),
        ("n ^ 2 = 0".into(), "operator '^' is not allowed"),
        ("name ~ '(a+)+$'".into(), "quantifier"),
        ("name ~ '(a)\\1'".into(), "pattern"),
        ("name ~ '(?=a)'".into(), "pattern"),
        ("name ~ '***=a'".into(), "directors"),
        ("name ~ '(?i)a'".into(), "inline flags"),
        ("name ~ 'a{1,1000}'".into(), "limited to 100"),
        ("name ~ '\\ba'".into(), "anchors"),
        ("name ~ name".into(), "quoted pattern"),
        ("\"name\" = 'a'".into(), "quoted identifiers"),
        ("name = 'a' -- comment".into(), "comments"),
        ("name = 'a' /* c */".into(), "comments"),
        ("ctid IS NULL".into(), "unknown field 'ctid'"),
        ("xmin IS NULL".into(), "unknown field 'xmin'"),
        ("tableoid IS NULL".into(), "unknown field 'tableoid'"),
        ("kova_erp.f(name) IS NULL".into(), "qualified names"),
        ("pg_catalog.length(name) > 0".into(), "qualified names"),
        ("name = E'a'".into(), "prefixed string"),
        ("name = U&'a'".into(), "lowercase"),
        ("name = $$a$$".into(), "'$' is not allowed"),
        ("n > 1e5".into(), "digits"),
        ("n > 0x10".into(), "digits"),
        ("n > 1_000".into(), "digits"),
        ("n > .5".into(), "'.'"),
        ("flag = flag = flag".into(), "do not chain"),
        ("secret = name".into(), "sensitive"),
        ("name > 'a'".into(), "collation"),
        ("name = 'a';DROP TABLE x".into(), "';' is not allowed"),
        (nested, "nesting depth"),
        (huge, "limited to 2048 bytes"),
        (many, "limited to 100 items"),
        (long_string, "limited to 256 bytes"),
        ("name = 'a\0b'".into(), "NUL"),
        ("n\u{0430}me = 'a'".into(), "only ASCII"),
        ("NaN > n".into(), "lowercase"),
        ("'a' = 'b'".into(), "cannot compare text with text"),
        ("name = 'a' 'b'".into(), "unexpected a string"),
        ("n = +1".into(), "unary '+'"),
        ("flag IS TRUE".into(), "only IS [NOT] NULL"),
        ("n BETWEEN SYMMETRIC 1 AND 2".into(), "not part of the rule language"),
        ("name ILIKE 'a'".into(), "not part of the rule language"),
        ("name LIKE 'a'".into(), "not part of the rule language"),
        ("n = NULL".into(), "IS [NOT] NULL"),
        ("n".into(), "must be a boolean"),
        ("".into(), "empty"),
        ("meta = meta".into(), "cannot compare json"),
        ("tags = tags".into(), "cannot compare array"),
        ("scale(n) <= 2".into(), "decimal field"),
        ("t1 + interval '1 days' > t2".into(), "cannot combine"),
        ("n IN (n, 1)".into(), "literals only"),
        ("coalesce('a', name) = 'a'".into(), "starts with"),
        ("length(name, 1) > 0".into(), "argument"),
        ("status = 'a' OR 1".into(), "needs booleans"),
        ("interval '1 month' > t2 - t1".into(), "intervals are written"),
        ("n = 1 AND".into(), "expected a field"),
    ];
    for (source, fragment) in &cases {
        let error = refusal(source);
        let shown: String = source.chars().take(120).collect();
        assert!(error.contains(fragment), "{shown:?}: expected '{fragment}' in: {error}");
    }
}

#[test]
fn errors_point_at_the_line_and_column() {
    let error = refusal("n > 1 AND\n  pg_read_file('x') IS NULL");
    assert!(error.starts_with("checks[rule].expr 2:3: function 'pg_read_file'"), "{error}");
}

#[test]
fn sensitive_fields_stay_in_single_field_rules() {
    assert!(compile_check("length(secret) > 8").is_ok());
    assert!(refusal("secret = name").contains("single-field"));
    let error = compile_entity(&entity(json!({"indexes": [
        {"name": "ix", "columns": ["name"], "where": "secret IS NOT NULL"}
    ]}))).unwrap_err();
    assert!(error.contains("index predicates and expressions cannot use it"), "{error}");
}

#[test]
fn names_are_required_bounded_and_unique() {
    let unnamed = compile_entity(&entity(json!({"checks": [{"expr": "n > 0"}]}))).unwrap_err();
    assert!(unnamed.contains("needs a 'name'"), "{unnamed}");
    let long = compile_entity(&entity(json!({"checks": [{"name": "c".repeat(64), "expr": "n > 0"}]}))).unwrap_err();
    assert!(long.contains("limited to 63"), "{long}");
    let dup = compile_entity(&entity(json!({"checks": [
        {"name": "c", "expr": "n > 0"}, {"name": "c", "expr": "n > 1"}
    ]}))).unwrap_err();
    assert!(dup.contains("duplicate"), "{dup}");
}

#[test]
fn indexes_compile_predicates_expressions_and_trigram() {
    let rules = compile_entity(&entity(json!({"indexes": [
        {"name": "uq_ref", "columns": ["ref_id", "status"], "unique": true, "where": "status <> 'cancelled'"},
        {"name": "uq_name", "columns": [{"expr": "lower(btrim(name))"}], "unique": true},
        {"name": "ix_trgm", "using": "trigram", "columns": ["name", "email"]},
        {"name": "ix_sorted", "columns": ["status", {"column": "t1", "sort": "desc", "nulls": "last"}], "where": "flag = true"}
    ]}))).unwrap();
    let sql: Vec<String> = rules.indexes.iter().map(|i| i.create_sql("app", "sample", &i.name, false)).collect();
    assert_eq!(sql[0], r#"CREATE UNIQUE INDEX "uq_ref" ON "app"."sample" USING btree ("ref_id", "status") WHERE ("status" <> 'cancelled')"#);
    assert_eq!(sql[1], r#"CREATE UNIQUE INDEX "uq_name" ON "app"."sample" USING btree ((pg_catalog.lower(pg_catalog.btrim("name"))))"#);
    assert_eq!(sql[2], r#"CREATE INDEX "ix_trgm" ON "app"."sample" USING gin ("name" rootcx_ext.gin_trgm_ops, "email" rootcx_ext.gin_trgm_ops)"#);
    assert_eq!(sql[3], r#"CREATE INDEX "ix_sorted" ON "app"."sample" USING btree ("status", "t1" DESC NULLS LAST) WHERE ("flag" = TRUE)"#);
    assert_eq!(rules.indexes[2].columns, ["name", "email"]);
    assert_eq!(
        rules.indexes[1].create_sql("app", "sample", "next", true),
        r#"CREATE UNIQUE INDEX CONCURRENTLY "next" ON "app"."sample" USING btree ((pg_catalog.lower(pg_catalog.btrim("name"))))"#,
    );
}

#[test]
fn raw_index_sql_stays_refused() {
    for (index, fragment) in [
        (json!({"columns": [{"column": "name", "ops": "kova_erp.gin_trgm_ops"}], "using": "gin"}), "\"using\": \"trigram\""),
        (json!({"columns": ["name"], "with": {"fillfactor": "70"}}), "storage parameters"),
        (json!({"columns": ["name"], "where": "true) WITH (fillfactor=10"}), "unexpected"),
        (json!({"columns": [{"expr": "pg_read_file(name)"}]}), "not allowed"),
        (json!({"columns": [{"expr": "1"}]}), "at least one field"),
        (json!({"columns": ["undeclared"]}), "not a declared or system field"),
        (json!({"columns": []}), "no columns"),
        (json!({"columns": ["n"], "using": "trigram"}), "must be text"),
        (json!({"columns": ["name"], "using": "trigram", "unique": true}), "cannot be unique"),
        (json!({"columns": ["name"], "using": "bogus"}), "unknown index method"),
        (json!({"columns": [{"column": "name", "sort": "sideways"}]}), "invalid sort"),
        (json!({"columns": ["name"], "where": "n"}), "boolean"),
        (json!({"name": "Bad", "columns": ["name"]}), "snake_case"),
    ] {
        let error = compile_entity(&entity(json!({"indexes": [index]}))).unwrap_err();
        assert!(error.contains(fragment), "{index}: {error}");
    }
}

#[test]
fn enum_checks_keep_legacy_names_and_fit_long_ones() {
    let long_field = format!("f_{}", "x".repeat(60));
    let rules = compile_entity(&serde_json::from_value(json!({
        "entityName": "entity_with_a_fairly_long_name",
        "fields": [
            {"name": "status", "type": "text", "enum_values": ["a"]},
            {"name": "tags", "type": "[text]", "enum_values": ["a", "b"]},
            {"name": long_field, "type": "text", "enum_values": ["x"]}
        ]
    })).unwrap()).unwrap();
    let names: Vec<&str> = rules.checks.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names[0], "chk_entity_with_a_fairly_long_name_status");
    assert_eq!(rules.checks[1].sql, r#"("tags" <@ ARRAY['a', 'b']::pg_catalog.text[])"#);
    assert!(names[2].len() <= 63, "{}", names[2]);
}

#[test]
fn patterns_accept_the_common_subset() {
    for ok in ["^[A-Z]{1,2}$", "^[a-z][a-z0-9_]*$", "^[^[:space:]@]+@[^[:space:]@]+\\.[^[:space:]@]+$", "^(ab|cd)?x$", "\\d+"] {
        pattern::validate(ok).unwrap_or_else(|e| panic!("{ok}: {e}"));
    }
    for bad in ["", "(?P<x>a)", "[[:^alpha:]]", "\\p{L}", "[a&&b]", "a**", &"a".repeat(129), "\\x41"] {
        assert!(pattern::validate(bad).is_err(), "{bad:?} must be refused");
    }
}

// ── Against PostgreSQL ──────────────────────────────────────────────────

async fn pool() -> Option<sqlx::PgPool> {
    let url = std::env::var("TEST_DATABASE_URL").ok()?;
    Some(sqlx::PgPool::connect(&url).await.expect("connect to test DB"))
}

const FIXTURE_TABLE: &str = "
    CREATE TABLE sample (
        id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
        created_at timestamptz NOT NULL DEFAULT now(), updated_at timestamptz NOT NULL DEFAULT now(),
        qty numeric(12,2), price numeric, n double precision, year double precision,
        name text, email text, status text, flag boolean, d1 date, d2 date,
        t1 timestamptz, t2 timestamptz, meta jsonb, ref_id uuid, tags text[], secret text
    )";

/// PostgreSQL requires every function and operator of an index expression to
/// be IMMUTABLE. Indexing each emitted rule therefore proves, against the real
/// catalog, that no allow-listed construct is stable or volatile.
#[tokio::test]
async fn every_emitted_rule_is_immutable_and_valid_sql() {
    let Some(pool) = pool().await else { return };
    let mut conn = pool.acquire().await.unwrap();
    sqlx::raw_sql("DROP SCHEMA IF EXISTS rules_immutable CASCADE; CREATE SCHEMA rules_immutable;")
        .execute(&mut *conn).await.unwrap();
    sqlx::raw_sql(&format!("SET search_path = rules_immutable; {FIXTURE_TABLE}"))
        .execute(&mut *conn).await.unwrap();
    let rules: Vec<String> = GOLDENS.iter().map(|(source, _)| sql_of(source))
        .chain(["length(secret) > 8", "t2 - t1 <= interval '5 hours'", "d2 - d1 > 3"].map(sql_of))
        .chain([
            json!({"name": "qty", "type": "decimal", "minimum": "0", "exclusive_maximum": "9.5", "integer": true, "max_scale": 1}),
            json!({"name": "n", "type": "number", "exclusive_minimum": -1}),
            json!({"name": "name", "type": "text", "not_blank": true, "format": "email", "max_length": 9}),
            json!({"name": "meta", "type": "json", "json_type": "array", "max_items": 3}),
        ].into_iter().flat_map(|field| shorthand_checks(field).unwrap().into_iter().map(|(_, sql)| sql)))
        .collect();
    for (i, rule) in rules.iter().enumerate() {
        sqlx::raw_sql(&format!("CREATE INDEX rule_{i} ON sample ((CASE WHEN {rule} THEN 1 END))"))
            .execute(&mut *conn).await.unwrap_or_else(|e| panic!("{rule}: {e}"));
        sqlx::raw_sql(&format!("ALTER TABLE sample ADD CONSTRAINT chk_{i} CHECK ({rule})"))
            .execute(&mut *conn).await.unwrap_or_else(|e| panic!("{rule}: {e}"));
    }
    sqlx::raw_sql("RESET search_path; DROP SCHEMA rules_immutable CASCADE").execute(&mut *conn).await.unwrap();
}

/// Random rules printed with minimal parentheses — as an author writes them —
/// must mean the same to PostgreSQL as Core's fully parenthesised compilation.
#[tokio::test]
async fn precedence_is_identical_to_postgresql_on_random_rules() {
    use rand::SeedableRng;
    let Some(pool) = pool().await else { return };
    let mut conn = pool.acquire().await.unwrap();
    sqlx::raw_sql("DROP SCHEMA IF EXISTS rules_precedence CASCADE; CREATE SCHEMA rules_precedence;
        SET search_path = rules_precedence;
        CREATE TABLE sample (n double precision, year double precision, flag boolean, name text, status text);
        INSERT INTO sample VALUES (1, 2, true, 'ab', 'x'), (-3, 0, false, 'zz', NULL), (NULL, 5, NULL, NULL, 'y'),
            (0, 0, true, 'a', 'a'), (2.5, -1, false, '', 'b');")
        .execute(&mut *conn).await.unwrap();
    let mut rng = rand::rngs::StdRng::seed_from_u64(0x5eed);
    let mut checked = 0;
    while checked < 400 {
        let source = random::boolean(&mut rng, 4);
        let Ok(check) = compile_check(&source) else { continue };
        let differ: i64 = sqlx::query_scalar(&format!(
            "SELECT count(*) FROM sample WHERE ({source}) IS DISTINCT FROM ({})", check.sql
        ))
        .fetch_one(&mut *conn)
        .await
        .unwrap_or_else(|e| panic!("{source} | {}: {e}", check.sql));
        assert_eq!(differ, 0, "PostgreSQL reads {source:?} differently from {}", check.sql);
        checked += 1;
    }
    sqlx::raw_sql("RESET search_path; DROP SCHEMA rules_precedence CASCADE").execute(&mut *conn).await.unwrap();
}

/// A tiny generator of author-style rules with minimal parentheses.
mod random {
    use rand::Rng;

    fn pick<'a>(rng: &mut impl Rng, items: &[&'a str]) -> &'a str {
        items[rng.gen_range(0..items.len())]
    }

    pub(super) fn number(rng: &mut impl Rng, depth: u32) -> String {
        if depth == 0 || rng.gen_bool(0.35) {
            return pick(rng, &["n", "year", "1", "2", "0", "3.5", "- n", "-1"]).to_string();
        }
        let op = pick(rng, &[" + ", " - ", " * "]);
        let l = number(rng, depth - 1);
        let r = number(rng, depth - 1);
        // Minimal parentheses: `*` binds tighter than `+`/`-`, both left-assoc.
        let wrap_r = |r: &str| r.contains(" + ") || r.contains(" - ") || (op == " * " && r.contains(" * "));
        let l = if op == " * " && (l.contains(" + ") || l.contains(" - ")) { format!("({l})") } else { l };
        let r = if wrap_r(&r) { format!("({r})") } else { r };
        format!("{l}{op}{r}")
    }

    pub(super) fn boolean(rng: &mut impl Rng, depth: u32) -> String {
        if depth == 0 {
            return atom(rng);
        }
        match rng.gen_range(0..7) {
            0 => format!("{} AND {}", boolean(rng, depth - 1), boolean(rng, depth - 1)),
            1 => format!("{} OR {}", boolean(rng, depth - 1), boolean(rng, depth - 1)),
            2 => format!("NOT {}", boolean(rng, depth - 1)),
            3 => format!("({})", boolean(rng, depth - 1)),
            4 => format!("({}) = ({})", boolean(rng, depth - 1), boolean(rng, depth - 1)),
            5 => format!("({}) IS {}NULL", boolean(rng, depth - 1), pick(rng, &["", "NOT "])),
            _ => atom(rng),
        }
    }

    fn atom(rng: &mut impl Rng) -> String {
        match rng.gen_range(0..8) {
            0 => format!("{} {} {}", number(rng, 2), pick(rng, &["=", "<>", "<", "<=", ">", ">=", "!="]), number(rng, 2)),
            1 => format!("{} {}BETWEEN {} AND {}", number(rng, 1), pick(rng, &["", "NOT "]), number(rng, 1), number(rng, 1)),
            2 => format!("{} {}IN (1, 2, -3)", number(rng, 1), pick(rng, &["", "NOT "])),
            3 => format!("name ~ '{}'", pick(rng, &["^a", "b$", "^[a-z]+$", "z"])),
            4 => format!("{} IS {}NULL", pick(rng, &["n", "name", "flag", "n + 1"]), pick(rng, &["", "NOT "])),
            5 => format!("status {}IN ('x', 'y')", pick(rng, &["", "NOT "])),
            6 => format!("length(btrim(coalesce(name, ''))) {} 1", pick(rng, &[">", "=", "<="])),
            _ => pick(rng, &["flag", "flag = true", "flag <> false", "NOT flag"]).to_string(),
        }
    }
}

/// A real application's manifest, kept out of the repository. After the two
/// edits its authors need — `using: "trigram"` instead of a raw operator class,
/// and no misspelled keys — every rule it declares must be admitted unchanged.
#[test]
fn private_corpus_is_admitted_after_trigram_migration() {
    let path = std::env::var("ROOTCX_RULES_CORPUS").unwrap_or_else(|_| {
        concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/private/corpus_manifest.json").into()
    });
    let Ok(raw) = std::fs::read_to_string(&path) else { return };
    let mut value: serde_json::Value = serde_json::from_str(&raw).expect("corpus parses");
    let (mut trigram, mut dropped) = (0, Vec::new());
    for entity in value["dataContract"].as_array_mut().unwrap() {
        for field in entity["fields"].as_array_mut().unwrap() {
            for key in ["onDelete"] {
                if field.as_object_mut().unwrap().remove(key).is_some() {
                    dropped.push(format!("{}.{key}", field["name"]));
                }
            }
        }
        for index in entity.get_mut("indexes").and_then(|i| i.as_array_mut()).into_iter().flatten() {
            let columns = index["columns"].as_array().unwrap();
            let is_trigram = columns.iter().any(|c| c.get("ops").and_then(|o| o.as_str()).is_some_and(|o| o.ends_with("gin_trgm_ops")));
            if is_trigram {
                trigram += 1;
                let names: Vec<serde_json::Value> = columns.iter().map(|c| c["column"].clone()).collect();
                index["columns"] = json!(names);
                index["using"] = json!("trigram");
            }
        }
    }
    let manifest: rootcx_types::AppManifest = serde_json::from_value(value).unwrap();
    crate::manifest::validate_manifest(&manifest).unwrap_or_else(|e| panic!("{e}"));
    let (mut checks, mut indexes) = (0, 0);
    for entity in &manifest.data_contract {
        checks += entity.checks.len();
        indexes += compile_entity(entity).unwrap().indexes.len();
    }
    eprintln!("corpus: {checks} checks, {indexes} indexes admitted; {trigram} trigram indexes migrated; removed {dropped:?}");
}

fn shorthand_checks(field: serde_json::Value) -> Result<Vec<(String, String)>, String> {
    let rules = compile_entity(&serde_json::from_value(json!({"entityName": "t", "fields": [field]})).unwrap())?;
    Ok(rules.checks.into_iter().map(|c| (c.name, c.sql)).collect())
}

#[test]
fn shorthands_compile_to_named_null_tolerant_checks() {
    let checks = shorthand_checks(json!({"name": "amount", "type": "decimal", "precision": 12, "scale": 2,
        "exclusive_minimum": "0", "maximum": "-0.5", "max_scale": 2, "integer": true})).unwrap();
    let finite = r#"("amount" < 'Infinity'::pg_catalog.numeric AND "amount" > '-Infinity'::pg_catalog.numeric)"#;
    assert_eq!(checks, vec![
        ("chk_t_amount_max".into(), format!(r#"(("amount" <= (- 0.5)) AND {finite})"#)),
        ("chk_t_amount_xmin".into(), format!(r#"(("amount" > 0) AND {finite})"#)),
        ("chk_t_amount_int".into(), format!(r#"(("amount" = pg_catalog.trunc("amount")) AND {finite})"#)),
        ("chk_t_amount_scale".into(), format!(r#"((pg_catalog.scale("amount") <= 2) AND {finite})"#)),
    ]);
    let checks = shorthand_checks(json!({"name": "attempts", "type": "number", "minimum": 0, "maximum": 5.5})).unwrap();
    assert_eq!(checks[0].1, r#"(("attempts" >= 0) AND ("attempts" < 'Infinity'::pg_catalog.float8 AND "attempts" > '-Infinity'::pg_catalog.float8))"#);
    assert!(checks[1].1.starts_with(r#"(("attempts" <= 5.5)"#), "{}", checks[1].1);
    let checks = shorthand_checks(json!({"name": "email", "type": "text", "format": "email",
        "not_blank": true, "min_length": 3, "max_length": 320, "pattern": "^[a-z@.]+$"})).unwrap();
    let names: Vec<&str> = checks.iter().map(|(n, _)| n.as_str()).collect();
    assert_eq!(names, ["chk_t_email_minlen", "chk_t_email_maxlen", "chk_t_email_notblank", "chk_t_email_format", "chk_t_email_pattern"]);
    assert_eq!(checks[2].1, r#"(pg_catalog.length(pg_catalog.btrim("email")) > 0)"#);
    assert_eq!(checks[3].1, format!(r#"("email" ~ '{}')"#, shorthand::EMAIL_PATTERN));
    let checks = shorthand_checks(json!({"name": "reminders", "type": "json", "json_type": "array", "max_items": 5})).unwrap();
    assert_eq!(checks[0].1, r#"(pg_catalog.jsonb_typeof("reminders") = 'array')"#);
    assert!(checks[1].1.contains("CASE WHEN pg_catalog.jsonb_typeof"), "{}", checks[1].1);
}

#[test]
fn shorthands_are_refused_where_they_do_not_apply() {
    for (field, fragment) in [
        (json!({"name": "x", "type": "decimal", "minimum": 0}), "JSON string"),
        (json!({"name": "x", "type": "number", "minimum": "0"}), "JSON number"),
        (json!({"name": "x", "type": "decimal", "minimum": "1e5"}), "written in digits"),
        (json!({"name": "x", "type": "decimal", "minimum": "NaN"}), "written in digits"),
        (json!({"name": "x", "type": "text", "minimum": 0}), "number and decimal"),
        (json!({"name": "x", "type": "number", "max_scale": 2}), "decimal"),
        (json!({"name": "x", "type": "number", "max_length": 2}), "text"),
        (json!({"name": "x", "type": "text", "min_length": 5, "max_length": 2}), "exceeds"),
        (json!({"name": "x", "type": "text", "format": "url"}), "unknown format"),
        (json!({"name": "x", "type": "text", "pattern": "(a+)+"}), "quantifier"),
        (json!({"name": "x", "type": "json", "json_type": "list"}), "json_type must be"),
        (json!({"name": "x", "type": "text", "max_items": 3}), "json"),
    ] {
        let error = shorthand_checks(field.clone()).unwrap_err();
        assert!(error.contains(fragment), "{field}: {error}");
    }
}

/// NaN and Infinity are ordered above every number by PostgreSQL, so a plain
/// `x > 0` accepts them; the shorthand's finiteness guard does not.
#[tokio::test]
async fn shorthand_bounds_reject_nan_and_infinity_in_postgresql() {
    let Some(pool) = pool().await else { return };
    let mut conn = pool.acquire().await.unwrap();
    sqlx::raw_sql("DROP SCHEMA IF EXISTS rules_finite CASCADE; CREATE SCHEMA rules_finite;
        SET search_path = rules_finite; CREATE TABLE t (f double precision, d numeric);")
        .execute(&mut *conn).await.unwrap();
    for field in [json!({"name": "f", "type": "number", "exclusive_minimum": 0}),
                  json!({"name": "d", "type": "decimal", "maximum": "10", "max_scale": 2})] {
        for (name, sql) in shorthand_checks(field).unwrap() {
            sqlx::raw_sql(&format!("ALTER TABLE t ADD CONSTRAINT {name} CHECK ({sql})"))
                .execute(&mut *conn).await.unwrap();
        }
    }
    for ok in ["(1, 1.25)", "(NULL, NULL)", "(0.5, -3)"] {
        sqlx::raw_sql(&format!("INSERT INTO t VALUES {ok}")).execute(&mut *conn).await
            .unwrap_or_else(|e| panic!("{ok}: {e}"));
    }
    for bad in ["('NaN', NULL)", "('Infinity', NULL)", "(0, NULL)", "(NULL, 'NaN')", "(NULL, 'Infinity')", "(NULL, 1.234)", "(NULL, 11)"] {
        let error = sqlx::raw_sql(&format!("INSERT INTO t VALUES {bad}")).execute(&mut *conn).await
            .expect_err(bad);
        assert_eq!(error.as_database_error().and_then(|e| e.code()).as_deref(), Some("23514"), "{bad}");
    }
    sqlx::raw_sql("RESET search_path; DROP SCHEMA rules_finite CASCADE").execute(&mut *conn).await.unwrap();
}
