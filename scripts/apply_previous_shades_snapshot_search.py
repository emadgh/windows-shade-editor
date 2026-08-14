from pathlib import Path


def replace_once(text: str, old: str, new: str, label: str) -> str:
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{label}: expected exactly one match, found {count}")
    return text.replace(old, new, 1)


# ---------------------------------------------------------------------------
# Previous Shades persistent cache + snapshot search index
# ---------------------------------------------------------------------------
path = Path("src/previous_shades.rs")
text = path.read_text(encoding="utf-8")

text = replace_once(
    text,
    "use crate::model::{FaceFileMetadata, ProjectThumbnail, ShadeProject};\n",
    "use crate::model::{FaceFileMetadata, ProjectThumbnail, ShadeProject};\n\nconst SNAPSHOT_CACHE_VERSION: u32 = 1;\n",
    "cache version constant",
)

old_entry = '''#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PreviousShadeEntry {
    pub path: String,
    pub project_name: String,
    pub last_opened_unix_ms: i64,
    pub saved_at_unix_ms: i64,
    pub open_count: u64,
}
'''
new_entry = '''#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct CachedSnapshot {
    pub id: u64,
    pub name: String,
    /// Effective code rendered by Test Code for this snapshot. When the project
    /// does not override Test Code text, this is the snapshot name itself.
    pub code: String,
    pub created_at_unix_ms: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PreviousShadeEntry {
    pub path: String,
    pub project_name: String,
    pub last_opened_unix_ms: i64,
    pub saved_at_unix_ms: i64,
    pub open_count: u64,
    /// Versioned local index so old Previous Shades JSON can be upgraded once
    /// without rereading every .shade file on each application start.
    pub snapshot_cache_version: u32,
    pub snapshots: Vec<CachedSnapshot>,
    pub test_code_text: String,
}
'''
text = replace_once(text, old_entry, new_entry, "PreviousShadeEntry")

old_default = '''            saved_at_unix_ms: 0,
            open_count: 0,
        }
    }
}
'''
new_default = '''            saved_at_unix_ms: 0,
            open_count: 0,
            snapshot_cache_version: 0,
            snapshots: Vec::new(),
            test_code_text: String::new(),
        }
    }
}
'''
text = replace_once(text, old_default, new_default, "PreviousShadeEntry default")

marker = '''impl PreviousShadesStore {
'''
entry_impl = r'''impl PreviousShadeEntry {
    fn refresh_from_project(&mut self, project: &ShadeProject) {
        self.project_name = project.name.trim().to_owned();
        self.test_code_text = project.test_code.text.trim().to_owned();
        let explicit_code = self.test_code_text.as_str();
        self.snapshots = project
            .snapshots
            .iter()
            .map(|snapshot| {
                let name = snapshot.name.trim().to_owned();
                CachedSnapshot {
                    id: snapshot.id,
                    code: if explicit_code.is_empty() {
                        name.clone()
                    } else {
                        explicit_code.to_owned()
                    },
                    name,
                    created_at_unix_ms: snapshot.created_at_unix_ms,
                }
            })
            .collect();
        self.snapshot_cache_version = SNAPSHOT_CACHE_VERSION;
    }

    pub fn matches_query(&self, query_lower: &str) -> bool {
        let query = query_lower.trim();
        if query.is_empty() {
            return true;
        }
        contains_case_insensitive(&self.project_name, query)
            || contains_case_insensitive(&self.path, query)
            || self.test_code_matches(query)
            || self.matching_snapshot(query).is_some()
    }

    pub fn matching_snapshot(&self, query_lower: &str) -> Option<&CachedSnapshot> {
        let query = query_lower.trim();
        if query.is_empty() {
            return None;
        }
        self.snapshots.iter().find(|snapshot| {
            contains_case_insensitive(&snapshot.name, query)
                || contains_case_insensitive(&snapshot.code, query)
                || snapshot_id_matches(snapshot.id, query)
        })
    }

    pub fn test_code_matches(&self, query_lower: &str) -> bool {
        !self.test_code_text.trim().is_empty()
            && contains_case_insensitive(&self.test_code_text, query_lower.trim())
    }
}

'''
text = replace_once(text, marker, entry_impl + marker, "PreviousShadeEntry impl insertion")

