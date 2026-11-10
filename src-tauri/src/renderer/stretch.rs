//! Image-space stretch tables. Statistics exclude transparent and masked pixels.
//!
//! The caller supplies one bounded, whole-image statistics sample; the resulting
//! RGB-interleaved table must be shared by every output tile.

fn identity() -> Vec<u8> {
    (0..=255).flat_map(|value| [value as u8; 3]).collect()
}

fn quantile(histogram: &[u64; 256], count: u64, fraction: f64) -> usize {
    let rank = (fraction.clamp(0.0, 1.0) * (count - 1) as f64).floor() as u64;
    let mut cumulative = 0;
    for (value, frequency) in histogram.iter().enumerate() {
        cumulative += frequency;
        if cumulative > rank {
            return value;
        }
    }
    255
}

/// Build 256 RGB entries (768 bytes), indexed as `value * 3 + channel`.
///
/// `mask`, when present, has one byte per input pixel; zero excludes that pixel.
/// A short mask excludes missing entries. Incomplete trailing RGBA bytes are
/// ignored. Unknown modes, empty samples and constant channels retain identity.
/// Percent clipping parameters are percentages, independently measured at each
/// end. Invalid or overlapping ranges retain identity rather than divide by zero.
pub fn build_lut(
    rgba: &[u8],
    mask: Option<&[u8]>,
    mode: &str,
    deviations: f32,
    low: f32,
    high: f32,
) -> Vec<u8> {
    let mut table = identity();
    if !matches!(mode, "minmax" | "stddev" | "equalize" | "percentclip") {
        return table;
    }
    let mut histograms = [[0_u64; 256]; 3];
    let mut count = 0_u64;
    for (index, pixel) in rgba.chunks_exact(4).enumerate() {
        if pixel[3] == 0 || mask.is_some_and(|values| values.get(index).copied().unwrap_or(0) == 0)
        {
            continue;
        }
        count += 1;
        for channel in 0..3 {
            histograms[channel][pixel[channel] as usize] += 1;
        }
    }
    if count == 0 {
        return table;
    }
    let percent = |value: f32| {
        if value.is_finite() {
            (value as f64).clamp(0.0, 100.0) / 100.0
        } else {
            0.0
        }
    };
    let low = percent(low);
    let high = percent(high);
    let deviations = if deviations.is_finite() && deviations > 0.0 {
        deviations as f64
    } else {
        2.0
    };
    for channel in 0..3 {
        let histogram = &histograms[channel];
        let minimum = histogram
            .iter()
            .position(|&frequency| frequency > 0)
            .unwrap();
        let maximum = histogram
            .iter()
            .rposition(|&frequency| frequency > 0)
            .unwrap();
        if minimum == maximum {
            continue;
        }
        if mode == "equalize" {
            let cdf_minimum = histogram[minimum];
            let denominator = (count - cdf_minimum) as f64;
            let mut cumulative = 0_u64;
            for value in 0..256 {
                cumulative += histogram[value];
                table[value * 3 + channel] =
                    ((cumulative.saturating_sub(cdf_minimum) as f64 / denominator) * 255.0).round()
                        as u8;
            }
            continue;
        }
        let (lower, upper) = match mode {
            "stddev" => {
                let mean = histogram
                    .iter()
                    .enumerate()
                    .map(|(value, &frequency)| value as f64 * frequency as f64)
                    .sum::<f64>()
                    / count as f64;
                let variance = histogram
                    .iter()
                    .enumerate()
                    .map(|(value, &frequency)| (value as f64 - mean).powi(2) * frequency as f64)
                    .sum::<f64>()
                    / count as f64;
                let radius = deviations * variance.sqrt();
                ((mean - radius).max(0.0), (mean + radius).min(255.0))
            }
            "percentclip" => {
                if low + high >= 1.0 {
                    continue;
                }
                (
                    quantile(histogram, count, low) as f64,
                    quantile(histogram, count, 1.0 - high) as f64,
                )
            }
            _ => (minimum as f64, maximum as f64),
        };
        if upper <= lower || !lower.is_finite() || !upper.is_finite() {
            continue;
        }
        for value in 0..256 {
            table[value * 3 + channel] =
                (((value as f64 - lower) / (upper - lower)).clamp(0.0, 1.0) * 255.0).round() as u8;
        }
    }
    table
}

