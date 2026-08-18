use std::collections::BTreeSet;

use crate::device_characterization::LabColor;
use crate::inverse_lut_identity::LabGridSpec;
use crate::inverse_lut_validation::InverseLutHoldoutMethod;

/// Bounded 3D cell-center population sampled by CellCentersAndFixedPathsV1.
/// Changing this bound changes the numerical validation method and therefore
/// requires a new holdout-method enum variant.
pub const MAX_INVERSE_LUT_CELL_CENTER_HOLDOUTS_V1: usize = 16_384;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InverseLutHoldoutPathKind {
    NeutralAxis,
    NearNeutralWarm,
    NearNeutralCool,
    AAxis,
    BAxis,
    AbDiagonal,
    AbOpposedDiagonal,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InverseLutHoldoutPath {
    pub kind: InverseLutHoldoutPathKind,
    pub samples: Vec<LabColor>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct InverseLutHoldoutSet {
    pub method: InverseLutHoldoutMethod,
    /// Deterministically stratified 3D cell-center samples excluding points
    /// already represented by one of the ordered diagnostic paths.
    pub point_samples: Vec<LabColor>,
    /// Ordered paths are kept separately so #177 continuity and curvature
    /// diagnostics can consume them without reconstructing traversal order.
    pub paths: Vec<InverseLutHoldoutPath>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InverseLutHoldoutError {
    InvalidGrid(Vec<String>),
    CellCountOverflow,
}

pub fn generate_inverse_lut_holdouts(
    grid: LabGridSpec,
    method: InverseLutHoldoutMethod,
) -> Result<InverseLutHoldoutSet, InverseLutHoldoutError> {
    grid.validate()
        .map_err(InverseLutHoldoutError::InvalidGrid)?;

    match method {
        InverseLutHoldoutMethod::CellCentersAndFixedPathsV1 => {
            generate_cell_centers_and_fixed_paths_v1(grid)
        }
    }
}

fn generate_cell_centers_and_fixed_paths_v1(
    grid: LabGridSpec,
) -> Result<InverseLutHoldoutSet, InverseLutHoldoutError> {
    let l_cells = usize::from(grid.l_samples - 1);
    let a_cells = usize::from(grid.a_samples - 1);
    let b_cells = usize::from(grid.b_samples - 1);
    let cell_count = l_cells
        .checked_mul(a_cells)
        .and_then(|value| value.checked_mul(b_cells))
        .ok_or(InverseLutHoldoutError::CellCountOverflow)?;

    let neutral_a = nearest_cell_center_index(0.0, grid.a_min, grid.a_max, a_cells);
    let neutral_b = nearest_cell_center_index(0.0, grid.b_min, grid.b_max, b_cells);
    let warm_a = nearest_cell_center_index(
        0.08 * (grid.a_max - grid.a_min),
        grid.a_min,
        grid.a_max,
        a_cells,
    );
    let warm_b = nearest_cell_center_index(
        0.08 * (grid.b_max - grid.b_min),
        grid.b_min,
        grid.b_max,
        b_cells,
    );
    let cool_a = nearest_cell_center_index(
        -0.08 * (grid.a_max - grid.a_min),
        grid.a_min,
        grid.a_max,
        a_cells,
    );
    let cool_b = nearest_cell_center_index(
        -0.08 * (grid.b_max - grid.b_min),
        grid.b_min,
        grid.b_max,
        b_cells,
    );
    let middle_l = l_cells / 2;

    let paths = vec![
        InverseLutHoldoutPath {
            kind: InverseLutHoldoutPathKind::NeutralAxis,
            samples: (0..l_cells)
                .map(|li| cell_center(&grid, li, neutral_a, neutral_b))
                .collect(),
        },
        InverseLutHoldoutPath {
            kind: InverseLutHoldoutPathKind::NearNeutralWarm,
            samples: (0..l_cells)
                .map(|li| cell_center(&grid, li, warm_a, warm_b))
                .collect(),
        },
        InverseLutHoldoutPath {
            kind: InverseLutHoldoutPathKind::NearNeutralCool,
            samples: (0..l_cells)
                .map(|li| cell_center(&grid, li, cool_a, cool_b))
                .collect(),
        },
        InverseLutHoldoutPath {
            kind: InverseLutHoldoutPathKind::AAxis,
            samples: (0..a_cells)
                .map(|ai| cell_center(&grid, middle_l, ai, neutral_b))
                .collect(),
        },
        InverseLutHoldoutPath {
            kind: InverseLutHoldoutPathKind::BAxis,
            samples: (0..b_cells)
                .map(|bi| cell_center(&grid, middle_l, neutral_a, bi))
                .collect(),
        },
        InverseLutHoldoutPath {
            kind: InverseLutHoldoutPathKind::AbDiagonal,
            samples: diagonal_path(&grid, middle_l, a_cells, b_cells, false),
        },
        InverseLutHoldoutPath {
            kind: InverseLutHoldoutPathKind::AbOpposedDiagonal,
            samples: diagonal_path(&grid, middle_l, a_cells, b_cells, true),
        },
    ];

    let mut seen = BTreeSet::new();
    for path in &paths {
        for sample in &path.samples {
            seen.insert(lab_bits(*sample));
        }
    }

    let requested = cell_count.min(MAX_INVERSE_LUT_CELL_CENTER_HOLDOUTS_V1);
    let mut point_samples = Vec::with_capacity(requested);
    if requested > 0 {
        for sample_index in 0..requested {
            // Evenly spread deterministic integer indices over the complete
            // lexicographic cell population. No RNG/seed or platform float
            // ordering participates in selection.
            let linear = ((sample_index as u128) * (cell_count as u128) / (requested as u128))
                as usize;
            let bi = linear % b_cells;
            let remaining = linear / b_cells;
            let ai = remaining % a_cells;
            let li = remaining / a_cells;
            let sample = cell_center(&grid, li, ai, bi);
            if seen.insert(lab_bits(sample)) {
                point_samples.push(sample);
            }
        }
    }

    Ok(InverseLutHoldoutSet {
        method: InverseLutHoldoutMethod::CellCentersAndFixedPathsV1,
        point_samples,
        paths,
    })
}

fn diagonal_path(
    grid: &LabGridSpec,
    li: usize,
    a_cells: usize,
    b_cells: usize,
    opposed: bool,
) -> Vec<LabColor> {
    let count = a_cells.max(b_cells);
    if count == 0 {
        return Vec::new();
    }
    (0..count)
        .map(|index| {
            let denominator = count.saturating_sub(1).max(1);
            let ai = scaled_index(index, denominator, a_cells);
            let direct_bi = scaled_index(index, denominator, b_cells);
            let bi = if opposed {
                b_cells.saturating_sub(1).saturating_sub(direct_bi)
            } else {
                direct_bi
            };
            cell_center(grid, li, ai, bi)
        })
        .collect()
}

fn scaled_index(index: usize, denominator: usize, cells: usize) -> usize {
    if cells <= 1 {
        return 0;
    }
    index
        .saturating_mul(cells - 1)
        .checked_div(denominator)
        .unwrap_or(0)
        .min(cells - 1)
}

fn cell_center(grid: &LabGridSpec, li: usize, ai: usize, bi: usize) -> LabColor {
    LabColor {
        l: axis_cell_center(grid.l_min, grid.l_max, usize::from(grid.l_samples - 1), li),
        a: axis_cell_center(grid.a_min, grid.a_max, usize::from(grid.a_samples - 1), ai),
        b: axis_cell_center(grid.b_min, grid.b_max, usize::from(grid.b_samples - 1), bi),
    }
}

fn axis_cell_center(minimum: f64, maximum: f64, cells: usize, index: usize) -> f64 {
    let step = (maximum - minimum) / cells as f64;
    minimum + (index as f64 + 0.5) * step
}

fn nearest_cell_center_index(target: f64, minimum: f64, maximum: f64, cells: usize) -> usize {
    if cells <= 1 {
        return 0;
    }
    let step = (maximum - minimum) / cells as f64;
    let raw = ((target.clamp(minimum, maximum) - minimum) / step) - 0.5;
    raw.round().clamp(0.0, (cells - 1) as f64) as usize
}

fn lab_bits(value: LabColor) -> (u64, u64, u64) {
    (value.l.to_bits(), value.a.to_bits(), value.b.to_bits())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(samples: u16) -> LabGridSpec {
        LabGridSpec {
            l_min: 0.0,
            l_max: 100.0,
            l_samples: samples,
            a_min: -80.0,
            a_max: 80.0,
            a_samples: samples,
            b_min: -100.0,
            b_max: 100.0,
            b_samples: samples,
        }
    }

    #[test]
    fn v1_holdouts_are_deterministic_and_exclude_grid_nodes() {
        let grid = grid(5);
        let first = generate_inverse_lut_holdouts(
            grid,
            InverseLutHoldoutMethod::CellCentersAndFixedPathsV1,
        )
        .unwrap();
        let second = generate_inverse_lut_holdouts(
            grid,
            InverseLutHoldoutMethod::CellCentersAndFixedPathsV1,
        )
        .unwrap();
        assert_eq!(first, second);
        assert_eq!(first.paths.len(), 7);

        for sample in first
            .point_samples
            .iter()
            .chain(first.paths.iter().flat_map(|path| path.samples.iter()))
        {
            assert!(sample.l > grid.l_min && sample.l < grid.l_max);
            assert!(sample.a > grid.a_min && sample.a < grid.a_max);
            assert!(sample.b > grid.b_min && sample.b < grid.b_max);
            assert!(!is_axis_grid_node(sample.l, grid.l_min, grid.l_max, grid.l_samples));
            assert!(!is_axis_grid_node(sample.a, grid.a_min, grid.a_max, grid.a_samples));
            assert!(!is_axis_grid_node(sample.b, grid.b_min, grid.b_max, grid.b_samples));
        }
    }

    #[test]
    fn full_small_grid_uses_every_non_path_cell_center_once() {
        let grid = grid(4);
        let holdouts = generate_inverse_lut_holdouts(
            grid,
            InverseLutHoldoutMethod::CellCentersAndFixedPathsV1,
        )
        .unwrap();
        let path_points = holdouts
            .paths
            .iter()
            .flat_map(|path| path.samples.iter().copied())
            .map(lab_bits)
            .collect::<BTreeSet<_>>();
        let point_bits = holdouts
            .point_samples
            .iter()
            .copied()
            .map(lab_bits)
            .collect::<BTreeSet<_>>();
        assert!(path_points.is_disjoint(&point_bits));
        assert_eq!(path_points.union(&point_bits).count(), 27);
    }

    #[test]
    fn large_grid_cell_population_is_bounded() {
        // 100^3 nodes is exactly the maximum valid LUT grid. Its 99^3
        // cell-center population is still far larger than the V1 holdout cap.
        let holdouts = generate_inverse_lut_holdouts(
            grid(100),
            InverseLutHoldoutMethod::CellCentersAndFixedPathsV1,
        )
        .unwrap();
        assert!(holdouts.point_samples.len() <= MAX_INVERSE_LUT_CELL_CENTER_HOLDOUTS_V1);
        assert!(holdouts.paths.iter().all(|path| path.samples.len() <= 99));
    }

    #[test]
    fn path_identity_and_order_are_stable() {
        let holdouts = generate_inverse_lut_holdouts(
            grid(6),
            InverseLutHoldoutMethod::CellCentersAndFixedPathsV1,
        )
        .unwrap();
        assert_eq!(
            holdouts.paths.iter().map(|path| path.kind).collect::<Vec<_>>(),
            vec![
                InverseLutHoldoutPathKind::NeutralAxis,
                InverseLutHoldoutPathKind::NearNeutralWarm,
                InverseLutHoldoutPathKind::NearNeutralCool,
                InverseLutHoldoutPathKind::AAxis,
                InverseLutHoldoutPathKind::BAxis,
                InverseLutHoldoutPathKind::AbDiagonal,
                InverseLutHoldoutPathKind::AbOpposedDiagonal,
            ]
        );
    }

    fn is_axis_grid_node(value: f64, minimum: f64, maximum: f64, samples: u16) -> bool {
        let step = (maximum - minimum) / f64::from(samples - 1);
        let position = (value - minimum) / step;
        (position - position.round()).abs() < 1.0e-10
    }
}
