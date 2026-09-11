//! Static TrueSkill with an ordinal, five-interval comparison likelihood.
//!
//! The Gaussian prior, performance noise and moment-matched update follow
//! TrueSkill. The strong-preference boundary is this application's extension.

use std::collections::{HashMap, HashSet};
use std::f64::consts::{PI, SQRT_2};
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

pub const MODEL_VERSION: u32 = 1;
pub const MODEL_PARAMETERS_JSON: &str = r#"{"mu":25.0,"sigma":8.333333333333334,"beta":4.166666666666667,"tau":0.0,"epsilon":0.7404665874521482,"strongMargin":8.333333333333334,"maxSigma":2.5,"minWindow":20,"maxRankSpan":1}"#;

const INITIAL_MU: f64 = 25.0;
const INITIAL_SIGMA: f64 = 25.0 / 3.0;
const BETA: f64 = 25.0 / 6.0;
const EPSILON: f64 = 0.740_466_587_452_148_2;
const STRONG_MARGIN: f64 = 25.0 / 3.0;
const MAX_SIGMA: f64 = 2.5;
const MIN_WINDOW: usize = 20;
const MAX_RANK_SPAN: usize = 1;
const LOG_SQRT_TWO_PI: f64 = 0.918_938_533_204_672_7;
const BOUNDARIES: [f64; 6] = [
    f64::NEG_INFINITY,
    -STRONG_MARGIN,
    -EPSILON,
    EPSILON,
    STRONG_MARGIN,
    f64::INFINITY,
];

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Rating {
    pub mu: f64,
    pub sigma: f64,
}

