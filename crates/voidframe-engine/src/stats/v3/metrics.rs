//! The 3 new WCPS v3 pacing/throughput metrics -- a direct port of
//! `study/research/scripts/metrics_v3.py`. Uses the same prefix-sum +
//! binary-search technique that script's own `_moving_average_vectorized`
//! uses (not the O(n^2) naive backward-walk `_moving_average_naive` --
//! that Python reference implementation exists there ONLY as a correctness
//! check on the vectorized one, per that module's own docstring; this port
//! goes straight to the fast version, verified against real fixture data
//! generated from the (already-cross-checked-against-naive) Python
//! `adaptive_frame_time_cv`/`stutter_count_pct`, not re-deriving that
//! naive-vs-vectorized equivalence a second time in Rust).

/// For each frame `i`, the largest trailing window (by summed duration, not
/// count) ending at `i` whose total is `< window_ms` -- returns
/// `(local_sum, local_count)` per frame. Requires `frame_times_ms`
/// non-empty and every element strictly positive (true of every real
/// PresentMon `MsBetweenPresents` column).
fn trailing_window_sums(frame_times_ms: &[f64], window_ms: f64) -> Vec<(f64, usize)> {
    let n = frame_times_ms.len();
    let mut prefix = vec![0.0_f64; n + 1];
    for i in 0..n {
        prefix[i + 1] = prefix[i] + frame_times_ms[i];
    }
    (0..n)
        .map(|i| {
            let target = prefix[i + 1] - window_ms;
            // Largest l such that prefix[l] <= target (prefix is strictly
            // increasing since every frame time is > 0) -- Rust's
            // `partition_point` on `prefix[..=i]` gives exactly the
            // `searchsorted(..., side='right') - 1` the Python reference
            // uses, restricted to indices `0..=i` (matching that
            // reference's own `np.clip(l_idx, 0, np.arange(n))` bound).
            let l = prefix[..=i]
                .partition_point(|&p| p <= target)
                .saturating_sub(1);
            let local_sum = prefix[i + 1] - prefix[l];
            let local_count = i - l + 1;
            (local_sum, local_count)
        })
        .collect()
}

/// CapFrameX's `GetAdaptiveStandardDeviation`: uncentered RMS of residuals
/// around the time-windowed backward moving average, `n-1` denominator.
pub fn adaptive_frame_time_std(frame_times_ms: &[f64], window_ms: f64) -> f64 {
    let n = frame_times_ms.len();
    let windows = trailing_window_sums(frame_times_ms, window_ms);
    let residual_sq_sum: f64 = frame_times_ms
        .iter()
        .zip(&windows)
        .map(|(&ft, &(sum, count))| {
            let moving_avg = sum / count as f64;
            (ft - moving_avg).powi(2)
        })
        .sum();
    (residual_sq_sum / (n as f64 - 1.0)).sqrt()
}

/// VOIDFRAME's own normalization: `adaptive_frame_time_std` divided by the
/// whole-capture mean frame time, matching `metrics_v3.py::adaptive_frame_time_cv`.
pub fn adaptive_frame_time_cv(frame_times_ms: &[f64], window_ms: f64) -> f64 {
    let std = adaptive_frame_time_std(frame_times_ms, window_ms);
    let mean_ft = frame_times_ms.iter().sum::<f64>() / frame_times_ms.len() as f64;
    std / mean_ft
}

/// CapFrameX's `GetStutteringCountPercentage`: `100 * count(ft > k*mean(ft)) / n`.
pub fn stutter_count_pct(frame_times_ms: &[f64], k: f64) -> f64 {
    let mean_ft = frame_times_ms.iter().sum::<f64>() / frame_times_ms.len() as f64;
    let threshold = k * mean_ft;
    let count = frame_times_ms.iter().filter(|&&ft| ft > threshold).count();
    100.0 * count as f64 / frame_times_ms.len() as f64
}

/// PresentMon's own `MsAnimationError` column, absolute value (frame 0's
/// always-NA value must already be dropped by the caller before this is
/// invoked -- matches `metrics_v3.py::mean_abs_animation_error_ms`'s own
/// documented caller contract).
pub fn mean_abs_animation_error_ms(animation_error_ms: &[f64]) -> f64 {
    animation_error_ms.iter().map(|v| v.abs()).sum::<f64>() / animation_error_ms.len() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mean_abs_animation_error_ms_of_a_hand_computed_array() {
        assert_eq!(mean_abs_animation_error_ms(&[1.0, -2.0, 3.0]), 2.0);
    }
}
