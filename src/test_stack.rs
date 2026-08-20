use std::collections::BTreeSet;

use crate::model::ShadeProject;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TestStackAnchor {
    #[default]
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

impl TestStackAnchor {
    pub fn label(self) -> &'static str {
        match self {
            Self::TopLeft => "Top Left",
            Self::TopRight => "Top Right",
            Self::BottomLeft => "Bottom Left",
            Self::BottomRight => "Bottom Right",
        }
    }

    fn uses_right(self) -> bool {
        matches!(self, Self::TopRight | Self::BottomRight)
    }

    fn uses_bottom(self) -> bool {
        matches!(self, Self::BottomLeft | Self::BottomRight)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TestStackLayout {
    pub rows: usize,
    pub columns: usize,
}

impl TestStackLayout {
    pub const ONE_BY_TWO: Self = Self {
        rows: 1,
        columns: 2,
    };
    pub const ONE_BY_THREE: Self = Self {
        rows: 1,
        columns: 3,
    };
    pub const TWO_BY_TWO: Self = Self {
        rows: 2,
        columns: 2,
    };
    pub const THREE_ROWS: Self = Self {
        rows: 3,
        columns: 1,
    };

    pub fn new(rows: usize, columns: usize) -> Result<Self, String> {
        let layout = Self { rows, columns };
        layout.validate()?;
        Ok(layout)
    }

    pub fn capacity(self) -> usize {
        self.rows.saturating_mul(self.columns)
    }

    pub fn validate(self) -> Result<(), String> {
        if self.rows == 0 || self.columns == 0 {
            return Err("Test Stack rows and columns must both be at least 1.".to_owned());
        }
        self.rows
            .checked_mul(self.columns)
            .ok_or_else(|| "Test Stack grid is too large.".to_owned())?;
        Ok(())
    }

    pub fn validate_snapshot_count(self, count: usize) -> Result<(), String> {
        self.validate()?;
        let expected = self.capacity();
        if count != expected {
            return Err(format!(
                "Test Stack {}×{} requires exactly {expected} Snapshot(s); {count} selected.",
                self.rows, self.columns
            ));
        }
        Ok(())
    }

    pub fn cell_rect(
        self,
        width: usize,
        height: usize,
        index: usize,
    ) -> Result<TestStackRect, String> {
        self.validate()?;
        if index >= self.capacity() {
            return Err(format!(
                "Test Stack cell index {index} is outside a {}×{} grid.",
                self.rows, self.columns
            ));
        }
        if width == 0 || height == 0 {
            return Err("Test Stack source dimensions must be non-zero.".to_owned());
        }
        if self.columns > width || self.rows > height {
            return Err(format!(
                "Test Stack {}×{} grid is larger than the {width}×{height} source raster.",
                self.rows, self.columns
            ));
        }

        let row = index / self.columns;
        let column = index % self.columns;
        let (x0, x1) = partition_bounds(width, self.columns, column);
        let (y0, y1) = partition_bounds(height, self.rows, row);
        Ok(TestStackRect {
            x: x0,
            y: y0,
            width: x1 - x0,
            height: y1 - y0,
        })
    }

    pub fn crop_rect(
        self,
        width: usize,
        height: usize,
        index: usize,
        anchor: TestStackAnchor,
    ) -> Result<TestStackRect, String> {
        let cell = self.cell_rect(width, height, index)?;
        let x = if anchor.uses_right() {
            width - cell.width
        } else {
            0
        };
        let y = if anchor.uses_bottom() {
            height - cell.height
        } else {
            0
        };
        Ok(TestStackRect {
            x,
            y,
            width: cell.width,
            height: cell.height,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TestStackRect {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

pub fn materialize_snapshot_projects(
    base_project: &ShadeProject,
    snapshot_ids: &[u64],
) -> Result<Vec<ShadeProject>, String> {
    if snapshot_ids.is_empty() {
        return Err("Select at least one Snapshot for Test Stack.".to_owned());
    }

    let mut seen = BTreeSet::new();
    let mut projects = Vec::with_capacity(snapshot_ids.len());
    for &snapshot_id in snapshot_ids {
        if !seen.insert(snapshot_id) {
            return Err(format!(
                "Snapshot {snapshot_id} appears more than once in Test Stack."
            ));
        }
        let snapshot = base_project
            .snapshots
            .iter()
            .find(|snapshot| snapshot.id == snapshot_id)
            .ok_or_else(|| format!("Snapshot {snapshot_id} no longer exists."))?;
        let mut project = base_project.clone();
        project.adjustments = snapshot.adjustments.clone();
        project.active_snapshot_id = Some(snapshot.id);
        projects.push(project);
    }
    Ok(projects)
}

pub fn compose_u16(
    layout: TestStackLayout,
    anchor: TestStackAnchor,
    width: usize,
    height: usize,
    channels: usize,
    rendered_snapshots: &[Vec<u16>],
) -> Result<Vec<u16>, String> {
    layout.validate_snapshot_count(rendered_snapshots.len())?;
    if channels == 0 {
        return Err("Test Stack source must contain at least one channel.".to_owned());
    }
    let expected_samples = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(channels))
        .ok_or_else(|| "Test Stack sample count is too large.".to_owned())?;
    if expected_samples == 0 {
        return Err("Test Stack source dimensions must be non-zero.".to_owned());
    }

    for (index, samples) in rendered_snapshots.iter().enumerate() {
        if samples.len() != expected_samples {
            return Err(format!(
                "Rendered Snapshot {} has {} samples; expected {expected_samples} for {width}×{height}×{channels}.",
                index + 1,
                samples.len()
            ));
        }
    }

    let mut output = vec![0u16; expected_samples];
    for (index, samples) in rendered_snapshots.iter().enumerate() {
        let cell = layout.cell_rect(width, height, index)?;
        let crop = layout.crop_rect(width, height, index, anchor)?;
        debug_assert_eq!(cell.width, crop.width);
        debug_assert_eq!(cell.height, crop.height);
        let row_samples = cell
            .width
            .checked_mul(channels)
            .ok_or_else(|| "Test Stack row sample count is too large.".to_owned())?;

        for local_y in 0..cell.height {
            let source_y = crop.y + local_y;
            let destination_y = cell.y + local_y;
            let source_start = source_y
                .checked_mul(width)
                .and_then(|offset| offset.checked_add(crop.x))
                .and_then(|pixel| pixel.checked_mul(channels))
                .ok_or_else(|| "Test Stack source offset overflow.".to_owned())?;
            let destination_start = destination_y
                .checked_mul(width)
                .and_then(|offset| offset.checked_add(cell.x))
                .and_then(|pixel| pixel.checked_mul(channels))
                .ok_or_else(|| "Test Stack destination offset overflow.".to_owned())?;
            let source_end = source_start + row_samples;
            let destination_end = destination_start + row_samples;
            output[destination_start..destination_end]
                .copy_from_slice(&samples[source_start..source_end]);
        }
    }
    Ok(output)
}

fn partition_bounds(total: usize, parts: usize, index: usize) -> (usize, usize) {
    let start = index * total / parts;
    let end = (index + 1) * total / parts;
    (start, end)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AdjustmentSnapshot;

    fn raster(width: usize, height: usize, base: u16) -> Vec<u16> {
        (0..width * height)
            .map(|pixel| base + pixel as u16)
            .collect()
    }

    #[test]
    fn three_rows_keep_top_third_from_each_snapshot_without_scaling() {
        let layout = TestStackLayout::THREE_ROWS;
        let width = 2;
        let height = 6;
        let rendered = vec![
            raster(width, height, 100),
            raster(width, height, 200),
            raster(width, height, 300),
        ];
        let output = compose_u16(
            layout,
            TestStackAnchor::TopLeft,
            width,
            height,
            1,
            &rendered,
        )
        .unwrap();

        assert_eq!(output, vec![100, 101, 102, 103, 200, 201, 202, 203, 300, 301, 302, 303]);
    }

    #[test]
    fn two_by_two_top_left_uses_top_left_quadrant_for_every_snapshot() {
        let layout = TestStackLayout::TWO_BY_TWO;
        let width = 4;
        let height = 4;
        let rendered = vec![
            raster(width, height, 100),
            raster(width, height, 200),
            raster(width, height, 300),
            raster(width, height, 400),
        ];
        let output = compose_u16(
            layout,
            TestStackAnchor::TopLeft,
            width,
            height,
            1,
            &rendered,
        )
        .unwrap();

        assert_eq!(
            output,
            vec![
                100, 101, 200, 201,
                104, 105, 204, 205,
                300, 301, 400, 401,
                304, 305, 404, 405,
            ]
        );
    }

    #[test]
    fn bottom_right_anchor_keeps_code_corner_for_each_cell() {
        let layout = TestStackLayout::TWO_BY_TWO;
        let width = 4;
        let height = 4;
        let rendered = vec![
            raster(width, height, 100),
            raster(width, height, 200),
            raster(width, height, 300),
            raster(width, height, 400),
        ];
        let output = compose_u16(
            layout,
            TestStackAnchor::BottomRight,
            width,
            height,
            1,
            &rendered,
        )
        .unwrap();

        assert_eq!(
            output,
            vec![
                110, 111, 210, 211,
                114, 115, 214, 215,
                310, 311, 410, 411,
                314, 315, 414, 415,
            ]
        );
    }

    #[test]
    fn odd_dimensions_partition_output_without_gaps() {
        let layout = TestStackLayout::TWO_BY_TWO;
        let width = 5;
        let height = 5;
        let first = layout.cell_rect(width, height, 0).unwrap();
        let second = layout.cell_rect(width, height, 1).unwrap();
        let third = layout.cell_rect(width, height, 2).unwrap();
        let fourth = layout.cell_rect(width, height, 3).unwrap();

        assert_eq!(first, TestStackRect { x: 0, y: 0, width: 2, height: 2 });
        assert_eq!(second, TestStackRect { x: 2, y: 0, width: 3, height: 2 });
        assert_eq!(third, TestStackRect { x: 0, y: 2, width: 2, height: 3 });
        assert_eq!(fourth, TestStackRect { x: 2, y: 2, width: 3, height: 3 });
    }

    #[test]
    fn materialized_snapshot_projects_use_saved_snapshot_state_and_name() {
        let mut project = ShadeProject::default();
        project.test_code.enabled = true;
        project.test_code.text.clear();
        project.snapshots = vec![
            AdjustmentSnapshot {
                id: 10,
                name: "Test A".to_owned(),
                created_at_unix_ms: 1,
                adjustments: Default::default(),
                exports: Vec::new(),
                history: Default::default(),
            },
            AdjustmentSnapshot {
                id: 20,
                name: "Test B".to_owned(),
                created_at_unix_ms: 2,
                adjustments: Default::default(),
                exports: Vec::new(),
                history: Default::default(),
            },
        ];

        let materialized = materialize_snapshot_projects(&project, &[20, 10]).unwrap();
        assert_eq!(materialized[0].active_snapshot_id, Some(20));
        assert_eq!(materialized[0].effective_test_code_text(), "Test B");
        assert_eq!(materialized[1].active_snapshot_id, Some(10));
        assert_eq!(materialized[1].effective_test_code_text(), "Test A");
    }

    #[test]
    fn duplicate_snapshot_selection_is_rejected() {
        let mut project = ShadeProject::default();
        project.snapshots.push(AdjustmentSnapshot {
            id: 7,
            name: "Test 7".to_owned(),
            created_at_unix_ms: 1,
            adjustments: Default::default(),
            exports: Vec::new(),
            history: Default::default(),
        });
        let err = materialize_snapshot_projects(&project, &[7, 7]).unwrap_err();
        assert!(err.contains("more than once"));
    }
}