old_load = '''        store.sanitize();
        Ok(store)
    }
'''
new_load = '''        store.sanitize();
        // Cache migration is intentionally one-shot. Existing history entries
        // created before snapshot indexing are hydrated from the .shade file
        // only when the file is currently available.
        if store.refresh_stale_snapshot_cache() {
            let _ = store.save();
        }
        Ok(store)
    }
'''
text = replace_once(text, old_load, new_load, "cache migration on load")

start = text.index("    pub fn record_open(&mut self, path: &Path, project_name: &str) {")
end = text.index("    fn sanitize(&mut self) {", start)
new_record = r'''    pub fn record_open(&mut self, path: &Path, project_name: &str) {
        let normalized = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let display = normalized.to_string_lossy().into_owned();
        let now = unix_ms_now();
        let loaded_project = ShadeProject::load(path).ok();
        let saved_at = loaded_project
            .as_ref()
            .and_then(|project| {
                project
                    .file_metadata
                    .as_ref()
                    .map(|metadata| metadata.saved_at_unix_ms)
            })
            .unwrap_or_else(|| {
                fs::metadata(path)
                    .ok()
                    .and_then(|metadata| metadata.modified().ok())
                    .and_then(system_time_to_unix_ms)
                    .unwrap_or(0)
            });
        let cached_name = loaded_project
            .as_ref()
            .map(|project| project.name.trim())
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| project_name.trim())
            .to_owned();

        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| same_path(&entry.path, &display))
        {
            entry.path = display;
            entry.project_name = cached_name;
            entry.last_opened_unix_ms = now;
            entry.saved_at_unix_ms = saved_at;
            entry.open_count = entry.open_count.saturating_add(1).max(1);
            if let Some(project) = loaded_project.as_ref() {
                // This path is used after Open, Save and Quick Save, so every
                // successful project save immediately refreshes its snapshot
                // names/codes in the persistent Previous Shades cache.
                entry.refresh_from_project(project);
            }
        } else {
            let mut entry = PreviousShadeEntry {
                path: display,
                project_name: cached_name,
                last_opened_unix_ms: now,
                saved_at_unix_ms: saved_at,
                open_count: 1,
                ..PreviousShadeEntry::default()
            };
            if let Some(project) = loaded_project.as_ref() {
                entry.refresh_from_project(project);
            }
            self.entries.push(entry);
        }
    }

    fn refresh_stale_snapshot_cache(&mut self) -> bool {
        let mut changed = false;
        for entry in &mut self.entries {
            if entry.snapshot_cache_version >= SNAPSHOT_CACHE_VERSION {
                continue;
            }
            let path = Path::new(&entry.path);
            let Ok(project) = ShadeProject::load(path) else {
                continue;
            };
            entry.refresh_from_project(&project);
            if let Some(saved_at) = project
                .file_metadata
                .as_ref()
                .map(|metadata| metadata.saved_at_unix_ms)
                .filter(|saved_at| *saved_at > 0)
            {
                entry.saved_at_unix_ms = saved_at;
            }
            changed = true;
        }
        changed
    }

'''
text = text[:start] + new_record + text[end:]

old_sanitize_head = '''        for mut entry in self.entries.drain(..) {
            entry.path = entry.path.trim().to_owned();
            entry.project_name = entry.project_name.trim().to_owned();
            if entry.path.is_empty() {
'''
new_sanitize_head = '''        for mut entry in self.entries.drain(..) {
            entry.path = entry.path.trim().to_owned();
            entry.project_name = entry.project_name.trim().to_owned();
            entry.test_code_text = entry.test_code_text.trim().to_owned();
            for snapshot in &mut entry.snapshots {
                snapshot.name = snapshot.name.trim().to_owned();
                snapshot.code = snapshot.code.trim().to_owned();
            }
            if entry.path.is_empty() {
'''
text = replace_once(text, old_sanitize_head, new_sanitize_head, "sanitize cached snapshot fields")

