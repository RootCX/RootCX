use std::collections::HashMap;

use chrono::{DateTime, NaiveDate, Utc};
use rootcx_types::{EntityContract, FieldContract};
use serde_json::Value as JsonValue;
use sqlx::types::{BigDecimal, Uuid};

use crate::manifest::{quote_ident, quote_literal};

const MAX_NUMERIC_PRECISION: u16 = 1_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DecimalType {
    precision: Option<u16>,
    scale: Option<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum FieldType {
    Text,
    Number,
    Decimal(DecimalType),
    Boolean,
    Date,
    Timestamp,
    Json,
    File,
    Uuid,
    EntityLink,
    TextArray,
    NumberArray,
}

/// A field as the generated data paths see it. Sensitivity travels with the
/// type — rather than in a second map threaded through every read path — so a
/// caller that knows the type can never forget the flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Field {
    pub(crate) field_type: FieldType,
    pub(crate) sensitive: bool,
}

pub(crate) type FieldTypes = HashMap<String, Field>;

impl Field {
    fn from_field(field: &FieldContract) -> Result<Self, String> {
        Ok(Self {
            field_type: FieldType::from_field(field)?,
            sensitive: field.sensitive,
        })
    }

    fn readable(field_type: FieldType) -> Self {
        Self {
            field_type,
            sensitive: false,
        }
    }
}

/// Matching field names, sorted. The generated SQL is a prepared-statement
/// cache key, so field order must not vary between calls.
fn sorted_names<'a>(types: &'a FieldTypes, pick: impl Fn(&Field) -> bool) -> Vec<&'a str> {
    let mut names: Vec<&str> = types
        .iter()
        .filter_map(|(name, field)| pick(field).then_some(name.as_str()))
        .collect();
    names.sort_unstable();
    names
}

impl DecimalType {
    fn from_field(field: &FieldContract) -> Result<Self, String> {
        match (field.precision, field.scale) {
            (None, None) => Ok(Self {
                precision: None,
                scale: None,
            }),
            (Some(precision), Some(scale))
                if (1..=MAX_NUMERIC_PRECISION).contains(&precision) && scale <= precision =>
            {
                Ok(Self {
                    precision: Some(precision),
                    scale: Some(scale),
                })
            }
            (Some(0), Some(_)) => Err(format!(
                "decimal field '{}' precision must be at least 1",
                field.name
            )),
            (Some(precision), Some(_)) if precision > MAX_NUMERIC_PRECISION => Err(format!(
                "decimal field '{}' precision must not exceed {MAX_NUMERIC_PRECISION}",
                field.name
            )),
            (Some(precision), Some(scale)) => Err(format!(
                "decimal field '{}' scale ({scale}) must not exceed precision ({precision})",
                field.name
            )),
            _ => Err(format!(
                "decimal field '{}' must declare precision and scale together, or neither",
                field.name
            )),
        }
    }
}

impl FieldType {
    pub(crate) fn from_field(field: &FieldContract) -> Result<Self, String> {
        let field_type = match field.field_type.as_str() {
            "text" => Self::Text,
            "number" => Self::Number,
            "decimal" => Self::Decimal(DecimalType::from_field(field)?),
            "boolean" => Self::Boolean,
            "date" => Self::Date,
            "timestamp" => Self::Timestamp,
            "json" => Self::Json,
            "file" => Self::File,
            "uuid" => Self::Uuid,
            "entity_link" => Self::EntityLink,
            "[text]" => Self::TextArray,
            "[number]" => Self::NumberArray,
            other => return Err(format!("field '{}' has unknown type '{other}'", field.name)),
        };

        if !matches!(field_type, Self::Decimal(_))
            && (field.precision.is_some() || field.scale.is_some())
        {
            return Err(format!(
                "field '{}' can only declare precision and scale when type is 'decimal'",
                field.name
            ));
        }

        if let Some(default) = &field.default_value {
            field_type.validate_default(&field.name, default)?;
        }

        Ok(field_type)
    }

    pub(crate) fn postgres_type(&self) -> String {
        match self {
            Self::Text | Self::File => "TEXT".into(),
            Self::Number => "DOUBLE PRECISION".into(),
            Self::Decimal(DecimalType {
                precision: Some(precision),
                scale: Some(scale),
            }) => {
                format!("NUMERIC({precision},{scale})")
            }
            Self::Decimal(_) => "NUMERIC".into(),
            Self::Boolean => "BOOLEAN".into(),
            Self::Date => "DATE".into(),
            Self::Timestamp => "TIMESTAMPTZ".into(),
            Self::Json => "JSONB".into(),
            Self::Uuid | Self::EntityLink => "UUID".into(),
            Self::TextArray => "TEXT[]".into(),
            Self::NumberArray => "DOUBLE PRECISION[]".into(),
        }
    }

