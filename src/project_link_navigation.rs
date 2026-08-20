use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use crate::color_conversion::{LinkedProjectRef, ProjectRole};
use crate::model::ShadeProject;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkedProjectNavigationStatus {
    Ready,
    Missing,
    Unreadable,
    RoleMismatch,
    ReciprocalLinkMissing,
}

impl LinkedProjectNavigationStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::Missing => "Missing",
            Self::Unreadable => "Unreadable",
            Self::RoleMismatch => "Role mismatch",
            Self::ReciprocalLinkMissing => "Reciprocal link missing",
        }
    }

    pub fn can_open(self) -> bool {
        self == Self::Ready
    }
}

#[derive(Clone, Debug)]
pub struct LinkedProjectNavigationTarget {
    pub role: ProjectRole,
    pub path: PathBuf,
    pub project_name: Option<String>,
    pub status: LinkedProjectNavigationStatus,
    pub detail: String,
}

/// Resolve and validate the projects that may be reached from the current Source/Production
/// project header. This is navigation metadata only: it never mutates either project and never
/// repairs links implicitly.
pub fn linked_navigation_targets(
    project: &ShadeProject,
    current_project_path: &Path,
) -> Vec<LinkedProjectNavigationTarget> {
    let expected_target_role = match project.project_role {
        ProjectRole::Source => ProjectRole::Production,
        ProjectRole::Production => ProjectRole::Source,
        ProjectRole::Standalone => return Vec::new(),
    };

    let current_dir = current_project_path
        .parent()
        .unwrap_or_else(|| Path::new("."));
    let mut seen = BTreeSet::new();
    let mut targets = Vec::new();

    for link in project
        .linked_projects
        .iter()
        .filter(|link| link.role == expected_target_role)
    {
        let path = resolve_link_path(current_dir, &link.path);
        let key = path_key(&path);
        if !seen.insert(key) {
            continue;
        }
        targets.push(inspect_link(
            project.project_role,
            current_project_path,
            expected_target_role,
            path,
        ));
    }

    targets.sort_by(|left, right| {
        left.project_name
            .as_deref()
            .unwrap_or("")
            .to_lowercase()
            .cmp(
                &right
                    .project_name
                    .as_deref()
                    .unwrap_or("")
                    .to_lowercase(),
            )
            .then_with(|| path_key(&left.path).cmp(&path_key(&right.path)))
    });
    targets
}

pub fn ready_navigation_targets(
    project: &ShadeProject,
    current_project_path: &Path,
) -> Vec<LinkedProjectNavigationTarget> {
    linked_navigation_targets(project, current_project_path)
        .into_iter()
        .filter(|target| target.status.can_open())
        .collect()
}

fn inspect_link(
    current_role: ProjectRole,
    current_project_path: &Path,
    expected_target_role: ProjectRole,
    path: PathBuf,
) -> LinkedProjectNavigationTarget {
    if !path.is_file() {
        return LinkedProjectNavigationTarget {
            role: expected_target_role,
            path,
            project_name: None,
            status: LinkedProjectNavigationStatus::Missing,
            detail: "Linked project file does not exist.".to_owned(),
        };
    }

    let linked = match ShadeProject::load(&path) {
        Ok(project) => project,
        Err(error) => {
            return LinkedProjectNavigationTarget {
                role: expected_target_role,
                path,
                project_name: None,
                status: LinkedProjectNavigationStatus::Unreadable,
                detail: error,
            };
        }
    };

    let project_name = Some(linked.name.clone()).filter(|name| !name.trim().is_empty());
    if linked.project_role != expected_target_role {
        return LinkedProjectNavigationTarget {
            role: expected_target_role,
            path,
            project_name,
            status: LinkedProjectNavigationStatus::RoleMismatch,
            detail: format!(
                "Linked project declares {:?}; expected {:?}.",
                linked.project_role, expected_target_role
            ),
        };
    }

    if !has_reciprocal_link(&linked, &path, current_role, current_project_path) {
        return LinkedProjectNavigationTarget {
            role: expected_target_role,
            path,
            project_name,
            status: LinkedProjectNavigationStatus::ReciprocalLinkMissing,
            detail: "Linked project does not point back to this project.".to_owned(),
        };
    }

    LinkedProjectNavigationTarget {
        role: expected_target_role,
        path,
        project_name,
        status: LinkedProjectNavigationStatus::Ready,
        detail: "Linked project is available and reciprocal.".to_owned(),
    }
}

