mod config;

pub mod commands;
pub mod goals;
pub mod skills;
pub mod vcs;
pub mod websearch;

pub use commands::CommandsPlugin;
pub use goals::GoalsPlugin;
pub use skills::SkillsPlugin;
pub use vcs::VcsPlugin;
pub use websearch::WebsearchPlugin;