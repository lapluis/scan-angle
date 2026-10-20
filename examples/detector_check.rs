use scan_angle::detection::{DetectionSettings, estimate_segments};
fn main() {
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("../tests/fixtures/images_golden.json")).unwrap();
    for c in cases.as_array().unwrap() {
        let name = c["file"].as_str().unwrap();
        let im = image::open(format!("tests/fixtures/{name}"))
            .unwrap()
            .to_rgb8();
        let gray: Vec<u8> = im
            .pixels()
            .map(|p| {
                ((4899 * p[0] as u32 + 9617 * p[1] as u32 + 1868 * p[2] as u32 + 8192) >> 14) as u8
            })
            .collect();
        let seg = lsdetect::detect(
            &gray,
            im.width() as usize,
            im.height() as usize,
            lsdetect::SCALE,
        )
        .unwrap();
        let coords: Vec<_> = seg.iter().map(|s| [s.x1, s.y1, s.x2, s.y2]).collect();
        let (r, _) = estimate_segments(
            &coords,
            im.height(),
            im.width(),
            &DetectionSettings::default(),
        )
        .unwrap();
        println!(
            "{name}: {:?} {} (expected {} {})",
            r.angle, r.status, c["angle"], c["status"]
        );
    }
}
