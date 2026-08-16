#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LabColor {
    pub l: f64,
    pub a: f64,
    pub b: f64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CharacterizationIdentity {
    /// Stable target-owned identity/version. This is not inferred from display colors.
    pub id: String,
    /// Exact ordered production channel names the model was measured for.
    pub channel_names: Vec<String>,
}

/// Forward characterization contract for custom N-ink optimization.
///
/// Implementations predict the printed/fired PCS color produced by one exact
/// target-ink vector. The optimizer may invert this model by searching candidate
/// ink vectors, but it must never invent device behavior from UI channel colors.
pub trait DeviceForwardModel {
    fn identity(&self) -> &CharacterizationIdentity;

    /// Predict CIE Lab for normalized channel coverage values in authoritative
    /// target order. Implementations should reject vectors outside the measured
    /// model domain instead of silently extrapolating production behavior.
    fn predict_lab(&self, coverages: &[f32]) -> Result<LabColor, String>;
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CharacterizedColorEvaluation {
    pub predicted: LabColor,
    pub delta_e00: f64,
}

pub fn evaluate_characterized_color(
    model: &dyn DeviceForwardModel,
    target_lab: LabColor,
    coverages: &[f32],
) -> Result<CharacterizedColorEvaluation, String> {
    if coverages.len() != model.identity().channel_names.len() {
        return Err(format!(
            "Characterization topology mismatch: model has {} channels, candidate has {}.",
            model.identity().channel_names.len(),
            coverages.len()
        ));
    }
    if coverages
        .iter()
        .any(|value| !value.is_finite() || !(0.0..=1.0).contains(value))
    {
        return Err("Characterization candidate contains coverage outside 0..=1.".to_owned());
    }

    let predicted = model.predict_lab(coverages)?;
    if !lab_is_finite(predicted) || !lab_is_finite(target_lab) {
        return Err("Characterization produced or received non-finite Lab values.".to_owned());
    }

    Ok(CharacterizedColorEvaluation {
        predicted,
        delta_e00: delta_e_2000(target_lab, predicted),
    })
}

fn lab_is_finite(value: LabColor) -> bool {
    value.l.is_finite() && value.a.is_finite() && value.b.is_finite()
}

/// CIEDE2000 color difference using the standard kL=kC=kH=1 graphic-arts weights.
pub fn delta_e_2000(first: LabColor, second: LabColor) -> f64 {
    use std::f64::consts::PI;

    let c1 = (first.a * first.a + first.b * first.b).sqrt();
    let c2 = (second.a * second.a + second.b * second.b).sqrt();
    let c_bar = (c1 + c2) * 0.5;
    let c_bar7 = c_bar.powi(7);
    let twenty_five7 = 25.0_f64.powi(7);
    let g = 0.5 * (1.0 - (c_bar7 / (c_bar7 + twenty_five7)).sqrt());

    let a1_prime = (1.0 + g) * first.a;
    let a2_prime = (1.0 + g) * second.a;
    let c1_prime = (a1_prime * a1_prime + first.b * first.b).sqrt();
    let c2_prime = (a2_prime * a2_prime + second.b * second.b).sqrt();

    let h1_prime = hue_degrees(first.b, a1_prime);
    let h2_prime = hue_degrees(second.b, a2_prime);

    let delta_l_prime = second.l - first.l;
    let delta_c_prime = c2_prime - c1_prime;

    let delta_h_degrees = if c1_prime * c2_prime == 0.0 {
        0.0
    } else {
        let raw = h2_prime - h1_prime;
        if raw.abs() <= 180.0 {
            raw
        } else if raw > 180.0 {
            raw - 360.0
        } else {
            raw + 360.0
        }
    };
    let delta_h_prime = 2.0
        * (c1_prime * c2_prime).sqrt()
        * ((delta_h_degrees * PI / 360.0).sin());

    let l_bar_prime = (first.l + second.l) * 0.5;
    let c_bar_prime = (c1_prime + c2_prime) * 0.5;

    let h_bar_prime = if c1_prime * c2_prime == 0.0 {
        h1_prime + h2_prime
    } else if (h1_prime - h2_prime).abs() <= 180.0 {
        (h1_prime + h2_prime) * 0.5
    } else if h1_prime + h2_prime < 360.0 {
        (h1_prime + h2_prime + 360.0) * 0.5
    } else {
        (h1_prime + h2_prime - 360.0) * 0.5
    };

    let t = 1.0
        - 0.17 * degrees_cos(h_bar_prime - 30.0)
        + 0.24 * degrees_cos(2.0 * h_bar_prime)
        + 0.32 * degrees_cos(3.0 * h_bar_prime + 6.0)
        - 0.20 * degrees_cos(4.0 * h_bar_prime - 63.0);

    let delta_theta = 30.0 * (-(((h_bar_prime - 275.0) / 25.0).powi(2))).exp();
    let c_bar_prime7 = c_bar_prime.powi(7);
    let r_c = 2.0 * (c_bar_prime7 / (c_bar_prime7 + twenty_five7)).sqrt();
    let l_term = (l_bar_prime - 50.0).powi(2);
    let s_l = 1.0 + (0.015 * l_term) / (20.0 + l_term).sqrt();
    let s_c = 1.0 + 0.045 * c_bar_prime;
    let s_h = 1.0 + 0.015 * c_bar_prime * t;
    let r_t = -degrees_sin(2.0 * delta_theta) * r_c;

    let l = delta_l_prime / s_l;
    let c = delta_c_prime / s_c;
    let h = delta_h_prime / s_h;
    (l * l + c * c + h * h + r_t * c * h).max(0.0).sqrt()
}

fn hue_degrees(b: f64, a_prime: f64) -> f64 {
    if a_prime == 0.0 && b == 0.0 {
        return 0.0;
    }
    let mut degrees = b.atan2(a_prime).to_degrees();
    if degrees < 0.0 {
        degrees += 360.0;
    }
    degrees
}

fn degrees_cos(value: f64) -> f64 {
    value.to_radians().cos()
}

fn degrees_sin(value: f64) -> f64 {
    value.to_radians().sin()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct SyntheticFourInkModel {
        identity: CharacterizationIdentity,
    }

    impl SyntheticFourInkModel {
        fn new() -> Self {
            Self {
                identity: CharacterizationIdentity {
                    id: "synthetic-test-only".to_owned(),
                    channel_names: vec![
                        "Blue".to_owned(),
                        "Brown".to_owned(),
                        "Beige".to_owned(),
                        "Black".to_owned(),
                    ],
                },
            }
        }
    }

    impl DeviceForwardModel for SyntheticFourInkModel {
        fn identity(&self) -> &CharacterizationIdentity {
            &self.identity
        }

        fn predict_lab(&self, coverages: &[f32]) -> Result<LabColor, String> {
            if coverages.len() != 4 {
                return Err("synthetic topology mismatch".to_owned());
            }
            // Test-only deterministic model. Production implementations must be
            // measured characterization/CLUTs, never coefficients like these.
            let blue = f64::from(coverages[0]);
            let brown = f64::from(coverages[1]);
            let beige = f64::from(coverages[2]);
            let black = f64::from(coverages[3]);
            Ok(LabColor {
                l: 95.0 - 25.0 * blue - 20.0 * brown - 12.0 * beige - 55.0 * black,
                a: 7.0 * brown + 3.0 * beige - 1.5 * blue,
                b: -18.0 * blue + 9.0 * brown + 7.0 * beige,
            })
        }
    }

    #[test]
    fn ciede2000_matches_published_reference_pairs() {
        let pairs = [
            (
                LabColor { l: 50.0, a: 2.6772, b: -79.7751 },
                LabColor { l: 50.0, a: 0.0, b: -82.7485 },
                2.0425,
            ),
            (
                LabColor { l: 50.0, a: 3.1571, b: -77.2803 },
                LabColor { l: 50.0, a: 0.0, b: -82.7485 },
                2.8615,
            ),
            (
                LabColor { l: 50.0, a: 2.8361, b: -74.0200 },
                LabColor { l: 50.0, a: 0.0, b: -82.7485 },
                3.4412,
            ),
        ];

        for (first, second, expected) in pairs {
            let actual = delta_e_2000(first, second);
            assert!(
                (actual - expected).abs() < 0.0002,
                "expected {expected}, got {actual}"
            );
        }
    }

    #[test]
    fn characterized_evaluation_enforces_topology_and_domain() {
        let model = SyntheticFourInkModel::new();
        let target = LabColor { l: 60.0, a: 2.0, b: -2.0 };
        assert!(evaluate_characterized_color(&model, target, &[0.1, 0.1, 0.1]).is_err());
        assert!(evaluate_characterized_color(&model, target, &[0.1, 0.1, 0.1, 1.2]).is_err());
        assert!(evaluate_characterized_color(&model, target, &[0.1, 0.1, 0.1, 0.3]).is_ok());
    }

    #[test]
    fn characterization_identity_preserves_exact_channel_order() {
        let model = SyntheticFourInkModel::new();
        assert_eq!(
            model.identity().channel_names,
            ["Blue", "Brown", "Beige", "Black"]
        );
    }
}
