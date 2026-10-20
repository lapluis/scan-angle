# Rust migration

Baseline: a0538d6; Python unittest suite: 58 passed (2026-10-20).

Stages:
1. Capture synthetic regression fixtures from Python and preserve behavior contracts.
2. Port validated settings, stable-anchor merging, evidence filtering and confidence.
3. Implement image decoding/resampling, batch CSV/JSON/previews, CLI and native GUI.
4. Initially retain Python alongside Rust; subsequently remove Python from the Rust branch at the user's request, preserving main.

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

The latest instruction supersedes the earlier side-by-side plan. codex/rust-port now
contains only the Rust application, Rust tests and build/CI configuration. Removed
tracked Python sources, tests, main.py, pyproject.toml, uv.lock, .python-version and
PyInstaller build script. Synthetic golden fixtures remain as Rust regression data.
Local main and remote main are at a0538d6; this work does not modify or push main.
Ignored local Python environments/build artifacts are not deleted.

## Validation

- Historical Python baseline suite: 58/58 passed before removing Python from this branch.
- Rust: 24 unit tests and 7 integration tests passed; fmt and clippy -D warnings passed.
- Golden coverage: 120 merge cases, 24 pipeline cases, 11 image angle/classification
  cases, 30 format/EXIF fixtures (42 pages), 12 six-filter pixel comparisons.
- Synthetic detector classifications match Python; maximum measured angle difference
  0.037 degrees (original acceptance tolerance 0.12 degrees).
- Six-filter samples including integer pre-reduction differ by at most 1 per channel.
- Release EXE: --help and 42-page batch passed with no failed pages; input SHA-256
  hashes remained unchanged. Rust executable is separate from the Python package.
- Native Windows GUI exercised through Computer Use: empty-input error, editable long
  paths, Chinese input/output paths, native folder selection and automatic output path,
  keyboard navigation, 30 format files / 42 pages, progress, results table and opening
  the results folder. CSV/JSON/previews verified on disk.
- A 301-file GUI batch (300 synthetic +1.75-degree scans and one corrupt PNG) produced
  300 trusted rows and one failure row, without aborting valid images. Running controls
  are disabled; closing prompts before interruption. Completion clears pending close
  confirmation (also regression tested for success and failure).
- GUI testing found and fixed narrow path fields caused by an egui Grid layout.
- CC (local Claude Code via DeepSeek) reviewed GUI/batch code. Fixed misleading errors
  for output ancestors and stale close confirmation. Kept the documented legacy CSV
  selected-merged-line count and explicit fail-fast directory discovery policy.
  Follow-up CC review confirmed both fixes with no remaining concrete regressions.
  Its minor stale-error-window observation was addressed by clearing the previous
  error when a validated batch starts.
- Windows WIC supplies TIFF fax/palette/other layout fallback. Those formats are not
  promised on other platforms; Windows is the supported packaged target.