impl Default for Rating {
    fn default() -> Self {
        Self {
            mu: INITIAL_MU,
            sigma: INITIAL_SIGMA,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Preference {
    AStrong,
    AWeak,
    Equal,
    BWeak,
    BStrong,
}

impl Preference {
    fn interval(self) -> (f64, f64) {
        let index = match self {
            Self::BStrong => 0,
            Self::BWeak => 1,
            Self::Equal => 2,
            Self::AWeak => 3,
            Self::AStrong => 4,
        };
        (BOUNDARIES[index], BOUNDARIES[index + 1])
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Convergence {
    pub converged: bool,
    pub max_sigma: f64,
    pub max_rank_span: Option<usize>,
    pub observed_answers: usize,
    pub required_answers: usize,
}

#[derive(Debug, Clone, Copy)]
struct Moments {
    log_probability: f64,
    mean: f64,
    variance: f64,
}

fn valid_rating(rating: Rating) -> bool {
    rating.mu.is_finite() && rating.sigma.is_finite() && rating.sigma > 0.0
}

fn comparison_scale(a: Rating, b: Rating) -> Result<(f64, f64, f64), String> {
    if !valid_rating(a) || !valid_rating(b) {
        return Err("評価値は有限の μ と正の有限の σ が必要です。".into());
    }
    let variance_a = a.sigma * a.sigma;
    let variance_b = b.sigma * b.sigma;
    let variance = variance_a + variance_b + 2.0 * BETA * BETA;
    if !variance.is_finite() || !(a.mu - b.mu).is_finite() {
        return Err("評価値が数値計算で扱える範囲を超えています。".into());
    }
    Ok((variance_a, variance_b, variance.sqrt()))
}

/// Apply exactly one observation. Strong preference denotes a larger gap,
/// rather than repeated independent wins or greater respondent certainty.
pub fn update_pair(a: Rating, b: Rating, answer: Preference) -> Result<(Rating, Rating), String> {
    let (variance_a, variance_b, scale) = comparison_scale(a, b)?;
    let difference = a.mu - b.mu;
    let (lower, upper) = answer.interval();
    let moments = truncated_moments((lower - difference) / scale, (upper - difference) / scale)?;
    let reduction = 1.0 - moments.variance;
    let next_a = Rating {
        mu: a.mu + (variance_a / scale) * moments.mean,
        sigma: a.sigma * (1.0 - (variance_a / scale / scale) * reduction).sqrt(),
    };
    let next_b = Rating {
        mu: b.mu - (variance_b / scale) * moments.mean,
        sigma: b.sigma * (1.0 - (variance_b / scale / scale) * reduction).sqrt(),
    };
    if !valid_rating(next_a) || !valid_rating(next_b) {
        return Err("評価値の更新を安全に計算できませんでした。".into());
    }
    Ok((next_a, next_b))
}

pub fn ranking_ids(items: &[(i64, Rating)]) -> Vec<i64> {
    let mut ordered = items.to_vec();
    ordered.sort_by(|(id_a, a), (id_b, b)| b.mu.total_cmp(&a.mu).then(id_a.cmp(id_b)));
    ordered.into_iter().map(|(id, _)| id).collect()
}

fn information_gain(a: Rating, b: Rating) -> Result<f64, String> {
    let (variance_a, variance_b, scale) = comparison_scale(a, b)?;
    // Sign symmetry makes equally informative mirrored pairs exact ties.
    let difference = (a.mu - b.mu).abs();
    let fraction_a = variance_a / scale / scale;
    let fraction_b = variance_b / scale / scale;
    let mut information = 0.0;
    for interval in BOUNDARIES.windows(2) {
        let moments = truncated_moments(
            (interval[0] - difference) / scale,
            (interval[1] - difference) / scale,
        )?;
        let probability = moments.log_probability.exp();
        let reduction = 1.0 - moments.variance;
        // With moment matching and a partition of possible observations,
        // expected Gaussian KL equals expected marginal entropy reduction.
        // ln_1p preserves information when the remaining uncertainty is tiny.
        let entropy_reduction =
            -0.5 * ((-fraction_a * reduction).ln_1p() + (-fraction_b * reduction).ln_1p());
        information += probability * entropy_reduction;
    }
    if information.is_finite() && information >= 0.0 {
        Ok(information)
    } else {
        Err("比較候補の情報量を安全に計算できませんでした。".into())
    }
}

/// Scan all unordered pairs without a fixed item cap or candidate sampling.
pub fn select_pair(
    items: &[(i64, Rating)],
    counts: &HashMap<(i64, i64), u64>,
) -> Result<Option<(i64, i64)>, String> {
    let mut ids = HashSet::with_capacity(items.len());
    for &(id, rating) in items {
        if !ids.insert(id) || !valid_rating(rating) {
            return Err("比較候補に重複 ID または不正な評価値があります。".into());
        }
    }
    let mut best: Option<(f64, u64, (i64, i64))> = None;
    for (index, &(id_a, a)) in items.iter().enumerate() {
        for &(id_b, b) in &items[index + 1..] {
            let pair = (id_a.min(id_b), id_a.max(id_b));
            let count = counts.get(&pair).copied().unwrap_or(0);
            let information = information_gain(a, b)?;
            let replace = best.is_none_or(|(best_information, best_count, best_pair)| {
                information > best_information
                    || (information == best_information && (count, pair) < (best_count, best_pair))
            });
            if replace {
                best = Some((information, count, pair));
            }
        }
    }
    Ok(best.map(|(_, _, pair)| pair))
}

/// Inspect the most recent W transitions (W + 1 actual ranking frames).
/// Callers reset frames when membership changes or comparison is resumed.
pub fn convergence(items: &[(i64, Rating)], snapshots: &[Vec<i64>]) -> Convergence {
    let required_answers = MIN_WINDOW.max(items.len());
    let observed_answers = snapshots.len().saturating_sub(1).min(required_answers);
    let max_sigma = items
        .iter()
        .map(|(_, rating)| rating.sigma)
        .fold(0.0, f64::max);
    let mut result = Convergence {
        converged: false,
        max_sigma,
        max_rank_span: None,
        observed_answers,
        required_answers,
    };
    if items.len() < 2 || items.iter().any(|(_, rating)| !valid_rating(*rating)) {
        return result;
    }
    let positions: HashMap<i64, usize> = items
        .iter()
        .enumerate()
        .map(|(index, (id, _))| (*id, index))
        .collect();
    if positions.len() != items.len() || snapshots.is_empty() {
        return result;
    }
    let frames = &snapshots[snapshots.len().saturating_sub(required_answers + 1)..];
    let mut minimum = vec![usize::MAX; items.len()];
    let mut maximum = vec![0; items.len()];
    for frame in frames {
        if frame.len() != items.len() {
            return result;
        }
        let mut seen = vec![false; items.len()];
        for (rank, id) in frame.iter().enumerate() {
            let Some(&index) = positions.get(id) else {
                return result;
            };
            if seen[index] {
                return result;
            }
            seen[index] = true;
            minimum[index] = minimum[index].min(rank);
            maximum[index] = maximum[index].max(rank);
        }
    }
    let max_rank_span = minimum
        .iter()
        .zip(maximum)
        .map(|(min, max)| max - min)
        .max()
        .unwrap_or(0);
    result.max_rank_span = Some(max_rank_span);
    result.converged = observed_answers == required_answers
        && max_sigma <= MAX_SIGMA
        && max_rank_span <= MAX_RANK_SPAN;
    result
}

/// Conditional standard-normal moments, including events too unlikely to
/// represent as an ordinary probability. No probability floor is applied.
fn truncated_moments(lower: f64, upper: f64) -> Result<Moments, String> {
    if lower.is_nan() || upper.is_nan() || lower >= upper {
        return Err("比較結果の区間を数値計算で表現できませんでした。".into());
    }
    if upper <= 0.0 {
        let mut moments = truncated_moments(-upper, -lower)?;
        moments.mean = -moments.mean;
        return Ok(moments);
    }
    let moments = if lower >= 8.0 {
        positive_tail_moments(lower, upper)
    } else if lower.is_finite() && upper.is_finite() && upper - lower <= 2.0 {
        finite_interval_moments(lower, upper)
    } else if upper == f64::INFINITY {
        let (mean, log_probability) = if lower >= 0.0 {
            let scaled_complement = puruspe::erfcx(lower / SQRT_2);
            (
                (2.0 / PI).sqrt() / scaled_complement,
                -0.5 * lower * lower + scaled_complement.ln() - 2.0_f64.ln(),
            )
        } else {
            let probability = 0.5 * puruspe::erfc(lower / SQRT_2);
            (normal_density(lower) / probability, probability.ln())
        };
        Moments {
            log_probability,
            mean,
            variance: 1.0 + finite_product(lower, mean) - mean * mean,
        }
    } else if lower >= 0.0 {
        // Divide interval mass and endpoint densities by phi(lower) before
        // taking moments; erfcx keeps these quantities representable in tails.
        let delta = 0.5 * (upper - lower) * (upper + lower);
        let exponential = (-delta).exp();
        let mass = (PI / 2.0).sqrt()
            * (puruspe::erfcx(lower / SQRT_2) - exponential * puruspe::erfcx(upper / SQRT_2));
        let mean = -(-delta).exp_m1() / mass;
        Moments {
            log_probability: -0.5 * lower * lower - LOG_SQRT_TWO_PI + mass.ln(),
            mean,
            variance: 1.0 + (lower - upper * exponential) / mass - mean * mean,
        }
    } else {
        let probability = 0.5 * (puruspe::erf(upper / SQRT_2) - puruspe::erf(lower / SQRT_2));
        let density_lower = normal_density(lower);
        let density_upper = normal_density(upper);
        let mean = (density_lower - density_upper) / probability;
        Moments {
            log_probability: probability.ln(),
            mean,
            variance: 1.0
                + (finite_product(lower, density_lower) - upper * density_upper) / probability
                - mean * mean,
        }
    };
    // Interval conditioning cannot increase standard-normal variance. Permit
    // only rounding at the endpoints; do not hide a failed calculation.
    const ROUNDING_TOLERANCE: f64 = 1e-12;
    if moments.log_probability.is_nan()
        || moments.log_probability > ROUNDING_TOLERANCE
        || !moments.mean.is_finite()
        || !moments.variance.is_finite()
        || moments.variance < -ROUNDING_TOLERANCE
        || moments.variance > 1.0 + ROUNDING_TOLERANCE
    {
        return Err("比較結果の分布を安全に計算できませんでした。".into());
    }
    Ok(Moments {
        log_probability: moments.log_probability.min(0.0),
        variance: moments.variance.clamp(0.0, 1.0),
        ..moments
    })
}

fn normal_density(value: f64) -> f64 {
    (-0.5 * value * value - LOG_SQRT_TWO_PI).exp()
}

fn finite_product(value: f64, density: f64) -> f64 {
    if value.is_infinite() {
        0.0
    } else {
        value * density
    }
}

fn finite_interval_moments(lower: f64, upper: f64) -> Moments {
    let half_width = (upper - lower) * 0.5;
    let midpoint = lower + half_width;
    let (mass, mean, variance) = integrate_moments(-1.0, 1.0, |point| {
        let offset = half_width * point;
        (-midpoint * offset - 0.5 * offset * offset).exp()
    });
    Moments {
        log_probability: -0.5 * midpoint * midpoint - LOG_SQRT_TWO_PI + half_width.ln() + mass.ln(),
        mean: midpoint + half_width * mean,
        variance: half_width * half_width * variance,
    }
}

fn positive_tail_moments(lower: f64, upper: f64) -> Moments {
    // For x = lower + t/lower the scaled density is
    // exp(-t - t²/(2 lower²)). Its scale stays near one even at x = 1e100.
    // Beyond t=40, omitted mass and its first two moments are below f64
    // precision at the resulting rating scale; this is integration tolerance,
    // not a substituted probability for an unlikely answer.
    let extent = ((upper - lower) * lower).min(40.0);
    let inverse_lower = 1.0 / lower;
    let (mass, mean, variance) = integrate_moments(0.0, extent, |point| {
        let offset = point * inverse_lower;
        (-point - 0.5 * offset * offset).exp()
    });
    Moments {
        log_probability: -0.5 * lower * lower - LOG_SQRT_TWO_PI - lower.ln() + mass.ln(),
        mean: lower + mean * inverse_lower,
        variance: variance * inverse_lower * inverse_lower,
    }
}

/// A cached 32-point Gauss–Legendre rule. Integrating centered moments avoids
/// subtracting numbers of size lower² to recover a tiny tail variance.
fn quadrature() -> &'static [(f64, f64); 32] {
    static RULE: OnceLock<[(f64, f64); 32]> = OnceLock::new();
    RULE.get_or_init(|| {
        let mut rule = [(0.0, 0.0); 32];
        for index in 0..16 {
            let mut root = (PI * (index as f64 + 0.75) / 32.5).cos();
            let mut derivative = 0.0;
            for _ in 0..16 {
                let mut previous = 1.0;
                let mut current = root;
                for degree in 2..=32 {
                    let next = ((2 * degree - 1) as f64 * root * current
                        - (degree - 1) as f64 * previous)
                        / degree as f64;
                    previous = current;
                    current = next;
                }
                derivative = 32.0 * (root * current - previous) / (root * root - 1.0);
                let correction = current / derivative;
                root -= correction;
                if correction.abs() <= f64::EPSILON {
                    break;
                }
            }
            let weight = 2.0 / ((1.0 - root * root) * derivative * derivative);
            rule[index] = (-root, weight);
            rule[31 - index] = (root, weight);
        }
        rule
    })
}

fn integrate_moments(lower: f64, upper: f64, density: impl Fn(f64) -> f64) -> (f64, f64, f64) {
    let half_width = (upper - lower) * 0.5;
    let midpoint = lower + half_width;
    let mut values = [(0.0, 0.0); 32];
    let mut mass = 0.0;
    let mut first_moment = 0.0;
    for (index, &(node, weight)) in quadrature().iter().enumerate() {
        let point = midpoint + half_width * node;
        let weighted_density = weight * density(point);
        values[index] = (point, weighted_density);
        mass += weighted_density;
        first_moment += weighted_density * point;
    }
    let mean = first_moment / mass;
    let variance = values
        .iter()
        .map(|(point, weight)| weight * (point - mean).powi(2))
        .sum::<f64>()
        / mass;
    (half_width * mass, mean, variance)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(actual: f64, expected: f64, tolerance: f64) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "actual {actual:.16}, expected {expected:.16}"
        );
    }

    fn settled_items(count: usize) -> Vec<(i64, Rating)> {
        (0..count)
            .map(|index| {
                (
                    index as i64,
                    Rating {
                        mu: 25.0 - index as f64,
                        sigma: 2.5,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn ordinal_updates_match_independent_reference_values() {
        for (answer, mu_a, mu_b, sigma) in [
            (
                Preference::AStrong,
                31.531_993_594_891_578,
                18.468_006_405_108_422,
                6.967_841_567_064_495,
            ),
            (
                Preference::AWeak,
                26.765_126_099_650_537,
                23.234_873_900_349_463,
                6.513_479_595_540_505,
            ),
            (Preference::Equal, 25.0, 25.0, 6.457_235_982_156_567_5),
        ] {
            let (a, b) = update_pair(Rating::default(), Rating::default(), answer).unwrap();
            close(a.mu, mu_a, 1e-10);
            close(b.mu, mu_b, 1e-10);
            close(a.sigma, sigma, 1e-10);
            close(b.sigma, sigma, 1e-10);
        }
    }

    #[test]
    fn swapping_sides_and_answer_preserves_the_update() {
        let a = Rating {
            mu: 41.0,
            sigma: 3.2,
        };
        let b = Rating {
            mu: 17.0,
            sigma: 7.5,
        };
        for (answer, reversed) in [
            (Preference::AStrong, Preference::BStrong),
            (Preference::AWeak, Preference::BWeak),
            (Preference::Equal, Preference::Equal),
        ] {
            let (next_a, next_b) = update_pair(a, b, answer).unwrap();
            let (reverse_b, reverse_a) = update_pair(b, a, reversed).unwrap();
            close(next_a.mu, reverse_a.mu, 1e-12);
            close(next_b.mu, reverse_b.mu, 1e-12);
            close(next_a.sigma, reverse_a.sigma, 1e-12);
            close(next_b.sigma, reverse_b.sigma, 1e-12);
        }
    }

    #[test]
    fn five_outcome_probabilities_partition_the_distribution() {
        for difference in [-1_000.0, -40.0, 0.0, 40.0, 1_000.0] {
            let scale = 8.0;
            let sum: f64 = BOUNDARIES
                .windows(2)
                .map(|bounds| {
                    truncated_moments(
                        (bounds[0] - difference) / scale,
                        (bounds[1] - difference) / scale,
                    )
                    .unwrap()
                    .log_probability
                    .exp()
                })
                .sum();
            close(sum, 1.0, 1e-12);
        }
    }

    #[test]
    fn tail_moments_remain_finite_when_probability_underflows() {
        for lower in [10.0, 40.0, 100.0, 1e8] {
            for upper in [lower + 0.2, f64::INFINITY] {
                let moments = truncated_moments(lower, upper).unwrap();
                assert!(moments.mean >= lower && moments.mean <= upper);
                assert!(moments.variance > 0.0 && moments.variance < 1.0);
                let reflected = truncated_moments(-upper, -lower).unwrap();
                close(reflected.mean, -moments.mean, 1e-12);
                close(reflected.variance, moments.variance, 1e-12);
            }
        }
        let tail = truncated_moments(40.0, f64::INFINITY).unwrap();
        assert_eq!(tail.log_probability.exp(), 0.0);
        close(tail.mean, 40.024_968_847_207_26, 1e-12);
        close(tail.variance, 0.000_622_668_378_591_388_8, 1e-13);
    }

    #[test]
    fn narrow_interval_moments_do_not_lose_the_variance() {
        let moments = truncated_moments(1.0, 1.0 + 1e-8).unwrap();
        close(moments.mean, 1.0 + 0.5e-8, 1e-15);
        close(moments.variance, (1e-8_f64).powi(2) / 12.0, 1e-24);
    }

    #[test]
    fn unexpected_answers_update_without_a_probability_floor() {
        for difference in [100.0, 1_000.0, 1e8] {
            let a = Rating {
                mu: difference,
                sigma: 2.5,
            };
            let b = Rating {
                mu: 0.0,
                sigma: 2.5,
            };
            for answer in [Preference::BStrong, Preference::BWeak, Preference::Equal] {
                let (next_a, next_b) = update_pair(a, b, answer).unwrap();
                assert!(next_a.mu < a.mu && next_b.mu > b.mu);
                assert!(next_a.sigma > 0.0 && next_a.sigma <= a.sigma);
                assert!(next_b.sigma > 0.0 && next_b.sigma <= b.sigma);
            }
        }
    }

    #[test]
    fn repeated_equal_answers_reduce_uncertainty_without_changing_means() {
        let mut pair = (Rating::default(), Rating::default());
        for _ in 0..100 {
            pair = update_pair(pair.0, pair.1, Preference::Equal).unwrap();
        }
        close(pair.0.mu, 25.0, 1e-12);
        close(pair.1.mu, 25.0, 1e-12);
        assert!(pair.0.sigma < MAX_SIGMA);
    }

    #[test]
    fn invalid_inputs_fail_without_producing_ratings() {
        for invalid in [
            Rating {
                mu: f64::NAN,
                sigma: 1.0,
            },
            Rating {
                mu: 25.0,
                sigma: 0.0,
            },
            Rating {
                mu: 25.0,
                sigma: f64::INFINITY,
            },
        ] {
            assert!(update_pair(invalid, Rating::default(), Preference::Equal).is_err());
            assert!(select_pair(&[(1, invalid)], &HashMap::new()).is_err());
        }
    }

    #[test]
    fn ranks_use_mean_then_stable_id_and_do_not_penalize_sigma() {
        let items = [
            (
                8,
                Rating {
                    mu: 30.0,
                    sigma: 8.0,
                },
            ),
            (
                2,
                Rating {
                    mu: 25.0,
                    sigma: 1.0,
                },
            ),
            (
                1,
                Rating {
                    mu: 25.0,
                    sigma: 8.0,
                },
            ),
        ];
        assert_eq!(ranking_ids(&items), vec![8, 1, 2]);
    }

    #[test]
    fn pair_selection_has_canonical_ids_and_count_then_id_ties() {
        let items = [
            (3, Rating::default()),
            (1, Rating::default()),
            (2, Rating::default()),
        ];
        assert_eq!(select_pair(&items, &HashMap::new()).unwrap(), Some((1, 2)));
        let counts = HashMap::from([((1, 2), 1), ((1, 3), 1)]);
        assert_eq!(select_pair(&items, &counts).unwrap(), Some((2, 3)));
        assert_eq!(select_pair(&[], &counts).unwrap(), None);
        assert_eq!(select_pair(&items[..1], &counts).unwrap(), None);
        assert!(select_pair(&[(1, Rating::default()), (1, Rating::default())], &counts).is_err());
    }

    #[test]
    fn uncertain_new_item_is_more_informative_than_a_settled_pair() {
        let settled = Rating {
            mu: 25.0,
            sigma: 1.0,
        };
        let items = [(1, settled), (2, settled), (3, Rating::default())];
        assert_eq!(select_pair(&items, &HashMap::new()).unwrap(), Some((1, 3)));
        assert!(
            information_gain(settled, Rating::default()).unwrap()
                > information_gain(settled, settled).unwrap()
        );
    }

    #[test]
    fn convergence_needs_the_full_window_and_all_items_to_be_certain() {
        let mut items = settled_items(3);
        let snapshots = vec![vec![0, 1, 2]; 21];
        assert!(convergence(&items, &snapshots).converged);
        assert!(!convergence(&items, &snapshots[..20]).converged);
        items[2].1.sigma = INITIAL_SIGMA;
        assert!(!convergence(&items, &snapshots).converged);
        assert!(!convergence(&items[..1], &[]).converged);
    }

    #[test]
    fn convergence_allows_adjacent_swaps_but_detects_transient_large_moves() {
        let items = settled_items(4);
        let mut snapshots = vec![vec![0, 1, 2, 3]; 21];
        snapshots[10] = vec![1, 0, 2, 3];
        let adjacent = convergence(&items, &snapshots);
        assert!(adjacent.converged);
        assert_eq!(adjacent.max_rank_span, Some(1));
        snapshots[10] = vec![3, 1, 2, 0];
        let unstable = convergence(&items, &snapshots);
        assert!(!unstable.converged);
        assert_eq!(unstable.max_rank_span, Some(3));
    }

    #[test]
    fn convergence_rejects_membership_changes_and_uses_only_recent_frames() {
        let items = settled_items(3);
        let mut snapshots = vec![vec![0, 1, 2]; 22];
        snapshots[0] = vec![2, 1, 0];
        assert!(convergence(&items, &snapshots).converged);
        snapshots[1] = vec![0, 1, 4];
        assert!(!convergence(&items, &snapshots).converged);
        snapshots[1] = vec![0, 0, 2];
        assert!(!convergence(&items, &snapshots).converged);
        assert!(!convergence(&items, &[vec![0, 1, 2]]).converged);
        assert_eq!(convergence(&settled_items(500), &[]).required_answers, 500);
    }

    #[test]
    #[ignore = "manual release-mode measurement: cargo test --release full_pair_scan_500 -- --ignored --nocapture"]
    fn full_pair_scan_500() {
        let items: Vec<_> = (0..500)
            .map(|index| {
                (
                    index,
                    Rating {
                        mu: 10.0 + 30.0 * index as f64 / 499.0,
                        sigma: 2.5 + (index % 43) as f64 / 42.0 * (INITIAL_SIGMA - 2.5),
                    },
                )
            })
            .collect();
        let started = std::time::Instant::now();
        let pair = select_pair(&items, &HashMap::new()).unwrap();
        eprintln!(
            "500 items / 124750 pairs: {:?}, selected {pair:?}",
            started.elapsed()
        );
        assert!(pair.is_some());
    }
}