    pub(crate) fn parameter_cast(&self) -> &'static str {
        match self {
            Self::Number => "::float8",
            Self::Decimal(_) => "::numeric",
            Self::Boolean => "::boolean",
            Self::Date => "::date",
            Self::Timestamp => "::timestamptz",
            Self::Uuid | Self::EntityLink => "::uuid",
            Self::Json => "::jsonb",
            Self::TextArray => "::text[]",
            Self::NumberArray => "::float8[]",
            Self::Text | Self::File => "",
        }
    }

    pub(crate) fn is_json(&self) -> bool {
        matches!(self, Self::Json)
    }

    pub(crate) fn is_decimal(&self) -> bool {
        matches!(self, Self::Decimal(_))
    }

    fn validate_default(&self, field_name: &str, value: &JsonValue) -> Result<(), String> {
        if !self.is_decimal() || value.is_null() {
            return Ok(());
        }
        let literal = value.as_str().ok_or_else(|| {
            format!("decimal field '{field_name}' default_value must be a JSON string")
        })?;
        parse_decimal(literal, field_name).map(|_| ())
    }
}

pub(crate) fn field_types(entity: &EntityContract) -> Result<FieldTypes, String> {
    let mut types = entity
        .fields
        .iter()
        .map(|field| Ok((field.name.clone(), Field::from_field(field)?)))
        .collect::<Result<FieldTypes, String>>()?;
    // System fields last so a manifest can never shadow one as sensitive and
    // hide `id` from every response.
    types.extend(system_field_types());
    Ok(types)
}

pub(crate) fn system_field_types() -> FieldTypes {
    [
        ("id".into(), Field::readable(FieldType::Uuid)),
        ("created_at".into(), Field::readable(FieldType::Timestamp)),
        ("updated_at".into(), Field::readable(FieldType::Timestamp)),
    ]
    .into_iter()
    .collect()
}

pub(crate) fn sql_default(value: &JsonValue, field_type: &FieldType) -> Option<String> {
    match value {
        JsonValue::Null => None,
        JsonValue::Bool(value) => Some(value.to_string()),
        JsonValue::Number(value) => Some(value.to_string()),
        JsonValue::String(value) if field_type.is_decimal() => {
            parse_decimal(value, "default_value")
                .ok()
                .map(|value| value.to_string())
        }
        JsonValue::String(value) => Some(quote_literal(value)),
        JsonValue::Array(_) | JsonValue::Object(_) if field_type.is_json() => {
            Some(format!("{}::jsonb", quote_literal(&value.to_string())))
        }
        JsonValue::Array(_) | JsonValue::Object(_) => None,
    }
}

/// Build the one canonical database-row → JSON representation used by every
/// app-data read path. Two transforms, in order:
///
/// 1. Sensitive fields are deleted. `to_jsonb(record.*)` takes the whole row as
///    one value, so there is no column list to omit them from; `- text[]`
///    removes the keys inside Postgres instead, before the row ever reaches the
///    Core. Because every read path already routes through here, no caller can
///    forget to strip them.
/// 2. Decimal columns override their JSON value with text — PostgreSQL can
///    represent NUMERIC exactly, JSON/JS cannot. This runs *after* the deletion
///    so a sensitive decimal stays deleted instead of being re-added as text.
pub(crate) fn row_json(record_ref: &str, types: &FieldTypes) -> String {
    let mut expression = format!("to_jsonb({record_ref}.*)");

    let sensitive = sorted_names(types, |field| field.sensitive);
    if !sensitive.is_empty() {
        let keys: Vec<String> = sensitive.iter().map(|name| quote_literal(name)).collect();
        expression = format!("({expression} - ARRAY[{}]::text[])", keys.join(", "));
    }

    for field in sorted_names(types, |field| {
        field.field_type.is_decimal() && !field.sensitive
    }) {
        expression.push_str(&format!(
            " || jsonb_build_object({}, to_jsonb({record_ref}.{}::text))",
            quote_literal(field),
            quote_ident(field)
        ));
    }
    expression
}

pub(crate) type JsonRowQuery<'q> =
    sqlx::query::QueryAs<'q, sqlx::Postgres, (JsonValue,), sqlx::postgres::PgArguments>;

