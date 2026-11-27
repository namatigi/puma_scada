//! Min/max downsampling for trends.
//!
//! The range is split into `max_points / 2` equal time buckets; each bucket
//! emits its minimum and maximum in time order. Peaks and dips survive at any
//! zoom level, which plain averaging or decimation would hide. When the range
//! holds no more than `max_points` points they are returned unchanged.

use serde::Serialize;

/// `[ts, value]`; `null` marks a gap (no value or bad quality).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Point(pub i64, pub Option<f64>);

#[derive(Debug)]
pub struct Downsampler {
    from: i64,
    bucket_ms: f64,
    max_points: usize,
    raw: Option<Vec<Point>>,
    bucket: Option<Bucket>,
    out: Vec<Point>,
}

#[derive(Debug, Clone, Copy)]
struct Bucket {
    index: i64,
    min: Option<(i64, f64)>,
    max: Option<(i64, f64)>,
    first_gap: Option<i64>,
}

impl Downsampler {
    /// `max_points` below 2 is treated as 2 (one bucket's min and max).
    pub fn new(from: i64, to: i64, max_points: usize) -> Self {
        let max_points = max_points.max(2);
        let buckets = (max_points / 2) as f64;
        let span = (to.saturating_sub(from)).max(0) as f64 + 1.0;
        Downsampler {
            from,
            bucket_ms: span / buckets,
            max_points,
            raw: Some(Vec::new()),
            bucket: None,
            out: Vec::new(),
        }
    }

    /// Points must arrive in time order.
    pub fn push(&mut self, ts: i64, value: Option<f64>) {
        let value = value.filter(|v| v.is_finite());
        if let Some(raw) = &mut self.raw {
            if raw.len() < self.max_points {
                raw.push(Point(ts, value));
            } else {
                self.raw = None;
            }
        }
        let index = ((ts - self.from) as f64 / self.bucket_ms).floor() as i64;
        match &mut self.bucket {
            Some(b) if b.index == index => b.add(ts, value),
            _ => {
                if let Some(done) = self.bucket.take() {
                    done.emit(&mut self.out);
                }
                let mut b = Bucket { index, min: None, max: None, first_gap: None };
                b.add(ts, value);
                self.bucket = Some(b);
            }
        }
    }

    pub fn finish(mut self) -> Vec<Point> {
        if let Some(raw) = self.raw {
            return raw;
        }
        if let Some(done) = self.bucket.take() {
            done.emit(&mut self.out);
        }
        self.out
    }
}

impl Bucket {
    fn add(&mut self, ts: i64, value: Option<f64>) {
        match value {
            Some(v) => {
                if self.min.map_or(true, |(_, m)| v < m) {
                    self.min = Some((ts, v));
                }
                if self.max.map_or(true, |(_, m)| v > m) {
                    self.max = Some((ts, v));
                }
            }
            None => {
                self.first_gap.get_or_insert(ts);
            }
        }
    }

    fn emit(self, out: &mut Vec<Point>) {
        match (self.min, self.max) {
            (Some(min), Some(max)) if min.0 == max.0 => out.push(Point(min.0, Some(min.1))),
            (Some(min), Some(max)) => {
                let (a, b) = if min.0 < max.0 { (min, max) } else { (max, min) };
                out.push(Point(a.0, Some(a.1)));
                out.push(Point(b.0, Some(b.1)));
            }
            _ => {
                if let Some(ts) = self.first_gap {
                    out.push(Point(ts, None));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(from: i64, to: i64, n: usize, points: &[(i64, Option<f64>)]) -> Vec<Point> {
        let mut d = Downsampler::new(from, to, n);
        for &(ts, v) in points {
            d.push(ts, v);
        }
        d.finish()
    }

    #[test]
    fn small_ranges_are_returned_unchanged() {
        let pts: Vec<_> = (0..10).map(|i| (i, Some(i as f64))).collect();
        let out = run(0, 9, 10, &pts);
        assert_eq!(out.len(), 10);
        assert_eq!(out[3], Point(3, Some(3.0)));
    }

    #[test]
    fn keeps_extremes_and_respects_the_limit() {
        // 10_000 points of a slow sine with one spike and one dip.
        let mut pts: Vec<_> = (0..10_000).map(|i| (i as i64, Some((i as f64 / 500.0).sin()))).collect();
        pts[1234].1 = Some(50.0);
        pts[8765].1 = Some(-50.0);
        let out = run(0, 9_999, 100, &pts);
        assert!(out.len() <= 100, "{} points", out.len());
        assert!(out.contains(&Point(1234, Some(50.0))));
        assert!(out.contains(&Point(8765, Some(-50.0))));
        assert!(out.windows(2).all(|w| w[0].0 < w[1].0), "time ordered");
        let max = out.iter().filter_map(|p| p.1).fold(f64::MIN, f64::max);
        let min = out.iter().filter_map(|p| p.1).fold(f64::MAX, f64::min);
        assert_eq!((min, max), (-50.0, 50.0));
    }

    #[test]
    fn gaps_survive_only_in_empty_buckets() {
        let mut pts: Vec<_> = (0..1000).map(|i| (i, Some(1.0))).collect();
        for p in &mut pts[500..600] {
            p.1 = None;
        }
        let out = run(0, 999, 20, &pts);
        assert!(out.len() <= 20);
        assert!(out.iter().any(|p| p.1.is_none()), "a fully bad bucket shows as a gap");
    }

    #[test]
    fn odd_limits_and_degenerate_ranges() {
        let pts: Vec<_> = (0..100).map(|i| (i, Some(i as f64))).collect();
        assert!(run(0, 99, 7, &pts).len() <= 7);
        assert_eq!(run(0, 99, 1, &pts).len(), 2, "limit below 2 means one bucket");
        assert_eq!(run(5, 5, 600, &[(5, Some(1.0))]), vec![Point(5, Some(1.0))]);
        assert!(run(0, 10, 600, &[]).is_empty());
    }
}
