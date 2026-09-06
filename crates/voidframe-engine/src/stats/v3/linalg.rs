//! An SPD (symmetric POSITIVE DEFINITE) matrix inverse, via `nalgebra`'s
//! Cholesky decomposition -- specifically NOT a general Moore-Penrose
//! pseudoinverse.
//!
//! `hotelling_generic.py`'s `hotelling_t2_statistic` computes
//! `sp_reg = sp + ridge * I` where `sp` is a pooled sample covariance
//! (always positive SEMI-definite by construction -- a convex combination
//! of two covariance matrices) and `ridge` is a strictly positive scalar
//! (`RIDGE_FRACTION * (trace(sp)/p if trace(sp) > 0 else 1.0)`, and
//! `RIDGE_FRACTION = 1e-3` is a fixed positive constant, so `ridge > 0`
//! ALWAYS, even when `trace(sp) == 0`). Adding a strictly-positive multiple
//! of the identity to a positive-semi-definite matrix makes the result
//! symmetric POSITIVE DEFINITE, not merely semi-definite -- and a full-rank
//! square positive-definite matrix's Moore-Penrose pseudoinverse is EXACTLY
//! its ordinary inverse (there is no rank deficiency left to pseudo-invert
//! around). This means `numpy.linalg.pinv(sp_reg)` in the Python reference
//! is mathematically identical to `numpy.linalg.inv(sp_reg)` for every
//! input this function will ever actually receive -- confirmed by this
//! module's own construction, not assumed;
//! `docs/superpowers/plans/2026-09-04-wcps-v3-rust-port.md`'s Hotelling
//! golden-value fixtures (generated from the real `pinv` call) are what actually verify this
//! holds for real data, not just the argument above.
//!
//! `nalgebra::Cholesky::new` returns `None` when the input isn't actually
//! positive definite -- surfaced here as an `Error`, not a panic, so a
//! future misuse fails loudly instead of returning silently-wrong numbers.

use crate::error::{Error, Result};
use nalgebra::{Cholesky, SMatrix};

/// The inverse of a symmetric positive-definite `P`x`P` matrix, via
/// `nalgebra`'s Cholesky decomposition. See module doc for why this (not a
/// general pseudoinverse) is the correct primitive for this crate's actual
/// caller.
pub fn spd_inverse<const P: usize>(m: &[[f64; P]; P]) -> Result<[[f64; P]; P]> {
    // `SMatrix::from_fn` takes `(row, col)`, matching this module's own
    // row-major `[[f64; P]; P]` indexing convention exactly.
    let matrix = SMatrix::<f64, P, P>::from_fn(|i, j| m[i][j]);
    let chol = Cholesky::new(matrix).ok_or_else(|| {
        Error::msg(
            "spd_inverse: matrix is not positive definite (Cholesky decomposition failed)".into(),
        )
    })?;
    let inv = chol.inverse();
    let mut result = [[0.0_f64; P]; P];
    for i in 0..P {
        for j in 0..P {
            result[i][j] = inv[(i, j)];
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matmul<const P: usize>(a: &[[f64; P]; P], b: &[[f64; P]; P]) -> [[f64; P]; P] {
        let mut out = [[0.0_f64; P]; P];
        for i in 0..P {
            for j in 0..P {
                out[i][j] = (0..P).map(|k| a[i][k] * b[k][j]).sum();
            }
        }
        out
    }

    fn assert_approx_identity<const P: usize>(m: &[[f64; P]; P], tol: f64) {
        for (i, row) in m.iter().enumerate() {
            for (j, &val) in row.iter().enumerate() {
                let expected = if i == j { 1.0 } else { 0.0 };
                assert!(
                    (val - expected).abs() < tol,
                    "not close to identity at ({i},{j}): got {val}, expected {expected}"
                );
            }
        }
    }

    #[test]
    fn inverts_a_known_2x2_matrix_by_hand() {
        // [[4,2],[2,3]] is SPD (both leading principal minors positive: 4>0,
        // 4*3-2*2=8>0). Known inverse: (1/8)*[[3,-2],[-2,4]].
        let m = [[4.0, 2.0], [2.0, 3.0]];
        let inv = spd_inverse(&m).unwrap();
        assert!((inv[0][0] - 0.375).abs() < 1e-12);
        assert!((inv[0][1] - -0.25).abs() < 1e-12);
        assert!((inv[1][0] - -0.25).abs() < 1e-12);
        assert!((inv[1][1] - 0.5).abs() < 1e-12);
    }

    #[test]
    fn inverting_the_2x2_identity_returns_the_identity() {
        let m = [[1.0, 0.0], [0.0, 1.0]];
        let inv = spd_inverse(&m).unwrap();
        assert_approx_identity(&inv, 1e-12);
    }

    #[test]
    fn product_of_a_4x4_matrix_and_its_inverse_is_the_identity() {
        // A real ridge-regularized-looking 4x4: diagonally dominant, so
        // guaranteed SPD without hand-verifying every minor.
        let m = [
            [2.5, 0.3, 0.1, 0.05],
            [0.3, 1.8, 0.2, 0.1],
            [0.1, 0.2, 3.1, 0.15],
            [0.05, 0.1, 0.15, 1.2],
        ];
        let inv = spd_inverse(&m).unwrap();
        let product = matmul(&m, &inv);
        assert_approx_identity(&product, 1e-9);
    }

    #[test]
    fn non_positive_definite_matrix_errors_rather_than_returning_wrong_numbers() {
        // [[1,2],[2,1]] has determinant 1-4=-3 < 0 -- not PD (eigenvalues
        // are 3 and -1).
        let m = [[1.0, 2.0], [2.0, 1.0]];
        let err = spd_inverse(&m).unwrap_err();
        assert!(err.is_msg());
    }
}
