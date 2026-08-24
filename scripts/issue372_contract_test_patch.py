from pathlib import Path


def replace_once(path: str, old: str, new: str) -> None:
    p = Path(path)
    text = p.read_text(encoding="utf-8")
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected exactly one match, found {count}\n--- OLD ---\n{old}")
    p.write_text(text.replace(old, new, 1), encoding="utf-8", newline="\n")
    print(f"patched {path}")


replace_once(
    "src/ui/color_conversion.rs",
    '''        let source = include_str!("color_conversion.rs");
        for required in [''',
    '''        let source = include_str!("color_conversion.rs");
        let runtime = source.split("\\n#[cfg(test)]").next().unwrap_or(source);
        for required in [''',
)
replace_once(
    "src/ui/color_conversion.rs",
    '''            assert!(source.contains(required), "missing unified conversion token: {required}");
        }
        assert!(!source.contains("Choose output TIFF"));
        assert!(!source.contains("next_versioned_output_path"));''',
    '''            assert!(runtime.contains(required), "missing unified conversion token: {required}");
        }
        assert!(!runtime.contains("Choose output TIFF"));
        assert!(!runtime.contains("next_versioned_output_path"));''',
)
replace_once(
    "src/ui/color_conversion.rs",
    '''        let source = include_str!("color_conversion.rs");
        assert!(source.contains("target: ConversionTargetState"));
        assert!(!source.contains("CandidateConfig"));
        assert!(!source.contains("ConversionBatchUiConfig"));''',
    '''        let source = include_str!("color_conversion.rs");
        let runtime = source.split("\\n#[cfg(test)]").next().unwrap_or(source);
        assert!(runtime.contains("target: ConversionTargetState"));
        assert!(!runtime.contains("CandidateConfig"));
        assert!(!runtime.contains("ConversionBatchUiConfig"));''',
)

replace_once(
    "src/ui/conversion_batch.rs",
    '''#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_runtime_contains_no_operator_target_config_or_window() {
        let source = include_str!("conversion_batch.rs");
        assert!(!source.contains("struct ConversionBatchUiConfig"));
        assert!(!source.contains("egui::Window::new"));
        assert!(!source.contains("Batch Convert"));
        assert!(source.contains("ConversionBatchCapture::capture"));
        assert!(source.contains("ConversionBatchQueue::load_persistent"));
        assert!(source.contains("queue_unified_conversion_plan"));
    }
}''',
    '''#[cfg(test)]
mod tests {
    #[test]
    fn batch_runtime_contains_no_operator_target_config_or_window() {
        let source = include_str!("conversion_batch.rs");
        let runtime = source.split("\\n#[cfg(test)]").next().unwrap_or(source);
        assert!(!runtime.contains("struct ConversionBatchUiConfig"));
        assert!(!runtime.contains("egui::Window::new"));
        assert!(!runtime.contains("Batch Convert"));
        assert!(runtime.contains("ConversionBatchCapture::capture"));
        assert!(runtime.contains("ConversionBatchQueue::load_persistent"));
        assert!(runtime.contains("queue_unified_conversion_plan"));
    }
}''',
)

replace_once(
    "src/ui/conversion_candidate_preview.rs",
    '''    fn candidate_controller_contains_no_independent_target_config_or_window() {
        let source = include_str!("conversion_candidate_preview.rs");
        assert!(!source.contains("struct CandidateConfig"));
        assert!(!source.contains("egui::Window::new"));
        assert!(!source.contains("Queue this exact conversion"));
        assert!(source.contains("sync_conversion_candidate"));
        assert!(source.contains("render_candidate_preview"));
    }''',
    '''    fn candidate_controller_contains_no_independent_target_config_or_window() {
        let source = include_str!("conversion_candidate_preview.rs");
        let runtime = source.split("\\n#[cfg(test)]").next().unwrap_or(source);
        assert!(!runtime.contains("struct CandidateConfig"));
        assert!(!runtime.contains("egui::Window::new"));
        assert!(!runtime.contains("Queue this exact conversion"));
        assert!(runtime.contains("sync_conversion_candidate"));
        assert!(runtime.contains("render_candidate_preview"));
    }''',
)

replace_once(
    "src/ui/mod.rs",
    '''        let conversion = include_str!("color_conversion.rs");
        let plan = include_str!("conversion_plan.rs");
        let batch = include_str!("conversion_batch.rs");
        let candidate = include_str!("conversion_candidate_preview.rs");''',
    '''        let conversion_source = include_str!("color_conversion.rs");
        let plan_source = include_str!("conversion_plan.rs");
        let batch_source = include_str!("conversion_batch.rs");
        let candidate_source = include_str!("conversion_candidate_preview.rs");
        let conversion = conversion_source.split("\\n#[cfg(test)]").next().unwrap_or(conversion_source);
        let plan = plan_source.split("\\n#[cfg(test)]").next().unwrap_or(plan_source);
        let batch = batch_source.split("\\n#[cfg(test)]").next().unwrap_or(batch_source);
        let candidate = candidate_source.split("\\n#[cfg(test)]").next().unwrap_or(candidate_source);''',
)

print("issue 372 contract tests now inspect production source only")