/// Bind using the manifest type so prepared statements keep stable parameter
/// OIDs. Decimal accepts strings only: accepting a JSON number would lose the
/// exactness guarantee before the Core sees the value.
pub(crate) fn bind_typed<'q>(
    query: JsonRowQuery<'q>,
    value: &'q JsonValue,
    field_type: Option<&FieldType>,
) -> Result<JsonRowQuery<'q>, String> {
    if value.is_null() {
        return Ok(match field_type {
            Some(FieldType::Number) => query.bind(None::<f64>),
            Some(FieldType::Decimal(_)) => query.bind(None::<BigDecimal>),
            Some(FieldType::Boolean) => query.bind(None::<bool>),
            Some(FieldType::Uuid | FieldType::EntityLink) => query.bind(None::<Uuid>),
            Some(FieldType::Date) => query.bind(None::<NaiveDate>),
            Some(FieldType::Timestamp) => query.bind(None::<DateTime<Utc>>),
            Some(FieldType::Json) => query.bind(None::<JsonValue>),
            Some(FieldType::TextArray) => query.bind(None::<Vec<String>>),
            Some(FieldType::NumberArray) => query.bind(None::<Vec<f64>>),
            _ => query.bind(None::<String>),
        });
    }

    Ok(match field_type {
        Some(FieldType::Number) => query.bind(coerce_f64(value)),
        Some(FieldType::Decimal(_)) => {
            let literal = value
                .as_str()
                .ok_or("decimal value must be a JSON string")?;
            query.bind(parse_decimal(literal, "value")?)
        }
        Some(FieldType::Boolean) => query.bind(coerce_bool(value)),
        Some(FieldType::Uuid | FieldType::EntityLink) => {
            query.bind(coerce_str(value).and_then(|value| value.parse::<Uuid>().ok()))
        }
        Some(FieldType::Date) => {
            query.bind(coerce_str(value).and_then(|value| value.parse::<NaiveDate>().ok()))
        }
        Some(FieldType::Timestamp) => {
            query.bind(coerce_str(value).and_then(|value| value.parse::<DateTime<Utc>>().ok()))
        }
        Some(FieldType::Json) => query.bind(value),
        Some(FieldType::TextArray) => query.bind(coerce_str_vec(value)),
        Some(FieldType::NumberArray) => query.bind(coerce_f64_vec(value)),
        _ => match value {
            JsonValue::String(value) => query.bind(value.as_str()),
            _ => query.bind(json_value_to_string(value)),
        },
    })
}

fn parse_decimal(value: &str, label: &str) -> Result<BigDecimal, String> {
    value
        .parse()
        .map_err(|_| format!("invalid decimal {label}: '{value}'"))
}

pub(crate) fn json_value_to_string(value: &JsonValue) -> String {
    match value {
        JsonValue::String(value) => value.clone(),
        JsonValue::Number(value) => value.to_string(),
        JsonValue::Bool(value) => value.to_string(),
        JsonValue::Null => String::new(),
        _ => value.to_string(),
    }
}

fn coerce_f64(value: &JsonValue) -> f64 {
    match value {
        JsonValue::Number(value) => value.as_f64().unwrap_or(0.0),
        JsonValue::String(value) => value.parse().unwrap_or(0.0),
        JsonValue::Bool(value) => i32::from(*value) as f64,
        _ => 0.0,
    }
}

fn coerce_bool(value: &JsonValue) -> bool {
    match value {
        JsonValue::Bool(value) => *value,
        JsonValue::Number(value) => value.as_i64().is_some_and(|value| value != 0),
        JsonValue::String(value) => matches!(value.as_str(), "true" | "1"),
        _ => false,
    }
}

fn coerce_str(value: &JsonValue) -> Option<&str> {
    value.as_str()
}

fn coerce_str_vec(value: &JsonValue) -> Vec<&str> {
    match value {
        JsonValue::Array(values) => values.iter().filter_map(JsonValue::as_str).collect(),
        _ => Vec::new(),
    }
}

