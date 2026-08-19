from pathlib import Path

workspace = Path(__file__).resolve().parents[2]
repo = workspace / "work"
compat = repo / "src" / "production_project_compat.rs"
text = compat.read_text(encoding="utf-8")

text = text.replace("use std::path::Path;", "use std::path::{Path, PathBuf};", 1)

start = text.index("pub fn validate_existing_production_project_for_append(")
end = text.index("\n/// Append one already-committed", start)
new_validation = r'''pub fn validate_existing_production_project_for_append(
    project: &ShadeProject,
    source_project_path: &Path,
    incoming: &ProductionProvenance,
) -> Result<ProductionCompatibilityKey, String> {
    let resolved_face_paths = project
        .faces
        .iter()
        .map(|face| PathBuf::from(&face.path))
        .collect::<Vec<_>>();
    validate_existing_production_project_for_append_with_resolved_paths(
        project,
        &resolved_face_paths,
        source_project_path,
        incoming,
    )
}

/// Validate a Production project loaded from disk. Face paths stored in `.shade`
/// are portable and therefore must be resolved relative to the exact project
/// path before comparing them with immutable provenance output identities.
pub fn validate_existing_production_project_for_append_at_path(
    project: &ShadeProject,
    production_project_path: &Path,
    source_project_path: &Path,
    incoming: &ProductionProvenance,
) -> Result<ProductionCompatibilityKey, String> {
    let resolved_face_paths = project.resolve_face_paths(production_project_path);
    validate_existing_production_project_for_append_with_resolved_paths(
        project,
        &resolved_face_paths,
        source_project_path,
        incoming,
    )
}

fn validate_existing_production_project_for_append_with_resolved_paths(
    project: &ShadeProject,
    resolved_face_paths: &[PathBuf],
    source_project_path: &Path,
    incoming: &ProductionProvenance,
) -> Result<ProductionCompatibilityKey, String> {
    if project.project_role != ProjectRole::Production {
        return Err("Append destination is not a Production project.".to_owned());
    }
    if source_project_path.as_os_str().is_empty() {
        return Err("Source project path cannot be empty for Production append.".to_owned());
    }
    if project.faces.is_empty() || project.production_provenance.is_empty() {
        return Err(
            "Existing Production project has no committed Face/provenance baseline to validate."
                .to_owned(),
        );
    }
    if project.faces.len() != project.production_provenance.len() {
        return Err(format!(
            "Existing Production project has {} Faces but {} provenance records; append is blocked until lineage is repaired.",
            project.faces.len(),
            project.production_provenance.len()
        ));
    }
    if resolved_face_paths.len() != project.faces.len() {
        return Err(format!(
            "Existing Production project resolved {} Face paths for {} Faces; append is blocked until path state is repaired.",
            resolved_face_paths.len(),
            project.faces.len()
        ));
    }
    if !project.linked_projects.iter().any(|link| {
        link.role == ProjectRole::Source && paths_match(&link.path, source_project_path)
    }) {
        return Err(format!(
            "Existing Production project is not linked to Source project {}.",
            source_project_path.display()
        ));
    }
    if !paths_match(&incoming.source.source_project_path, source_project_path) {
        return Err(
            "Incoming conversion provenance references a different Source project.".to_owned(),
        );
    }

    let canonical = ProductionCompatibilityKey::from_provenance(
        project
            .production_provenance
            .first()
            .expect("non-empty provenance checked above"),
    )?;

    for (index, (provenance, resolved_face_path)) in project
        .production_provenance
        .iter()
        .zip(resolved_face_paths.iter())
        .enumerate()
    {
        if !paths_match(&provenance.output_path, resolved_face_path) {
            return Err(format!(
                "Existing Production Face {} does not match its persisted provenance output path.",
                index + 1
            ));
        }
        let key = ProductionCompatibilityKey::from_provenance(provenance)?;
        if key != canonical {
            return Err(format!(
                "Existing Production project already contains incompatible target provenance at Face {}: {}",
                index + 1,
                describe_key_mismatch(&canonical, &key)
            ));
        }
    }

    for channel in &canonical.channel_names {
        if !project.adjustments.contains_key(channel) {
            return Err(format!(
                "Existing Production project is missing target channel adjustment state for '{channel}'."
            ));
        }
    }

    let incoming_key = ProductionCompatibilityKey::from_provenance(incoming)?;
    if incoming_key != canonical {
        return Err(format!(
            "Incoming converted Face is incompatible with the existing Production target: {}",
            describe_key_mismatch(&canonical, &incoming_key)
        ));
    }

    if resolved_face_paths
        .iter()
        .any(|path| paths_match(&incoming.output_path, path))
        || project
            .production_provenance
            .iter()
            .any(|provenance| paths_match(&provenance.output_path, Path::new(&incoming.output_path)))
    {
        return Err(
            "Incoming converted TIFF is already present in the Production project.".to_owned(),
        );
    }

    Ok(canonical)
}
'''
text = text[:start] + new_validation + text[end:]

