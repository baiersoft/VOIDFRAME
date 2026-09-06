//! Hotelling's two-sample T-squared statistic -- a direct structural port
//! of `study/research/scripts/hotelling_generic.py`'s `_standardize` +
//! `hotelling_t2_statistic`. Verified against that Python implementation's
//! actual output via `tests/wcps_v3_golden.rs`'s fixture-driven tests, not
//! merely "translated and assumed correct."

use super::spd_inverse;

const RIDGE_FRACTION: f64 = 1e-3; // matches hotelling_generic.py's own constant exactly

/// Column-wise mean of an (n, P) row-major slice.
fn column_means<const P: usize>(matrix: &[[f64; P]]) -> [f64; P] {
    let n = matrix.len() as f64;
    let mut out = [0.0_f64; P];
    for row in matrix {
        for j in 0..P {
            out[j] += row[j];
        }
    }
    for v in &mut out {
        *v /= n;
    }
    out
}

/// Column-wise sample standard deviation (`ddof=1`), matching
/// `baseline_matrix.std(axis=0, ddof=1)` exactly -- including that Python's
/// behavior for `n=1` (returns NaN, since dividing by `n-1=0`) is
/// intentionally reproduced here, not guarded against: this module's
/// caller (`hotelling_t2_statistic`) is never called with `n=1` inputs from
/// any real path in this codebase (calibrated thresholds only exist for
/// `n=2` and above, see `study/research/wcps-v3-audit.md`'s confirmed n=1
/// finding) -- matching the Python reference's own real behavior exactly is
/// more important here than adding a Rust-only safety net Python doesn't
/// have, since divergent n=1 behavior would itself be a port bug.
fn column_stddev<const P: usize>(matrix: &[[f64; P]], means: &[f64; P]) -> [f64; P] {
    let n = matrix.len() as f64;
    let mut sum_sq = [0.0_f64; P];
    for row in matrix {
        for j in 0..P {
            sum_sq[j] += (row[j] - means[j]).powi(2);
        }
    }
    let mut out = [0.0_f64; P];
    for j in 0..P {
        out[j] = (sum_sq[j] / (n - 1.0)).sqrt();
    }
    out
}

fn standardize<const P: usize>(
    baseline: &[[f64; P]],
    scenario: &[[f64; P]],
) -> (Vec<[f64; P]>, Vec<[f64; P]>) {
    let mu = column_means(baseline);
    let mut sigma = column_stddev(baseline, &mu);
    for s in &mut sigma {
        if s.abs() < 1e-9 {
            *s = 1e-9;
        }
    }
    let standardize_one = |row: &[f64; P]| -> [f64; P] {
        let mut out = [0.0_f64; P];
        for j in 0..P {
            out[j] = (row[j] - mu[j]) / sigma[j];
        }
        out
    };
    (
        baseline.iter().map(standardize_one).collect(),
        scenario.iter().map(standardize_one).collect(),
    )
}

fn covariance<const P: usize>(rows: &[[f64; P]]) -> [[f64; P]; P] {
    let n = rows.len();
    let mut cov = [[0.0_f64; P]; P];
    if n <= 1 {
        return cov; // matches Python's `np.zeros((p, p))` for n1<=1
    }
    let means = column_means(rows);
    for row in rows {
        for i in 0..P {
            for j in 0..P {
                cov[i][j] += (row[i] - means[i]) * (row[j] - means[j]);
            }
        }
    }
    let denom = (n - 1) as f64;
    for row in &mut cov {
        for v in row.iter_mut() {
            *v /= denom;
        }
    }
    cov
}

/// The raw Hotelling T-squared statistic between `baseline` and `scenario`
/// (each an (n, P) row-major slice, `P` fixed at compile time -- `P=4` for
/// the throughput vector, `P=2` for the pacing vector). Standardized
/// (baseline-relative z-scoring), ridge-regularized pooled covariance,
/// exact inverse (see `linalg.rs`'s module doc for why an exact inverse is
/// mathematically equivalent to `hotelling_generic.py`'s
/// `numpy.linalg.pinv` for every input this function actually receives).
pub fn hotelling_t2_statistic<const P: usize>(baseline: &[[f64; P]], scenario: &[[f64; P]]) -> f64 {
    let (bz, sz) = standardize(baseline, scenario);
    let n1 = bz.len() as f64;
    let n2 = sz.len() as f64;

    let bz_mean = column_means(&bz);
    let sz_mean = column_means(&sz);
    let mut diff = [0.0_f64; P];
    for j in 0..P {
        diff[j] = bz_mean[j] - sz_mean[j];
    }

    let s1 = covariance(&bz);
    let s2 = covariance(&sz);
    let denom = (n1 + n2 - 2.0).max(1.0);
    let mut sp = [[0.0_f64; P]; P];
    for i in 0..P {
        for j in 0..P {
            sp[i][j] = ((n1 - 1.0) * s1[i][j] + (n2 - 1.0) * s2[i][j]) / denom;
        }
    }

    let trace: f64 = (0..P).map(|i| sp[i][i]).sum();
    let ridge = RIDGE_FRACTION * if trace > 0.0 { trace / P as f64 } else { 1.0 };
    for (i, row) in sp.iter_mut().enumerate() {
        row[i] += ridge;
    }

    // `spd_inverse` cannot fail here: `sp` post-ridge is guaranteed
    // symmetric positive definite (see `linalg.rs`'s module doc) --
    // `.expect` is correct, not a shortcut, since a failure here would mean
    // the ridge-regularization argument itself is wrong, which
    // `docs/superpowers/plans/2026-09-04-wcps-v3-rust-port.md`'s
    // fixture-generation task's fixtures (real data, real ridge value) are what actually confirm.
    let sp_inv = spd_inverse(&sp).expect("ridge-regularized pooled covariance is always SPD");

    let scale = (n1 * n2) / (n1 + n2);
    let mut t2 = 0.0;
    for i in 0..P {
        let mut row_sum = 0.0;
        for j in 0..P {
            row_sum += sp_inv[i][j] * diff[j];
        }
        t2 += diff[i] * row_sum;
    }
    scale * t2
}