fn coerce_f64_vec(value: &JsonValue) -> Vec<f64> {
    match value {
        JsonValue::Array(values) => values.iter().filter_map(JsonValue::as_f64).collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn field(field_type: &str, precision: Option<u16>, scale: Option<u16>) -> FieldContract {
        FieldContract {
            name: "price".into(),
            field_type: field_type.into(),
            precision,
            scale,
            required: false,
            default_value: None,
            enum_values: None,
            references: None,
            is_primary_key: None,
            on_delete: None,
            sensitive: false,
            owner: false,
        }
    }

    fn types_of(entity_fields: &[(&str, &str, bool)]) -> FieldTypes {
        let entity = EntityContract {
            entity_name: "account".into(),
            fields: entity_fields
                .iter()
                .map(|(name, field_type, sensitive)| FieldContract {
                    name: (*name).into(),
                    field_type: (*field_type).into(),
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
        field_types(&entity).expect("valid entity")
    }

    /// Asserted on the exact string, not on `contains`: the projection is the
    /// only thing standing between a sensitive column and the wire, so a
    /// substring match would pass even if a later transform re-added the key.
    /// Multi-field cases also pin the ordering, which must stay stable because
    /// the generated SQL is a prepared-statement cache key.
    #[test]
    fn row_json_excludes_exactly_the_sensitive_fields() {
        for (fields, expected) in [
            // No sensitive field: byte-identical to the pre-feature projection.
            (vec![("email", "text", false)], "to_jsonb(t.*)".to_string()),
            (
                vec![("email", "text", false), ("password_hash", "text", true)],
                "(to_jsonb(t.*) - ARRAY['password_hash']::text[])".to_string(),
            ),
            // Two sensitive fields, sorted regardless of declaration order.
            (
                vec![("token", "text", true), ("password_hash", "text", true)],
                "(to_jsonb(t.*) - ARRAY['password_hash', 'token']::text[])".to_string(),
            ),
            // A readable decimal still gets its exact-text override.
            (
                vec![("salary", "decimal", false)],
                "to_jsonb(t.*) || jsonb_build_object('salary', to_jsonb(t.\"salary\"::text))"
                    .to_string(),
            ),
            // A sensitive decimal must not come back through that override.
            (
                vec![("salary", "decimal", true)],
                "(to_jsonb(t.*) - ARRAY['salary']::text[])".to_string(),
            ),
        ] {
            assert_eq!(
                row_json("t", &types_of(&fields)),
                expected,
                "projection mismatch for {fields:?}"
            );
        }
    }

    /// A manifest field named like a system column must not be able to hide
    /// `id` from every response.
    #[test]
    fn sensitive_cannot_shadow_a_system_field() {
        let types = types_of(&[("id", "uuid", true)]);
        assert!(
            !types["id"].sensitive,
            "system fields must stay readable regardless of the manifest"
        );
    }

    #[test]
    fn decimal_contract_rejects_ambiguous_or_invalid_bounds() {
        for (precision, scale, expected) in [
            (Some(10), None, "together"),
            (None, Some(2), "together"),
            (Some(0), Some(0), "at least 1"),
            (Some(1_001), Some(2), "must not exceed 1000"),
            (Some(4), Some(5), "must not exceed precision"),
        ] {
            let error = FieldType::from_field(&field("decimal", precision, scale))
                .expect_err("invalid decimal contract must fail");
            assert!(
                error.contains(expected),
                "precision={precision:?}, scale={scale:?}: {error}"
            );
        }
    }

    #[test]
    fn decimal_contract_accepts_unconstrained_and_postgres_boundary_bounds() {
        for (precision, scale) in [
            (None, None),
            (Some(1), Some(0)),
            (Some(MAX_NUMERIC_PRECISION), Some(MAX_NUMERIC_PRECISION)),
        ] {
            let result = FieldType::from_field(&field("decimal", precision, scale));
            assert!(
                result.is_ok(),
                "precision={precision:?}, scale={scale:?}: {result:?}"
            );
        }
    }

    #[test]
    fn decimal_default_requires_a_valid_exact_string() {
        for (default, expected) in [
            (serde_json::json!(12.34), "must be a JSON string"),
            (serde_json::json!("not-a-decimal"), "invalid decimal"),
            (serde_json::json!(""), "invalid decimal"),
        ] {
            let mut contract = field("decimal", None, None);
            contract.default_value = Some(default.clone());
            let error = FieldType::from_field(&contract)
                .expect_err("an inexact or malformed decimal default must fail");
            assert!(error.contains(expected), "default={default}: {error}");
        }
    }

    #[test]
    fn non_decimal_fields_reject_decimal_constraints() {
        for field_type in ["text", "number", "boolean", "date", "timestamp", "json"] {
            let error = FieldType::from_field(&field(field_type, Some(10), Some(2)))
                .expect_err("only decimal fields may declare precision and scale");
            assert!(
                error.contains("only declare precision and scale"),
                "type={field_type}: {error}"
            );
        }
    }
}
