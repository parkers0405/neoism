pub mod config;

pub mod commands;
pub mod agents;
pub mod goals;
pub mod skills;
pub mod system_prompt;
pub mod vcs;
pub mod websearch;

pub use commands::CommandsPlugin;
pub use agents::AgentsPlugin;
pub use config::ConfigPlugin;
pub use goals::GoalsPlugin;
pub use skills::SkillsPlugin;
pub use system_prompt::SystemPromptPlugin;
pub use vcs::VcsPlugin;
pub use websearch::WebsearchPlugin;