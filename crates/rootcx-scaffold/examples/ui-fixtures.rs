//! Emit the real app variants used by scripts/check-ui-consumer.mjs.
use rootcx_scaffold::{Answer, create, layers::AgentLayer};
use std::{collections::HashMap, path::PathBuf};

#[tokio::main]
async fn main() -> Result<(), String> {
    let destination = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("Usage: ui-fixtures <output-directory>")?,
    );
    for (name, auth, agent) in [
        ("simple", false, false),
        ("auth", true, false),
        ("agent", true, true),
    ] {
        let answers = HashMap::from([
            ("auth".to_owned(), Answer::Bool(auth)),
            ("backend".to_owned(), Answer::Bool(false)),
        ]);
        let layers: Vec<Box<dyn rootcx_scaffold::types::Layer>> = if agent {
            vec![Box::new(AgentLayer)]
        } else {
            vec![]
        };
        create(&destination.join(name), name, "blank", answers, layers).await?;
    }
    Ok(())
}