append_start = text.index("pub fn append_converted_face_to_production_project(")
append_end = text.index("\nfn describe_key_mismatch", append_start)
new_append = r'''pub fn append_converted_face_to_production_project(
    project: &mut ShadeProject,
    spec: AppendConvertedFaceSpec<'_>,
) -> Result<(), String> {
    validate_append_spec(&spec)?;
    validate_existing_production_project_for_append(
        project,
        spec.source_project_path,
        &spec.provenance,
    )?;
    push_converted_face(project, spec);
    Ok(())
}

/// Path-aware append for a Production project loaded from disk. This keeps
/// portable Face paths valid while preserving exact provenance/output checks.
pub fn append_converted_face_to_production_project_at_path(
    project: &mut ShadeProject,
    production_project_path: &Path,
    spec: AppendConvertedFaceSpec<'_>,
) -> Result<(), String> {
    validate_append_spec(&spec)?;
    validate_existing_production_project_for_append_at_path(
        project,
        production_project_path,
        spec.source_project_path,
        &spec.provenance,
    )?;
    push_converted_face(project, spec);
    Ok(())
}

fn validate_append_spec(spec: &AppendConvertedFaceSpec<'_>) -> Result<(), String> {
    if spec.output_face_label.trim().is_empty() {
        return Err("Production Face label cannot be empty.".to_owned());
    }
    if spec.provenance.output_path.trim().is_empty() {
        return Err("Production provenance output path cannot be empty.".to_owned());
    }
    Ok(())
}

fn push_converted_face(project: &mut ShadeProject, spec: AppendConvertedFaceSpec<'_>) {
    project.faces.push(FaceRef {
        path: spec.provenance.output_path.clone(),
        label: spec.output_face_label.trim().to_owned(),
        status: FaceStatus::Accepted,
        production_source_profile: None,
    });
    project.production_provenance.push(spec.provenance);
}
'''
text = text[:append_start] + new_append + text[append_end:]

# Add a real save/reload regression before the test module's final brace.
test_insert = r'''

    #[test]
    fn saved_and_reloaded_portable_face_path_validates_and_appends() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let project_path = std::env::temp_dir().join(format!(
            "shade-production-append-portable-{}-{stamp}.shade",
            std::process::id()
        ));
        let output_one = project_path.with_file_name(format!(
            "shade-production-face-1-{}-{stamp}.tif",
            std::process::id()
        ));
        let output_two = project_path.with_file_name(format!(
            "shade-production-face-2-{}-{stamp}.tif",
            std::process::id()
        ));
        let project = build_production_project(ProductionProjectSpec {
            project_name: "Production",
            source_project_path: Path::new(r"C:\Design\Source.shade"),
            output_tiff_path: &output_one,
            output_face_label: "Face 1",
            provenance: provenance(&output_one, r"C:\Design\Face-1.png", 'a'),
        })
        .unwrap();
        project
            .save_new(&project_path, std::slice::from_ref(&output_one))
            .unwrap();

        let mut loaded = ShadeProject::load(&project_path).unwrap();
        assert!(
            !Path::new(&loaded.faces[0].path).is_absolute(),
            "saved Production Face should use a portable relative path"
        );
        let incoming = provenance(&output_two, r"C:\Design\Face-2.jpg", 'b');
        validate_existing_production_project_for_append_at_path(
            &loaded,
            &project_path,
            Path::new(r"C:\Design\Source.shade"),
            &incoming,
        )
        .unwrap();
        append_converted_face_to_production_project_at_path(
            &mut loaded,
            &project_path,
            AppendConvertedFaceSpec {
                source_project_path: Path::new(r"C:\Design\Source.shade"),
                output_face_label: "Face 2",
                provenance: incoming,
            },
        )
        .unwrap();
        assert_eq!(loaded.faces.len(), 2);
        assert_eq!(loaded.production_provenance.len(), 2);
        let _ = std::fs::remove_file(project_path);
    }
'''
pos = text.rfind("\n}")
if pos == -1:
    raise SystemExit("production compat test module closing brace not found")
text = text[:pos] + test_insert + text[pos:]
compat.write_text(text, encoding="utf-8", newline="\n")

adapter = repo / "src" / "conversion_transaction_disposition.rs"
text = adapter.read_text(encoding="utf-8")
text = text.replace(
    "AppendConvertedFaceSpec, append_converted_face_to_production_project,\n    validate_existing_production_project_for_append,",
    "AppendConvertedFaceSpec, append_converted_face_to_production_project_at_path,\n    validate_existing_production_project_for_append_at_path,",
    1,
)
text = text.replace(
    "validate_existing_production_project_for_append(\n                    &loaded.project,\n                    self.source_project_path,",
    "validate_existing_production_project_for_append_at_path(\n                    &loaded.project,\n                    path,\n                    self.source_project_path,",
    1,
)
text = text.replace(
    "append_converted_face_to_production_project(\n                    &mut appended,\n                    AppendConvertedFaceSpec {",
    "append_converted_face_to_production_project_at_path(\n                    &mut appended,\n                    path,\n                    AppendConvertedFaceSpec {",
    1,
)
adapter.write_text(text, encoding="utf-8", newline="\n")