fn has_reciprocal_link(
    linked: &ShadeProject,
    linked_project_path: &Path,
    expected_role: ProjectRole,
    current_project_path: &Path,
) -> bool {
    let linked_dir = linked_project_path
        .parent()
        .unwrap_or_else(|| Path::new("."));
    linked
        .linked_projects
        .iter()
        .filter(|candidate| candidate.role == expected_role)
        .any(|candidate| {
            let candidate_path = resolve_link_path(linked_dir, &candidate.path);
            paths_match(&candidate_path, current_project_path)
        })
}

fn resolve_link_path(base: &Path, stored: &str) -> PathBuf {
    let path = PathBuf::from(stored);
    if path.is_absolute() {
        path
    } else {
        base.join(path)
    }
}

fn paths_match(left: &Path, right: &Path) -> bool {
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => path_key(&left) == path_key(&right),
        _ => path_key(left) == path_key(right),
    }
}

fn path_key(path: &Path) -> String {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if parts.last().is_some_and(|part: &String| part != "..") {
                    parts.pop();
                } else {
                    parts.push("..".to_owned());
                }
            }
            Component::Prefix(prefix) => {
                parts.push(prefix.as_os_str().to_string_lossy().to_lowercase())
            }
            Component::RootDir => parts.push("/".to_owned()),
            Component::Normal(value) => parts.push(value.to_string_lossy().to_lowercase()),
        }
    }
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_dir(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "shade-project-navigation-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn save_project(path: &Path, project: &ShadeProject) {
        project.save_new(path, &[]).unwrap();
    }

    fn linked(role: ProjectRole, path: &Path) -> LinkedProjectRef {
        LinkedProjectRef {
            role,
            path: path.to_string_lossy().into_owned(),
        }
    }

    #[test]
    fn source_and_production_with_reciprocal_links_are_ready() {
        let dir = temp_dir("ready");
        let source_path = dir.join("source.shade");
        let production_path = dir.join("production.shade");

        let mut source = ShadeProject::default();
        source.name = "Source".to_owned();
        source.project_role = ProjectRole::Source;
        source.linked_projects.push(linked(ProjectRole::Production, &production_path));

        let mut production = ShadeProject::default();
        production.name = "Production 7C".to_owned();
        production.project_role = ProjectRole::Production;
        production.linked_projects.push(linked(ProjectRole::Source, &source_path));

        save_project(&source_path, &source);
        save_project(&production_path, &production);

        let targets = linked_navigation_targets(&source, &source_path);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].status, LinkedProjectNavigationStatus::Ready);
        assert_eq!(targets[0].project_name.as_deref(), Some("Production 7C"));
        assert_eq!(ready_navigation_targets(&source, &source_path).len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn missing_and_nonreciprocal_links_are_not_openable() {
        let dir = temp_dir("blocked");
        let source_path = dir.join("source.shade");
        let missing_path = dir.join("missing.shade");
        let production_path = dir.join("production.shade");

        let mut source = ShadeProject::default();
        source.project_role = ProjectRole::Source;
        source.linked_projects.push(linked(ProjectRole::Production, &missing_path));
        source.linked_projects.push(linked(ProjectRole::Production, &production_path));

        let mut production = ShadeProject::default();
        production.project_role = ProjectRole::Production;
        save_project(&source_path, &source);
        save_project(&production_path, &production);

        let targets = linked_navigation_targets(&source, &source_path);
        assert_eq!(targets.len(), 2);
        assert!(targets.iter().any(|target| target.status == LinkedProjectNavigationStatus::Missing));
        assert!(targets.iter().any(|target| target.status == LinkedProjectNavigationStatus::ReciprocalLinkMissing));
        assert!(ready_navigation_targets(&source, &source_path).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn role_mismatch_and_duplicate_paths_fail_closed_or_deduplicate() {
        let dir = temp_dir("role");
        let source_path = dir.join("source.shade");
        let wrong_path = dir.join("wrong.shade");

        let mut source = ShadeProject::default();
        source.project_role = ProjectRole::Source;
        source.linked_projects.push(linked(ProjectRole::Production, &wrong_path));
        source.linked_projects.push(linked(ProjectRole::Production, &wrong_path));

        let mut wrong = ShadeProject::default();
        wrong.project_role = ProjectRole::Source;
        save_project(&source_path, &source);
        save_project(&wrong_path, &wrong);

        let targets = linked_navigation_targets(&source, &source_path);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].status, LinkedProjectNavigationStatus::RoleMismatch);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn standalone_projects_offer_no_cross_project_navigation() {
        let project = ShadeProject::default();
        assert!(linked_navigation_targets(&project, Path::new("standalone.shade")).is_empty());
    }
}
