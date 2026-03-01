use std::collections::HashMap;
use std::time::Instant;

use serde_json::Value as JsonValue;
use rootcx_types::{SupervisionConfig, SupervisionMode};

pub enum PolicyDecision {
    Allow,
    RequiresApproval { reason: String },
    RateLimited { retry_after_secs: u64 },
}

pub struct PolicyEvaluator {
    config: SupervisionConfig,
    /// tool_name -> timestamps of recent calls (for rate limiting)
    call_times: HashMap<String, Vec<Instant>>,
}

impl PolicyEvaluator {
    pub fn new(config: SupervisionConfig) -> Self {
        Self { config, call_times: HashMap::new() }
    }

    pub fn evaluate(&mut self, tool_name: &str, args: &JsonValue) -> PolicyDecision {
        // Check rate limits first (applies to all modes)
        if let Some(decision) = self.check_rate_limits(tool_name) {
            return decision;
        }

        // Record this call for rate limiting
        self.call_times.entry(tool_name.to_string()).or_default().push(Instant::now());

        match self.config.mode {
            SupervisionMode::Autonomous => PolicyDecision::Allow,
            SupervisionMode::Strict => PolicyDecision::RequiresApproval {
                reason: format!("strict mode: {tool_name} requires approval"),
            },
            SupervisionMode::Supervised => self.check_policies(tool_name, args),
        }
    }

    fn check_policies(&self, tool_name: &str, args: &JsonValue) -> PolicyDecision {
        let action = tool_action(tool_name);
        let entity = args.get("entity").and_then(|e| e.as_str());

        for policy in &self.config.policies {
            // Match action
            if policy.action != "*" && policy.action != action {
                continue;
            }

            // Match entity (if specified in policy)
            if let Some(ref policy_entity) = policy.entity {
                match entity {
                    Some(e) if e == policy_entity => {}
                    _ => continue,
                }
            }

            // Check if approval is required
            if policy.requires.as_deref() == Some("approval") {
                let reason = match (&policy.entity, entity) {
                    (Some(pe), Some(e)) => format!("policy: {action} on {e} (matched {pe})"),
                    _ => format!("policy: {action} requires approval"),
                };
                return PolicyDecision::RequiresApproval { reason };
            }
        }

        PolicyDecision::Allow
    }

    fn check_rate_limits(&mut self, tool_name: &str) -> Option<PolicyDecision> {
        let action = tool_action(tool_name);

        for policy in &self.config.policies {
            if policy.action != "*" && policy.action != action {
                continue;
            }

            let Some(ref rl) = policy.rate_limit else { continue };
            let window_secs = parse_window(&rl.window);
            let now = Instant::now();

            let times = self.call_times.entry(tool_name.to_string()).or_default();
            times.retain(|t| now.duration_since(*t).as_secs() < window_secs);

            if times.len() as u32 >= rl.max {
                return Some(PolicyDecision::RateLimited {
                    retry_after_secs: window_secs.saturating_sub(
                        now.duration_since(*times.first().unwrap()).as_secs()
                    ),
                });
            }
        }

        None
    }
}

fn tool_action(tool_name: &str) -> &str {
    match tool_name {
        "query_data" => "query",
        "mutate_data" => "mutate",
        _ => tool_name,
    }
}

fn parse_window(window: &str) -> u64 {
    let s = window.trim();
    if let Some(n) = s.strip_suffix('h') {
        n.parse::<u64>().unwrap_or(1) * 3600
    } else if let Some(n) = s.strip_suffix('m') {
        n.parse::<u64>().unwrap_or(1) * 60
    } else if let Some(n) = s.strip_suffix('d') {
        n.parse::<u64>().unwrap_or(1) * 86400
    } else if let Some(n) = s.strip_suffix('s') {
        n.parse::<u64>().unwrap_or(60)
    } else {
        s.parse::<u64>().unwrap_or(3600)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rootcx_types::{SupervisionPolicy, RateLimit};
    use serde_json::json;

    fn config(mode: SupervisionMode, policies: Vec<SupervisionPolicy>) -> SupervisionConfig {
        SupervisionConfig { mode, policies }
    }

    #[test]
    fn autonomous_always_allows() {
        let mut eval = PolicyEvaluator::new(config(SupervisionMode::Autonomous, vec![]));
        assert!(matches!(eval.evaluate("mutate_data", &json!({})), PolicyDecision::Allow));
    }

    #[test]
    fn strict_requires_approval_for_all() {
        let mut eval = PolicyEvaluator::new(config(SupervisionMode::Strict, vec![]));
        assert!(matches!(eval.evaluate("mutate_data", &json!({})), PolicyDecision::RequiresApproval { .. }));
        assert!(matches!(eval.evaluate("query_data", &json!({})), PolicyDecision::RequiresApproval { .. }));
        assert!(matches!(eval.evaluate("browser", &json!({})), PolicyDecision::RequiresApproval { .. }));
    }

    #[test]
    fn supervised_checks_policies() {
        let mut eval = PolicyEvaluator::new(config(SupervisionMode::Supervised, vec![
            SupervisionPolicy {
                action: "mutate".into(),
                entity: Some("Orders".into()),
                requires: Some("approval".into()),
                rate_limit: None,
            },
        ]));
        // Matching entity
        assert!(matches!(
            eval.evaluate("mutate_data", &json!({"entity": "Orders"})),
            PolicyDecision::RequiresApproval { .. }
        ));
        // Non-matching entity
        assert!(matches!(
            eval.evaluate("mutate_data", &json!({"entity": "Users"})),
            PolicyDecision::Allow
        ));
    }

    #[test]
    fn rate_limit_blocks() {
        let mut eval = PolicyEvaluator::new(config(SupervisionMode::Autonomous, vec![
            SupervisionPolicy {
                action: "mutate".into(),
                entity: None,
                requires: None,
                rate_limit: Some(RateLimit { max: 2, window: "1h".into() }),
            },
        ]));
        assert!(matches!(eval.evaluate("mutate_data", &json!({})), PolicyDecision::Allow));
        assert!(matches!(eval.evaluate("mutate_data", &json!({})), PolicyDecision::Allow));
        assert!(matches!(eval.evaluate("mutate_data", &json!({})), PolicyDecision::RateLimited { .. }));
    }

    #[test]
    fn parse_window_variants() {
        assert_eq!(parse_window("1h"), 3600);
        assert_eq!(parse_window("30m"), 1800);
        assert_eq!(parse_window("1d"), 86400);
        assert_eq!(parse_window("60s"), 60);
    }
}
