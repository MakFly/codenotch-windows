//! The CLI coding agents CodeNotch knows, from data/cli-agents.json (generated from Orca's
//! catalog by `bun run sync:agents`).
use serde::Deserialize;
use std::sync::OnceLock;

#[derive(Deserialize)]
pub struct Agent {
    pub id: String,
    pub name: String,
    /// Executables to look for, the official one first.
    pub commands: Vec<String>,
    pub homepage: Option<String>,
    /// Install command verified for Windows; otherwise the homepage explains it.
    pub install: Option<String>,
    /// CodeNotch provider that reads this CLI's quotas, if any.
    pub quota: Option<String>,
    /// Logo key in assets/glyphs.json.
    pub logo: Option<String>,
}

#[derive(Deserialize)]
struct Catalog {
    agents: Vec<Agent>,
}

pub fn all() -> &'static [Agent] {
    static AGENTS: OnceLock<Vec<Agent>> = OnceLock::new();
    AGENTS.get_or_init(|| {
        serde_json::from_str::<Catalog>(include_str!("../../data/cli-agents.json"))
            .map(|c| c.agents)
            .unwrap_or_default()
    })
}

/// Where each agent's CLI is installed on this PC, in catalog order.
pub fn detect() -> Vec<Option<String>> {
    all()
        .iter()
        .map(|a| crate::login::find_command(&a.commands).map(|p| p.to_string_lossy().to_string()))
        .collect()
}