old_dedupe = '''                    existing.last_opened_unix_ms = entry.last_opened_unix_ms;
                    existing.saved_at_unix_ms = entry.saved_at_unix_ms;
                }
                existing.open_count = existing.open_count.max(entry.open_count).max(1);
'''
new_dedupe = '''                    existing.last_opened_unix_ms = entry.last_opened_unix_ms;
                    existing.saved_at_unix_ms = entry.saved_at_unix_ms;
                    existing.snapshot_cache_version = entry.snapshot_cache_version;
                    existing.snapshots = entry.snapshots.clone();
                    existing.test_code_text = entry.test_code_text.clone();
                } else if entry.snapshot_cache_version > existing.snapshot_cache_version {
                    existing.snapshot_cache_version = entry.snapshot_cache_version;
                    existing.snapshots = entry.snapshots.clone();
                    existing.test_code_text = entry.test_code_text.clone();
                }
                existing.open_count = existing.open_count.max(entry.open_count).max(1);
'''
text = replace_once(text, old_dedupe, new_dedupe, "dedupe cached snapshot fields")

helper_marker = '''fn same_path(left: &str, right: &str) -> bool {
'''
helpers = r'''fn contains_case_insensitive(value: &str, query_lower: &str) -> bool {
    !query_lower.is_empty() && value.to_lowercase().contains(query_lower)
}

fn snapshot_id_matches(id: u64, query: &str) -> bool {
    let normalized = query.trim().strip_prefix('#').unwrap_or(query.trim());
    normalized.parse::<u64>().ok() == Some(id)
}

'''
text = replace_once(text, helper_marker, helpers + helper_marker, "search helpers")

old_tests = '''    #[test]
    fn sort_labels_are_stable() {
'''
new_tests = r'''    #[test]
    fn snapshot_cache_searches_name_code_and_id_and_refreshes_after_save() {
        let root = std::env::temp_dir().join(format!(
            "shade-editor-previous-shades-{}",
            unix_ms_now()
        ));
        fs::create_dir_all(&root).unwrap();
        let shade_path = root.join("cache-test.shade");

        let mut project = ShadeProject::default();
        project.name = "Floor Tile 24".to_owned();
        let snapshot_id = project.create_snapshot();
        project.rename_snapshot(snapshot_id, "Kiln-A-042").unwrap();
        project.test_code.text = "TC-042".to_owned();
        project.save(&shade_path, &[]).unwrap();

        let mut store = PreviousShadesStore::default();
        store.record_open(&shade_path, "fallback");
        let entry = &store.entries()[0];
        assert_eq!(entry.snapshot_cache_version, SNAPSHOT_CACHE_VERSION);
        assert_eq!(entry.snapshots.len(), 1);
        assert_eq!(entry.snapshots[0].name, "Kiln-A-042");
        assert_eq!(entry.snapshots[0].code, "TC-042");
        assert!(entry.matches_query("kiln-a-042"));
        assert!(entry.matches_query("tc-042"));
        assert!(entry.matches_query(&format!("#{snapshot_id}")));

        project.rename_snapshot(snapshot_id, "Kiln-A-043").unwrap();
        project.test_code.text = "TC-043".to_owned();
        project.save(&shade_path, &[]).unwrap();
        store.record_open(&shade_path, &project.name);
        let entry = &store.entries()[0];
        assert!(!entry.matches_query("kiln-a-042"));
        assert!(!entry.matches_query("tc-042"));
        assert!(entry.matches_query("kiln-a-043"));
        assert!(entry.matches_query("tc-043"));

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sort_labels_are_stable() {
'''
text = replace_once(text, old_tests, new_tests, "snapshot cache tests")
path.write_text(text, encoding="utf-8")


# ---------------------------------------------------------------------------
# Previous Shades UI: search cached snapshot names/codes and show match detail
# ---------------------------------------------------------------------------
path = Path("src/app_main.rs")
text = path.read_text(encoding="utf-8")

text = replace_once(
    text,
    '.hint_text("Search project name or path...")',
    '.hint_text("Search project, path, snapshot name or test code...")',
    "Previous Shades search hint",
)

