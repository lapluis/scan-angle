# Rust migration

Baseline: a0538d6; Python unittest suite: 58 passed (2026-10-20).

Stages:
1. Capture synthetic regression fixtures from Python and preserve behavior contracts.
2. Port validated settings, stable-anchor merging, evidence filtering and confidence.
3. Implement image decoding/resampling, batch CSV/JSON/previews, CLI and native GUI.
4. Keep Python intact; add independent Rust build/docs and CI checks before pushing the Rust branch.

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

## Scope update (2026-10-20)

Per the user's instruction, Rust work lives on codex/rust-port and coexists with Python.
Local main remains at a0538d6. No Python source, entry point, uv configuration, test,
or build script is removed. Python and Rust have separate build scripts and executable
names. CI runs both suites. Only the new Rust branch is to be pushed after validation.


## Validation

- Python baseline suite: 58/58 passed, unchanged Python source/config/build script.
- Rust: 22 unit tests and 7 integration tests passed; fmt and clippy -D warnings passed.
- Golden coverage: 120 merge cases, 24 pipeline cases, 11 image angle/classification
  cases, 30 format/EXIF fixtures (42 pages), 12 six-filter pixel comparisons.
- Synthetic detector classifications match Python; maximum measured angle difference
  0.037 degrees (original acceptance tolerance 0.12 degrees).
- Six-filter samples including integer pre-reduction differ by at most 1 per channel.
- Release EXE: --help and 42-page batch passed with no failed pages; input SHA-256
  hashes remained unchanged. Rust executable is separate from the Python package.
- Windows GUI logic is unit tested. Native visual smoke testing was not completed:
  the Computer Use app-launch approval timed out before a window opened.
- Windows WIC supplies TIFF fax/palette/other layout fallback. Those formats are not
  promised on other platforms; Windows is the supported packaged target.
