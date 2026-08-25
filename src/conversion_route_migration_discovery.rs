use std::fs;
use std::path::{Path, PathBuf};

use crate::conversion_route_migration::RouteMigrationPlan;
use crate::conversion_route_migration_executor::{
    RouteMigrationRecoveryJournal, initialize_route_migration_journal,
    load_route_migration_journal,
};

/// Discover an unfinished route-migration journal that owns `production_project_path`.
///
/// Journals intentionally live next to the Production project so recovery does not depend on a
/// machine-global queue database. Multiple matching journals are treated as ambiguous/destructive
/// state and fail closed.
pub fn discover_pending_route_migration(
    production_project_path: &Path,
) -> Result<Option<RouteMigrationRecoveryJournal>, String> {
    let parent = production_project_path.parent().unwrap_or_else(|| Path::new("."));
    if !parent.exists() {
        return Ok(None);
    }

    let mut matching = Vec::new();
    let entries = fs::read_dir(parent).map_err(|error| {
        format!(
            "Cannot scan Production folder {} for route-migration recovery journals: {error}",
            parent.display()
        )
    })?;
    for entry in entries {
        let entry = entry.map_err(|error| {
            format!(
                "Cannot inspect Production folder {} for route-migration recovery: {error}",
                parent.display()
            )
        })?;
        let path = entry.path();
        if !is_route_migration_journal_filename(&path) {
            continue;
        }
        let journal = load_route_migration_journal(&path).map_err(|error| {
            format!(
                "Found an unreadable Shade Editor route-migration journal {}: {error}",
                path.display()
            )
        })?;
        if paths_match(
            &journal.plan.production_project_path,
            production_project_path,
        ) {
            matching.push((path, journal));
        }
    }

    match matching.len() {
        0 => Ok(None),
        1 => Ok(matching.pop().map(|(_, journal)| journal)),
        count => Err(format!(
            "Found {count} route-migration recovery journals for {}. Automatic destructive recovery is blocked until the duplicate journal state is resolved.",
            production_project_path.display()
        )),
    }
}

/// Start a new migration only when the destination has no unfinished journal. If one exists, the
/// caller must resume that exact immutable journal instead of replacing it with a newly captured
/// plan.
pub fn initialize_route_migration_if_clear(
    plan: RouteMigrationPlan,
) -> Result<RouteMigrationRecoveryJournal, String> {
    if discover_pending_route_migration(&plan.production_project_path)?.is_some() {
        return Err(format!(
            "An unfinished route migration already exists for {}. Resume its recovery journal before capturing another migration.",
            plan.production_project_path.display()
        ));
    }
    initialize_route_migration_journal(plan)
}

pub fn pending_route_migration_journal_paths(
    production_project_path: &Path,
) -> Result<Vec<PathBuf>, String> {
    let parent = production_project_path.parent().unwrap_or_else(|| Path::new("."));
    if !parent.exists() {
        return Ok(Vec::new());
    }
    let mut result = Vec::new();
    for entry in fs::read_dir(parent).map_err(|error| {
        format!(
            "Cannot scan Production folder {} for route-migration journals: {error}",
            parent.display()
        )
    })? {
        let path = entry.map_err(|error| error.to_string())?.path();
        if !is_route_migration_journal_filename(&path) {
            continue;
        }
        if let Ok(journal) = load_route_migration_journal(&path) {
            if paths_match(
                &journal.plan.production_project_path,
                production_project_path,
            ) {
                result.push(path);
            }
        }
    }
    result.sort_by_key(|path| path.to_string_lossy().to_ascii_lowercase());
    Ok(result)
}

fn is_route_migration_journal_filename(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    name.starts_with(".shade-migrate-") && name.ends_with("-journal.json")
}

fn paths_match(left: &Path, right: &Path) -> bool {
    path_key(left) == path_key(right)
}

fn path_key(path: &Path) -> String {
    path.to_string_lossy()
        .trim()
        .replace('/', "\\")
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn journal_filename_filter_is_strict() {
        assert!(is_route_migration_journal_filename(Path::new(
            ".shade-migrate-abcdef-journal.json"
        )));
        assert!(!is_route_migration_journal_filename(Path::new(
            ".shade-migrate-abcdef-stage-0000.tif"
        )));
        assert!(!is_route_migration_journal_filename(Path::new(
            "shade-migrate-abcdef-journal.json"
        )));
    }
}
