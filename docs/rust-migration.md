# Rust migration

Baseline: a0538d6; Python unittest suite: 58 passed (2026-10-20).

Stages:
1. Capture synthetic regression fixtures from Python and preserve behavior contracts.
2. Port validated settings, stable-anchor merging, evidence filtering and confidence.
3. Implement image decoding/resampling, batch CSV/JSON/previews, CLI and native GUI.
4. Replace Python runtime/build/CI/docs; complete regression and release checks before push.

Required parity: all ten detection settings, six distinct resampling filters, no upscaling,
EXIF orientation, multipage TIFF and animated image frames, supported file extensions,
recursive discovery and output exclusion, per-page failure details, per-file progress,
UTF-8 BOM CSV, JSON diagnostics and settings, blue/red previews, GUI initialization from
CLI, responsive background processing, result-folder opening and close confirmation.

Fixtures are generated only from synthetic test data, not user scans. merge_golden.json
contains 120 successful calls to the original merge implementation (including deterministic
randomized chains). images_golden.json records the original classifications and angles for
11 PNG fixtures. Rust image detection must preserve classifications and angle accuracy
within the original tests' tolerances; independent detector implementations need not emit
identical raw segments. Exact merge contracts are tested against the golden data.
