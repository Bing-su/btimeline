use std::collections::BTreeMap;

use anyhow::{Context, Result, ensure};
use serde::Serialize;

pub(super) fn median(values: impl Iterator<Item = i64>) -> Result<i64> {
    let mut values: Vec<_> = values.collect();
    ensure!(!values.is_empty(), "Missing timing samples");
    values.sort_unstable();
    values
        .get((values.len() - 1) / 2)
        .copied()
        .context("Missing median")
}

#[derive(Debug, Serialize)]
pub(super) struct Sample {
    trace: String,
    at_ms: i64,
    origin_ms: i64,
    offset_ms: i64,
}

#[derive(Debug)]
pub(super) struct Estimate {
    pub samples: Vec<Sample>,
    pub median_ms: i64,
    pub tick: i64,
    pub min_tick: i64,
    pub max_tick: i64,
}

// Estimate a practical schedule from bound occurrences; large offsets are not correspondence errors.
pub(super) fn estimate(
    samples: &BTreeMap<String, i64>,
    origins: &BTreeMap<String, i64>,
) -> Result<Estimate> {
    let samples = samples
        .iter()
        .map(|(trace, at)| {
            let origin = *origins.get(trace).context("Missing origin sample")?;
            let offset_ms = at.checked_sub(origin).context("Timing offset overflow")?;
            Ok(Sample {
                trace: trace.clone(),
                at_ms: *at,
                origin_ms: origin,
                offset_ms,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    ensure!(
        samples
            .iter()
            .all(|sample| (0..=crate::timeline::MAX_MS).contains(&sample.offset_ms)),
        "Sample exceeds supported time range"
    );
    let median_ms = median(samples.iter().map(|sample| sample.offset_ms))?;
    let tick = (median_ms + 50) / 100;
    let min_tick = samples
        .iter()
        .map(|sample| (sample.offset_ms + 50) / 100)
        .min()
        .context("Missing sync samples")?;
    let max_tick = samples
        .iter()
        .map(|sample| (sample.offset_ms + 50) / 100)
        .max()
        .context("Missing sync samples")?;
    Ok(Estimate {
        samples,
        median_ms,
        tick,
        min_tick,
        max_tick,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[rstest::rstest]
    #[case(30000, 300)]
    #[case(90000, 900)]
    fn large_clock_adjustments_do_not_remove_a_bound_occurrence(
        #[case] shift: i64,
        #[case] maximum: i64,
    ) {
        let times = BTreeMap::from([("first".into(), 10000), ("second".into(), 10000 + shift)]);
        let origins = BTreeMap::from([("first".into(), 0), ("second".into(), 0)]);
        let estimated = estimate(&times, &origins).unwrap();
        assert_eq!(estimated.samples.len(), 2);
        assert_eq!(estimated.median_ms, 10000);
        assert_eq!(estimated.max_tick - estimated.min_tick, maximum);
        let corrected = estimate(
            &times,
            &BTreeMap::from([("first".into(), 0), ("second".into(), shift)]),
        )
        .unwrap();
        assert_eq!(corrected.min_tick, corrected.max_tick);
    }
}
