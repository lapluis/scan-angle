use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

pub const RESAMPLES: [&str; 6] = [
    "lanczos", "bicubic", "bilinear", "nearest", "box", "hamming",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DetectionSettings {
    pub max_angle: f64,
    pub min_line_percent: f64,
    pub min_line_pixels: f64,
    pub analysis_size: u32,
    pub resample: String,
    pub merge_angle: f64,
    pub merge_distance_percent: f64,
    pub merge_distance_pixels: f64,
    pub merge_gap_percent: f64,
    pub merge_gap_pixels: f64,
}
impl Default for DetectionSettings {
    fn default() -> Self {
        Self {
            max_angle: 15.,
            min_line_percent: 10.,
            min_line_pixels: 50.,
            analysis_size: 2400,
            resample: "lanczos".into(),
            merge_angle: 0.75,
            merge_distance_percent: 0.12,
            merge_distance_pixels: 1.5,
            merge_gap_percent: 0.5,
            merge_gap_pixels: 6.,
        }
    }
}
impl DetectionSettings {
    pub fn validate(&self) -> Result<()> {
        for (name, value, valid) in [
            (
                "max_angle",
                self.max_angle,
                self.max_angle > 0. && self.max_angle <= 45.,
            ),
            (
                "min_line_percent",
                self.min_line_percent,
                self.min_line_percent > 0. && self.min_line_percent <= 100.,
            ),
            (
                "min_line_pixels",
                self.min_line_pixels,
                self.min_line_pixels > 0.,
            ),
            (
                "merge_angle",
                self.merge_angle,
                self.merge_angle > 0. && self.merge_angle < 45.,
            ),
            (
                "merge_distance_percent",
                self.merge_distance_percent,
                self.merge_distance_percent >= 0.,
            ),
            (
                "merge_distance_pixels",
                self.merge_distance_pixels,
                self.merge_distance_pixels >= 0.,
            ),
            (
                "merge_gap_percent",
                self.merge_gap_percent,
                self.merge_gap_percent >= 0.,
            ),
            (
                "merge_gap_pixels",
                self.merge_gap_pixels,
                self.merge_gap_pixels >= 0.,
            ),
        ] {
            if !value.is_finite() || !valid {
                bail!("{name} 必须是有效范围内的有限数值");
            }
        }
        if !(256..=10000).contains(&self.analysis_size) {
            bail!("analysis_size 必须在 256～10000 之间");
        }
        if !RESAMPLES.contains(&self.resample.trim().to_lowercase().as_str()) {
            bail!("resample 必须是 {}", RESAMPLES.join("/"));
        }
        Ok(())
    }
    pub fn min_length(&self, height: u32, width: u32, orientation: Orientation) -> f64 {
        self.min_line_pixels.max(
            self.min_line_percent / 100.
                * match orientation {
                    Orientation::Horizontal => width,
                    Orientation::Vertical => height,
                } as f64,
        )
    }
    pub fn merge_tolerances(&self, height: u32, width: u32) -> (f64, f64) {
        let short = height.min(width) as f64;
        (
            self.merge_distance_pixels
                .max(short * self.merge_distance_percent / 100.),
            self.merge_gap_pixels
                .max(short * self.merge_gap_percent / 100.),
        )
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Orientation {
    Horizontal,
    Vertical,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Line {
    pub coords: [f64; 4],
    pub length: f64,
    pub support_length: f64,
    pub span_length: f64,
    pub gap_length: f64,
    pub angle: f64,
    pub member_count: usize,
    pub orientation: Orientation,
}
#[derive(Clone)]
struct Fragment {
    coords: [f64; 4],
    ux: f64,
    uy: f64,
    length: f64,
    angle: f64,
    orientation: Orientation,
}
fn skew(dx: f64, dy: f64) -> f64 {
    (dy.atan2(dx).to_degrees() + 45.).rem_euclid(90.) - 45.
}
fn round(n: f64, places: i32) -> f64 {
    let factor = 10_f64.powi(places);
    (n * factor).round_ties_even() / factor
}
impl Fragment {
    fn new(mut c: [f64; 4]) -> Option<Self> {
        if !c.iter().all(|x| x.is_finite()) {
            return None;
        }
        let horizontal = (c[2] - c[0]).abs() >= (c[3] - c[1]).abs();
        if (horizontal && (c[0] > c[2] || (c[0] == c[2] && c[1] > c[3])))
            || (!horizontal && (c[1] > c[3] || (c[1] == c[3] && c[0] > c[2])))
        {
            c.swap(0, 2);
            c.swap(1, 3);
        }
        let (dx, dy) = (c[2] - c[0], c[3] - c[1]);
        let length = dx.hypot(dy);
        if length <= 1e-9 {
            return None;
        }
        Some(Self {
            coords: c,
            ux: dx / length,
            uy: dy / length,
            length,
            angle: skew(dx, dy),
            orientation: if horizontal {
                Orientation::Horizontal
            } else {
                Orientation::Vertical
            },
        })
    }
    fn interval(&self, ux: f64, uy: f64) -> (f64, f64) {
        let a = self.coords[0] * ux + self.coords[1] * uy;
        let b = self.coords[2] * ux + self.coords[3] * uy;
        (a.min(b), a.max(b))
    }
    fn compatible(&self, a: &Self, angle: f64, distance: f64) -> bool {
        self.orientation == a.orientation
            && (self.angle - a.angle).abs() <= angle
            && [0, 2].iter().all(|&i| {
                (a.ux * (self.coords[i + 1] - a.coords[1]) - a.uy * (self.coords[i] - a.coords[0]))
                    .abs()
                    <= distance
            })
    }
}
fn add_interval(union: &mut Vec<(f64, f64)>, lo: f64, hi: f64) {
    union.push((lo, hi));
    union.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut merged: Vec<(f64, f64)> = Vec::new();
    for &(a, b) in union.iter() {
        if let Some(last) = merged.last_mut() {
            if a <= last.1 {
                last.1 = last.1.max(b);
                continue;
            }
        }
        merged.push((a, b));
    }
    *union = merged;
}
fn coords_cmp(a: &[f64; 4], b: &[f64; 4]) -> std::cmp::Ordering {
    for i in 0..4 {
        let c = a[i].total_cmp(&b[i]);
        if !c.is_eq() {
            return c;
        }
    }
    std::cmp::Ordering::Equal
}
fn finalize(anchor: &Fragment, members: &[Fragment]) -> Line {
    let mut order: Vec<usize> = (0..members.len()).collect();
    order.sort_by(|&a, &b| {
        members[b]
            .length
            .total_cmp(&members[a].length)
            .then_with(|| coords_cmp(&members[a].coords, &members[b].coords))
    });
    let mut union = Vec::new();
    let mut weights = vec![0.; members.len()];
    for i in order {
        let (lo, hi) = members[i].interval(anchor.ux, anchor.uy);
        let overlap: f64 = union
            .iter()
            .map(|&(a, b): &(f64, f64)| (hi.min(b) - lo.max(a)).max(0.))
            .sum();
        weights[i] = ((hi - lo) - overlap).max(0.);
        add_interval(&mut union, lo, hi);
    }
    let total: f64 = weights.iter().sum();
    let mut ux = members
        .iter()
        .zip(&weights)
        .map(|(m, w)| m.ux * w)
        .sum::<f64>()
        / total;
    let mut uy = members
        .iter()
        .zip(&weights)
        .map(|(m, w)| m.uy * w)
        .sum::<f64>()
        / total;
    let norm = ux.hypot(uy);
    if norm <= 1e-9 {
        ux = anchor.ux;
        uy = anchor.uy;
    } else {
        ux /= norm;
        uy /= norm;
    }
    let offset = members
        .iter()
        .zip(&weights)
        .map(|(m, w)| {
            w * ((m.coords[0] + m.coords[2]) / 2. * (-uy) + (m.coords[1] + m.coords[3]) / 2. * ux)
        })
        .sum::<f64>()
        / total.max(1e-9);
    union.clear();
    for m in members {
        let (lo, hi) = m.interval(ux, uy);
        add_interval(&mut union, lo, hi);
    }
    let support: f64 = union.iter().map(|(a, b)| b - a).sum();
    let lo = union[0].0;
    let hi = union.last().unwrap().1;
    let span = hi - lo;
    Line {
        coords: [
            round(-uy * offset + ux * lo, 4),
            round(ux * offset + uy * lo, 4),
            round(-uy * offset + ux * hi, 4),
            round(ux * offset + uy * hi, 4),
        ],
        length: round(support, 6),
        support_length: round(support, 6),
        span_length: round(span, 6),
        gap_length: round((span - support).max(0.), 6),
        angle: round(skew(ux, uy), 6),
        member_count: members.len(),
        orientation: anchor.orientation,
    }
}
pub fn merge_collinear_segments(
    segments: &[[f64; 4]],
    height: u32,
    width: u32,
    angle: Option<f64>,
    distance: Option<f64>,
    gap: Option<f64>,
) -> Result<Vec<Line>> {
    let angle = angle.unwrap_or(0.75);
    let distance = distance.unwrap_or((height.min(width) as f64 * 0.0012).max(1.5));
    let gap = gap.unwrap_or((height.min(width) as f64 * 0.005).max(6.));
    if !angle.is_finite()
        || angle <= 0.
        || angle >= 45.
        || !distance.is_finite()
        || distance < 0.
        || !gap.is_finite()
        || gap < 0.
    {
        bail!("无效的合并容差");
    }
    let mut pending: Vec<_> = segments.iter().filter_map(|&c| Fragment::new(c)).collect();
    pending.sort_by(|a, b| {
        b.length
            .total_cmp(&a.length)
            .then_with(|| coords_cmp(&a.coords, &b.coords))
    });
    let mut lines = Vec::new();
    while !pending.is_empty() {
        let anchor = &pending[0];
        let mut intervals: Vec<_> = pending
            .iter()
            .enumerate()
            .filter(|(i, f)| *i == 0 || f.compatible(anchor, angle, distance))
            .map(|(i, f)| {
                let (lo, hi) = f.interval(anchor.ux, anchor.uy);
                (lo, hi, i)
            })
            .collect();
        intervals.sort_by(|a, b| {
            a.0.total_cmp(&b.0)
                .then(a.1.total_cmp(&b.1))
                .then_with(|| coords_cmp(&pending[a.2].coords, &pending[b.2].coords))
        });
        let mut selected = Vec::new();
        let mut start = 0;
        while start < intervals.len() {
            let mut end = start + 1;
            let mut hi = intervals[start].1;
            while end < intervals.len() && intervals[end].0 - hi <= gap {
                hi = hi.max(intervals[end].1);
                end += 1;
            }
            if intervals[start..end].iter().any(|x| x.2 == 0) {
                selected = intervals[start..end].iter().map(|x| x.2).collect();
                break;
            }
            start = end;
        }
        let members: Vec<_> = selected.iter().map(|&i| pending[i].clone()).collect();
        lines.push(finalize(anchor, &members));
        let mut remove = vec![false; pending.len()];
        for i in selected {
            remove[i] = true;
        }
        pending = pending
            .into_iter()
            .enumerate()
            .filter(|(i, _)| !remove[*i])
            .map(|(_, f)| f)
            .collect();
    }
    lines.sort_by(|a, b| {
        b.support_length
            .total_cmp(&a.support_length)
            .then(a.orientation.cmp(&b.orientation))
            .then_with(|| coords_cmp(&a.coords, &b.coords))
    });
    Ok(lines)
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thresholds {
    pub horizontal: f64,
    pub vertical: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MergeDiagnostics {
    pub angle_tolerance_deg: f64,
    pub perpendicular_tolerance_px: f64,
    pub gap_tolerance_px: f64,
    pub fragments: usize,
    pub merged: usize,
    pub selected: usize,
    pub ignored_frame_fragments: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Estimate {
    pub angle: Option<f64>,
    pub correction: Option<f64>,
    pub status: String,
    pub count: usize,
    pub spread: Option<f64>,
    pub reason: String,
    pub fragment_count: usize,
    pub merged_count: usize,
    pub merged_lines: Vec<Line>,
    pub thresholds_px: Thresholds,
    pub merge: MergeDiagnostics,
}
pub fn weighted_median(values: &[(f64, f64)]) -> f64 {
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    let half = v.iter().map(|x| x.1).sum::<f64>() / 2.;
    let mut sum = 0.;
    for (x, w) in &v {
        sum += w;
        if sum >= half {
            return *x;
        }
    }
    v.last().map_or(f64::NAN, |x| x.0)
}
pub fn estimate_segments(
    segments: &[[f64; 4]],
    height: u32,
    width: u32,
    s: &DetectionSettings,
) -> Result<(Estimate, Vec<Line>)> {
    s.validate()?;
    let (distance, gap) = s.merge_tolerances(height, width);
    let mut ignored = 0;
    let fragments: Vec<_> = segments
        .iter()
        .filter_map(|&c| Fragment::new(c))
        .filter(|f| f.angle.abs() <= s.max_angle)
        .filter(|f| {
            let c = f.coords;
            let frame = c[0].max(c[2]) < 4.
                || c[0].min(c[2]) > width as f64 - 5.
                || c[1].max(c[3]) < 4.
                || c[1].min(c[3]) > height as f64 - 5.;
            if frame {
                ignored += 1;
            }
            !frame
        })
        .map(|f| f.coords)
        .collect();
    let merged = merge_collinear_segments(
        &fragments,
        height,
        width,
        Some(s.merge_angle),
        Some(distance),
        Some(gap),
    )?;
    let mut selected: Vec<_> = merged
        .iter()
        .filter(|l| {
            l.support_length >= s.min_length(height, width, l.orientation)
                && l.angle.abs() <= s.max_angle
        })
        .cloned()
        .collect();
    let mut result = Estimate {
        angle: None,
        correction: None,
        status: "需人工确认".into(),
        count: 0,
        spread: None,
        reason: "没有足够的长直线；圆形、无边框或低对比图片可能无法判断".into(),
        fragment_count: fragments.len(),
        merged_count: merged.len(),
        thresholds_px: Thresholds {
            horizontal: round(s.min_length(height, width, Orientation::Horizontal), 3),
            vertical: round(s.min_length(height, width, Orientation::Vertical), 3),
        },
        merge: MergeDiagnostics {
            angle_tolerance_deg: s.merge_angle,
            perpendicular_tolerance_px: distance,
            gap_tolerance_px: gap,
            fragments: fragments.len(),
            merged: merged.len(),
            selected: selected.len(),
            ignored_frame_fragments: ignored,
        },
        merged_lines: merged,
    };
    if selected.is_empty() {
        return Ok((result, selected));
    }
    let total: f64 = selected.iter().map(|l| l.support_length).sum();
    let mut center = selected[0].angle;
    let mut best = -1.;
    for l in &selected {
        let support = selected
            .iter()
            .filter(|x| (x.angle - l.angle).abs() <= 0.35)
            .map(|x| x.support_length)
            .sum::<f64>();
        if support > best {
            best = support;
            center = l.angle;
        }
    }
    center = weighted_median(
        &selected
            .iter()
            .filter(|x| (x.angle - center).abs() <= 0.35)
            .map(|x| (x.angle, x.support_length))
            .collect::<Vec<_>>(),
    );
    selected.retain(|x| (x.angle - center).abs() <= 0.35);
    let angle = weighted_median(
        &selected
            .iter()
            .map(|x| (x.angle, x.support_length))
            .collect::<Vec<_>>(),
    );
    let support: f64 = selected.iter().map(|l| l.support_length).sum();
    let spread = (selected
        .iter()
        .map(|l| (l.angle - angle).powi(2) * l.support_length)
        .sum::<f64>()
        / support)
        .sqrt();
    let mut groups: Vec<(Orientation, f64)> = Vec::new();
    let short = height.min(width) as f64;
    for l in &selected {
        let c = l.coords;
        let mx = (c[0] + c[2]) / 2.;
        let my = (c[1] + c[3]) / 2.;
        let intercept = match l.orientation {
            Orientation::Horizontal => my - angle.to_radians().tan() * mx,
            Orientation::Vertical => mx + angle.to_radians().tan() * my,
        };
        if groups
            .iter()
            .all(|&(o, p)| o != l.orientation || (p - intercept).abs() > short * 0.025)
        {
            groups.push((l.orientation, intercept));
        }
    }
    let trusted =
        groups.len() >= 2 && support >= short * 0.7 && support / total >= 0.65 && spread < 0.18;
    result.status = if trusted { "可信" } else { "需人工确认" }.into();
    result.reason = if trusted {
        "多条分离直线方向一致"
    } else {
        "直线证据不足或方向存在冲突；请核对预览"
    }
    .into();
    if angle.abs() > s.max_angle - 0.3 {
        result.status = "需人工确认".into();
        result.reason = "角度接近检测范围上限；建议扩大范围复核".into();
    }
    result.angle = Some(round(angle, 3));
    result.correction = Some(round(-angle, 3));
    result.spread = Some(round(spread, 3));
    result.count = selected.len();
    result.merge.selected = selected.len();
    Ok((result, selected))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn python_merge_goldens() {
        let cases: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/merge_golden.json")).unwrap();
        for (i, c) in cases.as_array().unwrap().iter().enumerate() {
            let segments: Vec<[f64; 4]> = serde_json::from_value(c["segments"].clone()).unwrap();
            let shape = c["shape"].as_array().unwrap();
            let o = &c["options"];
            let got = merge_collinear_segments(
                &segments,
                shape[0].as_u64().unwrap() as u32,
                shape[1].as_u64().unwrap() as u32,
                o["angle_tolerance_deg"].as_f64(),
                o["perpendicular_tolerance_px"].as_f64(),
                o["gap_tolerance_px"].as_f64(),
            )
            .unwrap();
            let want: Vec<Line> = serde_json::from_value(c["expected"].clone()).unwrap();
            assert_eq!(got.len(), want.len(), "case {i}");
            for (a, b) in got.iter().zip(&want) {
                assert_eq!(a.orientation, b.orientation);
                assert_eq!(a.member_count, b.member_count);
                for (x, y) in a.coords.iter().zip(&b.coords) {
                    assert!((x - y).abs() < 0.00011, "case {i}: coords");
                }
                for (x, y) in [
                    (a.angle, b.angle),
                    (a.support_length, b.support_length),
                    (a.span_length, b.span_length),
                    (a.gap_length, b.gap_length),
                ] {
                    assert!((x - y).abs() < 0.000002, "case {i}: {x} != {y}");
                }
            }
        }
    }
    #[test]
    fn settings_and_zero_tolerances() {
        let mut s = DetectionSettings::default();
        assert!(s.validate().is_ok());
        for v in [f64::NAN, f64::INFINITY, -1., 0., 101.] {
            s.min_line_percent = v;
            assert!(s.validate().is_err());
        }
        s.min_line_percent = 10.;
        s.merge_distance_pixels = 0.;
        s.merge_distance_percent = 0.;
        s.merge_gap_pixels = 0.;
        s.merge_gap_percent = 0.;
        assert!(s.validate().is_ok());
        assert_eq!(s.merge_tolerances(1000, 4000), (0., 0.));
        assert_eq!(s.min_length(4000, 1000, Orientation::Horizontal), 100.);
        assert_eq!(s.min_length(4000, 1000, Orientation::Vertical), 400.);
    }
    #[test]
    fn directional_filter_and_diagnostics() {
        let s = DetectionSettings::default();
        let (r, lines) = estimate_segments(
            &[[50., 100., 250., 100.], [100., 50., 100., 250.]],
            400,
            4000,
            &s,
        )
        .unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].orientation, Orientation::Vertical);
        assert_eq!(r.thresholds_px.horizontal, 400.);
        assert_eq!(r.thresholds_px.vertical, 50.);
        assert_eq!(r.merge.selected, 1);
    }
}