old_filter = '''        if !query.is_empty() {
            rows.retain(|entry| {
                entry.project_name.to_lowercase().contains(&query)
                    || entry.path.to_lowercase().contains(&query)
            });
        }
'''
new_filter = '''        if !query.is_empty() {
            rows.retain(|entry| entry.matches_query(&query));
        }
'''
text = replace_once(text, old_filter, new_filter, "Previous Shades cached search filter")

old_help = '''        ui.small("All .shade projects opened or saved by Shade Editor are retained here. Missing paths stay in history so moved/offline projects are visible.");
'''
new_help = '''        ui.small("All .shade projects opened or saved by Shade Editor are retained here. Search uses the local cache for project/path plus Snapshot names, Snapshot IDs and effective Test Code values; missing paths stay visible.");
'''
text = replace_once(text, old_help, new_help, "Previous Shades cache help")

old_row = '''                        let opened = format_previous_shade_time(entry.last_opened_unix_ms);
                        let selected = self.previous_shades_selected.as_deref()
                            == Some(entry.path.as_str());
                        if clickable_row(ui, selected, &label, Some(&opened), None, 38.0)
                            .on_hover_text(&entry.path)
                            .clicked()
'''
new_row = '''                        let opened = format_previous_shade_time(entry.last_opened_unix_ms);
                        let match_detail = if query.is_empty() {
                            None
                        } else {
                            entry
                                .matching_snapshot(&query)
                                .map(|snapshot| {
                                    if snapshot.code.trim().is_empty()
                                        || snapshot.code.eq_ignore_ascii_case(&snapshot.name)
                                    {
                                        format!("Snapshot: {} · #{}", snapshot.name, snapshot.id)
                                    } else {
                                        format!("Snapshot: {} · code {}", snapshot.name, snapshot.code)
                                    }
                                })
                                .or_else(|| {
                                    entry.test_code_matches(&query).then(|| {
                                        format!("Test code: {}", entry.test_code_text)
                                    })
                                })
                        };
                        let detail = match_detail.as_deref().unwrap_or(&opened);
                        let selected = self.previous_shades_selected.as_deref()
                            == Some(entry.path.as_str());
                        if clickable_row(ui, selected, &label, Some(detail), None, 38.0)
                            .on_hover_text(&entry.path)
                            .clicked()
'''
text = replace_once(text, old_row, new_row, "Previous Shades matching snapshot row detail")
path.write_text(text, encoding="utf-8")


# ---------------------------------------------------------------------------
# Version + release notes
# ---------------------------------------------------------------------------
path = Path("Cargo.toml")
text = path.read_text(encoding="utf-8")
text = replace_once(text, 'version = "0.13.0"', 'version = "0.13.1"', "Cargo.toml version")
path.write_text(text, encoding="utf-8")

path = Path("Cargo.lock")
text = path.read_text(encoding="utf-8")
old_lock = 'name = "windows-shade-editor"\nversion = "0.13.0"'
new_lock = 'name = "windows-shade-editor"\nversion = "0.13.1"'
text = replace_once(text, old_lock, new_lock, "Cargo.lock package version")
path.write_text(text, encoding="utf-8")

path = Path("RELEASE_NOTES.md")
text = path.read_text(encoding="utf-8")
notes = '''# Shade Editor 0.13.1

- Index Snapshot names, Snapshot IDs and effective Test Code values in the persistent Previous Shades cache.
- Previous Shades search can now find projects by a specific Snapshot/Test code without reopening `.shade` files during search.
- Existing Previous Shades history is migrated once when cached Snapshot metadata is missing and the `.shade` file is available.
- Opening, Save and Quick Save refresh the cached Snapshot/Test index immediately so renamed/new tests become searchable at once.
- Search results show the matching Snapshot name/code when a Snapshot term produced the match.

'''
if not text.startswith("# Shade Editor 0.13.0\n"):
    raise SystemExit("RELEASE_NOTES.md: unexpected current release heading")
path.write_text(notes + text, encoding="utf-8")
