//! Governance release gate: one Cargo binary, fresh PostgreSQL database per test.
mod harness;

#[path = "governance/harness_isolation_test.rs"]
mod harness_isolation_test;

#[path = "governance/collection_reads_test.rs"]
mod collection_reads_test;

#[path = "governance/sensitive_sql_test.rs"]
mod sensitive_sql_test;

#[path = "governance/agent_identity_test.rs"]
mod agent_identity_test;
#[path = "governance/cross_app_crud_test.rs"]
mod cross_app_crud_test;
#[path = "governance/cross_app_delegation_test.rs"]
mod cross_app_delegation_test;
#[path = "governance/cross_app_grants_test.rs"]
mod cross_app_grants_test;
#[path = "governance/cross_app_jobs_test.rs"]
mod cross_app_jobs_test;
#[path = "governance/cross_app_lifecycle_test.rs"]
mod cross_app_lifecycle_test;
#[path = "governance/cross_app_metadata_test.rs"]
mod cross_app_metadata_test;
#[path = "governance/delegation_matrix_test.rs"]
mod delegation_matrix_test;
#[path = "governance/governance_contract_test.rs"]
mod governance_contract_test;
#[path = "governance/row_ownership_test.rs"]
mod row_ownership_test;
#[path = "governance/publications_test.rs"]
mod publications_test;
#[path = "governance/tool_availability_test.rs"]
mod tool_availability_test;
#[path = "governance/workflows_integration.rs"]
mod workflows_integration;

#[path = "governance/assignment_access_test.rs"]
mod assignment_access_test;

#[path = "governance/resource_sharing_test.rs"]
mod resource_sharing_test;

#[path = "governance/approved_actions_test.rs"]
mod approved_actions_test;

#[path = "governance/manifest_sql_admission_test.rs"]
mod manifest_sql_admission_test;

#[path = "governance/governed_rules_test.rs"]
mod governed_rules_test;

#[path = "governance/row_access_projection_test.rs"]
mod row_access_projection_test;
