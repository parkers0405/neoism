/// Display name and channel namespace. Namespace matching uses whole path segments.
pub use neochat::PROJECTS;

pub fn contains(project: &str, channel: &str) -> bool {
    channel.split('/').next() == Some(project)
}

pub fn channel(project: &str, name: &str) -> String {
    if contains(project,name) {name.into()} else {format!("{project}/{name}")}
}