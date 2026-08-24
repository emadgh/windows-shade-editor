from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one anchor, found {count}: {old[:100]!r}")
    p.write_text(text.replace(old, new, 1), encoding="utf-8", newline="\n")


def replace_between(path: str, start: str, end: str, replacement: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    i = text.index(start)
    j = text.index(end, i)
    p.write_text(text[:i] + replacement + text[j:], encoding="utf-8", newline="\n")


# 1) Persist an explicit existing-route disposition, distinct from append-only legacy behavior.
replace_once(
    "src/production_project_disposition.rs",
    '''    AppendExisting {
        expected_project_sha256: String,
        expected_compatibility: CapturedProductionCompatibilityKey,
    },
''',
    '''    AppendExisting {
        expected_project_sha256: String,
        expected_compatibility: CapturedProductionCompatibilityKey,
    },
    UpdateExistingRoute {
        expected_project_sha256: String,
        expected_compatibility: CapturedProductionCompatibilityKey,
        route_policy_sha256: String,
        allow_production_work_discard: bool,
    },
''',
)
replace_once(
    "src/production_project_disposition.rs",
    '''    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::CreateNew => Ok(()),
            Self::AppendExisting {
                expected_project_sha256,
                expected_compatibility,
            } => {
                if !is_bare_sha256(expected_project_sha256) {
                    return Err(
                        "Append-existing Production capture requires canonical lowercase project SHA-256."
                            .to_owned(),
                    );
                }
                expected_compatibility.validate()
            }
        }
    }
''',
    '''    pub fn update_existing_route(
        expected_project_sha256: impl Into<String>,
        compatibility: &ProductionCompatibilityKey,
        route_policy_sha256: impl Into<String>,
        allow_production_work_discard: bool,
    ) -> Result<Self, String> {
        let disposition = Self::UpdateExistingRoute {
            expected_project_sha256: expected_project_sha256.into(),
            expected_compatibility: CapturedProductionCompatibilityKey::from_runtime(compatibility),
            route_policy_sha256: route_policy_sha256.into(),
            allow_production_work_discard,
        };
        disposition.validate()?;
        Ok(disposition)
    }

    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::CreateNew => Ok(()),
            Self::AppendExisting {
                expected_project_sha256,
                expected_compatibility,
            } => {
                if !is_bare_sha256(expected_project_sha256) {
                    return Err(
                        "Append-existing Production capture requires canonical lowercase project SHA-256."
                            .to_owned(),
                    );
                }
                expected_compatibility.validate()
            }
            Self::UpdateExistingRoute {
                expected_project_sha256,
                expected_compatibility,
                route_policy_sha256,
                ..
            } => {
                if !is_bare_sha256(expected_project_sha256) {
                    return Err(
                        "Existing-route update requires canonical lowercase project SHA-256."
                            .to_owned(),
                    );
                }
                if !is_bare_sha256(route_policy_sha256) {
                    return Err(
                        "Existing-route update requires canonical lowercase route-policy SHA-256."
                            .to_owned(),
                    );
                }
                expected_compatibility.validate()
            }
        }
    }
''',
)
# Add one domain regression test before final test-module brace.
p = Path("src/production_project_disposition.rs")
text = p.read_text(encoding="utf-8")
needle = '''    #[test]
    fn compatibility_snapshot_preserves_channel_order() {
        let key = runtime_key();
        let mut captured = CapturedProductionCompatibilityKey::from_runtime(&key);
        captured.channel_names.swap(0, 1);
        assert!(!captured.matches_runtime(&key));
    }
'''
addition = needle + '''
    #[test]
    fn route_update_freezes_project_target_policy_and_confirmation_intent() {
        let key = runtime_key();
        let disposition = ProductionProjectDisposition::update_existing_route(
            "b".repeat(64),
            &key,
            "c".repeat(64),
            true,
        )
        .unwrap();
        let json = serde_json::to_string(&disposition).unwrap();
        let restored: ProductionProjectDisposition = serde_json::from_str(&json).unwrap();
        assert_eq!(disposition, restored);
        assert!(matches!(
            restored,
            ProductionProjectDisposition::UpdateExistingRoute {
                allow_production_work_discard: true,
                ..
            }
        ));
    }
'''
if text.count(needle) != 1:
    raise SystemExit("production_project_disposition test anchor mismatch")
p.write_text(text.replace(needle, addition, 1), encoding="utf-8", newline="\n")

# 2) Subsequent batch Faces refresh the optimistic project SHA while preserving route-update intent.
replace_once(
    "src/conversion_batch_execution.rs",
    '''    ProductionProjectDisposition::append_existing(loaded.file_sha256, &compatibility)
}
''',
    '''    match &batch.production_project_disposition {
        ProductionProjectDisposition::UpdateExistingRoute {
            route_policy_sha256,
            allow_production_work_discard,
            ..
        } => ProductionProjectDisposition::update_existing_route(
            loaded.file_sha256,
            &compatibility,
            route_policy_sha256.clone(),
            *allow_production_work_discard,
        ),
        ProductionProjectDisposition::CreateNew
        | ProductionProjectDisposition::AppendExisting { .. } => {
            ProductionProjectDisposition::append_existing(loaded.file_sha256, &compatibility)
        }
    }
}
''',
)

# 3) Route-owned output replacement is verified before the TIFF commit, then the Production Face
#    is replaced in-place (or appended when this Source Face has never been committed on the route).
replace_once(
    "src/conversion_transaction_disposition.rs",
    '''use crate::icc_conversion_worker::{FilesystemIccConversionBackend, sha256_file};
use crate::model::ShadeProject;
''',
    '''use crate::conversion_batch::batch_recipe_policy_sha256;
use crate::icc_conversion_worker::{FilesystemIccConversionBackend, sha256_file};
use crate::model::ShadeProject;
use crate::production_replacement::prepare_production_replacement_plan;
use crate::reconversion_policy::analyze_replacement_risk;
''',
)
replace_once(
    "src/conversion_transaction_disposition.rs",
    '''    appended_project: Option<ShadeProject>,
}
''',
    '''    appended_project: Option<ShadeProject>,
    prepared_existing: Option<LoadedExistingProductionProject>,
    prepared_replace_index: Option<usize>,
}
''',
)
replace_once(
    "src/conversion_transaction_disposition.rs",
    '''    ) -> Result<CommittedConversionOutput, String> {
        self.inner
            .render_convert_and_commit(capture, cancellation, report)
    }
''',
    '''    ) -> Result<CommittedConversionOutput, String> {
        if let ProductionProjectDisposition::UpdateExistingRoute {
            expected_project_sha256,
            expected_compatibility,
            route_policy_sha256,
            allow_production_work_discard,
        } = self.disposition
        {
            if capture.output_policy
                != crate::conversion_transaction::CapturedOutputPolicy::TransactionalReplace
            {
                return Err(
                    "Existing-route conversion must use transactional output replacement policy."
                        .to_owned(),
                );
            }
            let loaded = self.inner.load_existing_production_project(&capture.production_project_path)?;
            if !loaded
                .file_sha256
                .eq_ignore_ascii_case(expected_project_sha256.trim())
            {
                return Err(
                    "Existing Production project changed after the route update was captured."
                        .to_owned(),
                );
            }
            let compatibility = validate_existing_production_project_baseline_at_path(
                &loaded.project,
                &capture.production_project_path,
                self.source_project_path,
            )?;
            if !expected_compatibility.matches_runtime(&compatibility) {
                return Err(
                    "Existing Production route target compatibility changed after capture."
                        .to_owned(),
                );
            }
            let incoming_policy = batch_recipe_policy_sha256(&capture.conversion_recipe)?;
            if !incoming_policy.eq_ignore_ascii_case(route_policy_sha256.trim()) {
                return Err(
                    "Captured conversion settings no longer match the selected Production route."
                        .to_owned(),
                );
            }

            let matching = loaded
                .project
                .production_provenance
                .iter()
                .enumerate()
                .filter(|(_, provenance)| {
                    paths_match_str(
                        &provenance.source.source_project_path,
                        &capture.source_project_path.to_string_lossy(),
                    ) && paths_match_str(
                        &provenance.source.source_face_path,
                        &capture.source_face_path.to_string_lossy(),
                    )
                })
                .collect::<Vec<_>>();
            if matching.len() > 1 {
                return Err(
                    "Selected Production route contains duplicate provenance for this Source Face."
                        .to_owned(),
                );
            }
            let replace_index = if let Some((index, previous)) = matching.first().copied() {
                let previous_policy = batch_recipe_policy_sha256(&previous.recipe)?;
                if !previous_policy.eq_ignore_ascii_case(route_policy_sha256.trim()) {
                    return Err(
                        "Existing output belongs to a different conversion route; overwrite is blocked."
                            .to_owned(),
                    );
                }
                if !paths_match(
                    Path::new(&previous.output_path),
                    &capture.output_tiff_path,
                ) {
                    return Err(
                        "Same-route Source Face maps to a different recorded TIFF path; overwrite is blocked."
                            .to_owned(),
                    );
                }
                if capture.output_tiff_path.exists() {
                    let actual = sha256_file(&capture.output_tiff_path)?;
                    if !actual.eq_ignore_ascii_case(previous.output_sha256.trim()) {
                        return Err(
                            "Existing Production TIFF bytes no longer match route provenance; automatic overwrite is blocked."
                                .to_owned(),
                        );
                    }
                }
                let risk = analyze_replacement_risk(
                    &loaded.project,
                    &capture.production_project_path,
                    previous,
                )?;
                if risk.requires_explicit_confirmation && !*allow_production_work_discard {
                    return Err(risk.warning.unwrap_or_else(|| {
                        "Production-side work requires explicit replacement confirmation.".to_owned()
                    }));
                }
                Some(index)
            } else {
                if capture.output_tiff_path.exists() {
                    return Err(
                        "Deterministic TIFF path already exists but is not owned by this Source Face + conversion route."
                            .to_owned(),
                    );
                }
                None
            };
            self.prepared_existing = Some(loaded);
            self.prepared_replace_index = replace_index;
        }
        self.inner
            .render_convert_and_commit(capture, cancellation, report)
    }
''',
)
# Replace save match with three-way handling.
start = '''        match self.disposition {
            ProductionProjectDisposition::CreateNew => {
'''
end = '''    }
}

/// Execute one conversion with explicit Production-project destination intent.
'''
replacement = '''        match self.disposition {
            ProductionProjectDisposition::CreateNew => {
                self.inner.save_production_project(path, generated_project)
            }
            ProductionProjectDisposition::AppendExisting {
                expected_project_sha256,
                expected_compatibility,
            } => {
                if generated_project.faces.len() != 1
                    || generated_project.production_provenance.len() != 1
                {
                    return Err(
                        "Append transaction expected exactly one newly converted Face/provenance pair."
                            .to_owned(),
                    );
                }
                let loaded = self.inner.load_existing_production_project(path)?;
                if !loaded
                    .file_sha256
                    .eq_ignore_ascii_case(expected_project_sha256.trim())
                {
                    return Err(
                        "Existing Production project SHA-256 changed after the conversion job was captured."
                            .to_owned(),
                    );
                }

                let incoming = generated_project.production_provenance[0].clone();
                let compatibility = validate_existing_production_project_for_append_at_path(
                    &loaded.project,
                    path,
                    self.source_project_path,
                    &incoming,
                )?;
                if !expected_compatibility.matches_runtime(&compatibility) {
                    return Err(
                        "Existing Production project target compatibility changed after the conversion job was captured."
                            .to_owned(),
                    );
                }

                let mut appended = loaded.project;
                append_converted_face_to_production_project_at_path(
                    &mut appended,
                    path,
                    AppendConvertedFaceSpec {
                        source_project_path: self.source_project_path,
                        output_face_label: &generated_project.faces[0].label,
                        provenance: incoming,
                    },
                )?;
                self.appended_project = Some(appended.clone());
                self.inner.save_existing_production_project(
                    path,
                    expected_project_sha256,
                    &appended,
                )
            }
            ProductionProjectDisposition::UpdateExistingRoute {
                expected_project_sha256,
                allow_production_work_discard,
                ..
            } => {
                if generated_project.faces.len() != 1
                    || generated_project.production_provenance.len() != 1
                {
                    return Err(
                        "Existing-route update expected exactly one converted Face/provenance pair."
                            .to_owned(),
                    );
                }
                let loaded = self.prepared_existing.take().ok_or_else(|| {
                    "Existing-route ownership was not prepared before output commit.".to_owned()
                })?;
                let mut updated = loaded.project;
                let incoming = generated_project.production_provenance[0].clone();
                if let Some(index) = self.prepared_replace_index.take() {
                    let previous = updated
                        .production_provenance
                        .get(index)
                        .cloned()
                        .ok_or_else(|| "Prepared route replacement Face disappeared.".to_owned())?;
                    let plan = prepare_production_replacement_plan(
                        &updated,
                        path,
                        &previous,
                        incoming.clone(),
                    )?;
                    if plan.risk.requires_explicit_confirmation && !*allow_production_work_discard {
                        return Err(plan.risk.warning.unwrap_or_else(|| {
                            "Production-side work requires explicit replacement confirmation."
                                .to_owned()
                        }));
                    }
                    updated.faces[index] = generated_project.faces[0].clone();
                    updated.production_provenance[index] = incoming;
                } else {
                    append_converted_face_to_production_project_at_path(
                        &mut updated,
                        path,
                        AppendConvertedFaceSpec {
                            source_project_path: self.source_project_path,
                            output_face_label: &generated_project.faces[0].label,
                            provenance: incoming,
                        },
                    )?;
                }
                self.appended_project = Some(updated.clone());
                self.inner.save_existing_production_project(
                    path,
                    expected_project_sha256,
                    &updated,
                )
            }
        }
'''
replace_between("src/conversion_transaction_disposition.rs", start, end, replacement + end)
replace_once(
    "src/conversion_transaction_disposition.rs",
    '''    let is_append = matches!(
        disposition,
        ProductionProjectDisposition::AppendExisting { .. }
    );
    let mut adapter = ProductionDispositionBackend {
        inner: backend,
        disposition,
        source_project_path: &capture.source_project_path,
        appended_project: None,
    };
''',
    '''    let uses_existing_project = matches!(
        disposition,
        ProductionProjectDisposition::AppendExisting { .. }
            | ProductionProjectDisposition::UpdateExistingRoute { .. }
    );
    let mut adapter = ProductionDispositionBackend {
        inner: backend,
        disposition,
        source_project_path: &capture.source_project_path,
        appended_project: None,
        prepared_existing: None,
        prepared_replace_index: None,
    };
''',
)
replace_once(
    "src/conversion_transaction_disposition.rs",
    '''    if !is_append {
        return outcome;
    }
''',
    '''    if !uses_existing_project {
        return outcome;
    }
''',
)
# Path helpers used by pre-commit ownership checks.
replace_once(
    "src/conversion_transaction_disposition.rs",
    '''#[cfg(test)]
mod tests {
''',
    '''fn paths_match(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .replace('/', "\\\\")
        .eq_ignore_ascii_case(&right.to_string_lossy().replace('/', "\\\\"))
}

fn paths_match_str(left: &str, right: &str) -> bool {
    left.trim()
        .replace('/', "\\\\")
        .eq_ignore_ascii_case(&right.trim().replace('/', "\\\\"))
}

#[cfg(test)]
mod tests {
''',
)

# 4) MustNotExist protects the TIFF itself. Existing Production-project intent is handled by the
#    disposition adapter; rejecting the .shade here made legitimate append-existing jobs fail.
replace_once(
    "src/icc_conversion_worker.rs",
    '''        if !self.replace_existing
            && (capture.output_tiff_path.exists() || capture.production_project_path.exists())
        {
            return Err(
                "Queued versioned conversion destination is no longer free; review and queue a new version."
                    .to_owned(),
            );
        }
''',
    '''        if !self.replace_existing && capture.output_tiff_path.exists() {
            return Err(
                "Queued conversion TIFF destination is no longer free; review route ownership and queue again."
                    .to_owned(),
            );
        }
''',
)

print("issue #373 route transaction patch applied")