fn identity16() -> Vec<u16> {
    (0..=65535).flat_map(|value| [value as u16; 3]).collect()
}

fn quantile16(histogram: &[u64], count: u64, fraction: f64) -> usize {
    let rank = (fraction.clamp(0.0, 1.0) * (count - 1) as f64).floor() as u64;
    let mut cumulative = 0;
    for (value, frequency) in histogram.iter().enumerate() {
        cumulative += frequency;
        if cumulative > rank {
            return value;
        }
    }
    65535
}

/// Full 16-bit RGB mapping; input and output retain all 65536 code values.
pub fn build_lut16(
    rgba: &[u16],
    mask: Option<&[u8]>,
    mode: &str,
    deviations: f32,
    low: f32,
    high: f32,
) -> Vec<u16> {
    let mut table = identity16();
    if !matches!(mode, "minmax" | "stddev" | "equalize" | "percentclip") {
        return table;
    }
    let mut histograms = vec![vec![0_u64; 65536]; 3];
    let mut count = 0_u64;
    for (index, pixel) in rgba.chunks_exact(4).enumerate() {
        if pixel[3] == 0 || mask.is_some_and(|values| values.get(index).copied().unwrap_or(0) == 0)
        {
            continue;
        }
        count += 1;
        for channel in 0..3 {
            histograms[channel][pixel[channel] as usize] += 1;
        }
    }
    if count == 0 {
        return table;
    }
    let percent = |value: f32| {
        if value.is_finite() {
            (value as f64).clamp(0.0, 100.0) / 100.0
        } else {
            0.0
        }
    };
    let low = percent(low);
    let high = percent(high);
    let deviations = if deviations.is_finite() && deviations > 0.0 {
        deviations as f64
    } else {
        2.0
    };
    for channel in 0..3 {
        let histogram = &histograms[channel];
        let minimum = histogram
            .iter()
            .position(|&frequency| frequency > 0)
            .unwrap();
        let maximum = histogram
            .iter()
            .rposition(|&frequency| frequency > 0)
            .unwrap();
        if minimum == maximum {
            continue;
        }
        if mode == "equalize" {
            let cdf_minimum = histogram[minimum];
            let denominator = (count - cdf_minimum) as f64;
            let mut cumulative = 0_u64;
            for value in 0..65536 {
                cumulative += histogram[value];
                table[value * 3 + channel] =
                    ((cumulative.saturating_sub(cdf_minimum) as f64 / denominator) * 65535.0).round()
                        as u16;
            }
            continue;
        }
        let (lower, upper) = match mode {
            "stddev" => {
                let mean = histogram
                    .iter()
                    .enumerate()
                    .map(|(value, &frequency)| value as f64 * frequency as f64)
                    .sum::<f64>()
                    / count as f64;
                let variance = histogram
                    .iter()
                    .enumerate()
                    .map(|(value, &frequency)| (value as f64 - mean).powi(2) * frequency as f64)
                    .sum::<f64>()
                    / count as f64;
                let radius = deviations * variance.sqrt();
                ((mean - radius).max(0.0), (mean + radius).min(65535.0))
            }
            "percentclip" => {
                if low + high >= 1.0 {
                    continue;
                }
                (
                    quantile16(histogram, count, low) as f64,
                    quantile16(histogram, count, 1.0 - high) as f64,
                )
            }
            _ => (minimum as f64, maximum as f64),
        };
        if upper <= lower || !lower.is_finite() || !upper.is_finite() {
            continue;
        }
        for value in 0..65536 {
            table[value * 3 + channel] =
                (((value as f64 - lower) / (upper - lower)).clamp(0.0, 1.0) * 65535.0).round() as u16;
        }
    }
    table
}
