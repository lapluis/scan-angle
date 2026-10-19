import json
import math
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import numpy as np
from PIL import Image, ImageDraw

from scan_angle.batch import FIELDS, run_batch
from scan_angle.detection import RESAMPLE_ALGORITHMS, DetectionSettings, estimate


class _FakeLineSegmentDetector:
    def __init__(self, segments):
        self._segments = np.asarray(segments, dtype=np.float32).reshape(-1, 4)

    def detect(self, _gray):
        return self._segments.reshape(-1, 1, 4), None, None


def estimate_with_segments(image, segments, **kwargs):
    """Run the real thumbnail/merge/filter pipeline with fixed detector output."""
    detector = _FakeLineSegmentDetector(segments)
    with mock.patch('scan_angle.detection.cv2.createLineSegmentDetector',
                    return_value=detector):
        return estimate(image, **kwargs)


def _has_dominant_channel(image, center, channel, margin=20):
    """Return whether a neighbourhood contains a pixel dominated by a channel.

    The margin stays modest because previews are JPEG-compressed and the
    one-pixel blue merged layer can be partly desaturated by chroma
    subsampling.
    """
    cx, cy = center
    for x in range(cx - 3, cx + 4):
        for y in range(cy - 3, cy + 4):
            pixel = image.getpixel((x, y))
            others = [pixel[index] for index in range(3) if index != channel]
            if pixel[channel] > 150 and pixel[channel] - max(others) > margin:
                return True
    return False


def rectangle():
    image = Image.new('RGB', (1400, 1700), '#dfeefb')
    ImageDraw.Draw(image).rectangle((140, 170, 1250, 1510), fill='white', outline='#666666', width=3)
    return image


def _draw_dashed_line(draw, start, end, dash, gap, width):
    x1, y1 = start
    x2, y2 = end
    length = math.hypot(x2 - x1, y2 - y1)
    ux, uy = (x2 - x1) / length, (y2 - y1) / length
    offset = 0.0
    while offset < length:
        segment_end = min(offset + dash, length)
        draw.line([(x1 + ux * offset, y1 + uy * offset),
                   (x1 + ux * segment_end, y1 + uy * segment_end)],
                  fill='black', width=width)
        offset = segment_end + gap


def fragmented_rotated_border(angle_deg, canvas_size=2000, crop_size=1600,
                              box=(400, 400, 1600, 1600), dash=100, gap=5,
                              width=3):
    """White image with a clockwise-rotated rectangular border of dashes.

    Every dash is 100 px long, well below the detector's minimum line length,
    and the 5 px gaps are inside the documented default merge tolerance of
    8 px for a 1600 px analysis image.
    """
    canvas = Image.new('RGB', (canvas_size, canvas_size), 'white')
    draw = ImageDraw.Draw(canvas)
    left, top, right, bottom = box
    edges = [((left, top), (right, top)),
             ((right, top), (right, bottom)),
             ((right, bottom), (left, bottom)),
             ((left, bottom), (left, top))]
    for start, end in edges:
        _draw_dashed_line(draw, start, end, dash, gap, width)
    rotated = canvas.rotate(-angle_deg, resample=Image.Resampling.BICUBIC,
                            fillcolor='white')
    offset = (canvas_size - crop_size) // 2
    return rotated.crop((offset, offset, offset + crop_size, offset + crop_size))


class EstimateTests(unittest.TestCase):
    def test_known_clockwise_angles(self):
        for angle in [-7, -2, -.25, 0, .3, 1.75, 6]:
            with self.subTest(angle=angle):
                image = rectangle().rotate(-angle, resample=Image.Resampling.BICUBIC, fillcolor='#dfeefb')
                result, _, _ = estimate(image)
                self.assertIsNotNone(result['angle'])
                self.assertAlmostEqual(result['angle'], angle, delta=.12)
                self.assertAlmostEqual(result['correction'], -angle, delta=.12)
                self.assertEqual(result['status'], '可信')

    def test_blank_and_circle_do_not_invent_angles(self):
        for circle in [False, True]:
            with self.subTest(circle=circle):
                image = Image.new('RGB', (1200, 1200), '#dfeefb')
                if circle:
                    ImageDraw.Draw(image).ellipse((150, 150, 1050, 1050), fill='gold', outline='black', width=5)
                result, _, _ = estimate(image)
                self.assertIsNone(result['angle'])
                self.assertEqual(result['status'], '需人工确认')

    def test_all_resample_choices_and_no_upscaling(self):
        image = Image.new('RGB', (800, 600), '#dfeefb')
        for name in RESAMPLE_ALGORITHMS:
            with self.subTest(resample=name):
                _, small, _ = estimate(image, analysis_size=400, resample=name)
                self.assertEqual(small.size, (400, 300))
        # Small images are never enlarged to the analysis-size target.
        _, small, _ = estimate(Image.new('RGB', (320, 200), '#dfeefb'),
                               analysis_size=2400, resample='lanczos')
        self.assertEqual(small.size, (320, 200))

    def test_threshold_effects_on_the_same_image(self):
        image = rectangle()
        detected, _, lines = estimate(image)
        self.assertIsNotNone(detected['angle'])
        self.assertEqual(detected['count'], len(lines))
        self.assertEqual(set(detected['thresholds_px']),
                         {'horizontal', 'vertical'})
        self.assertTrue(all(value > 0
                            for value in detected['thresholds_px'].values()))

        too_short, _, lines = estimate(image, min_line_pixels=5000)
        self.assertIsNone(too_short['angle'])
        self.assertEqual(too_short['count'], 0)
        self.assertEqual(lines, [])
        self.assertGreater(too_short['merged_count'], 0)
        self.assertEqual(len(too_short['merged_lines']), too_short['merged_count'])

        # 99.9% of the analysis width (1398.6 px) and height (1698.3 px) is
        # longer than the corresponding rectangle edges that survive merging.
        percent_too_high, _, _ = estimate(image, min_line_percent=99.9,
                                          min_line_pixels=1)
        self.assertIsNone(percent_too_high['angle'])

    def test_fragmented_border_recovers_known_small_rotation(self):
        angle = 1.6
        image = fragmented_rotated_border(angle)
        result, _, lines = estimate(image)
        self.assertIsNotNone(result['angle'])
        self.assertAlmostEqual(result['angle'], angle, delta=.2)
        self.assertAlmostEqual(result['correction'], -angle, delta=.2)
        # 5 px dash gaps are within the 8 px documented tolerance for this
        # image size, but each 100 px dash is below the 160 px threshold.
        self.assertGreater(result['merge']['gap_tolerance_px'], 5)
        self.assertGreater(result['thresholds_px']['horizontal'], 100)
        self.assertGreater(result['thresholds_px']['vertical'], 100)
        self.assertGreater(result['merge']['fragments'],
                           4 * result['merge']['selected'])
        self.assertTrue(any(line['member_count'] >= 3 for line in lines))
        for line in lines:
            if line['member_count'] >= 2:
                self.assertGreater(line['support_length'],
                                   result['thresholds_px'][line['orientation']])

    def test_dashed_line_fragments_become_eligible_after_merge(self):
        image = Image.new('RGB', (1600, 1600), 'white')
        draw = ImageDraw.Draw(image)
        for start in range(100, 100 + 6 * 86, 86):
            draw.rectangle((start, 798, start + 80, 801), fill='black')
        result, _, lines = estimate(image)
        self.assertIsNotNone(result['angle'])
        self.assertEqual(result['count'], len(lines))
        self.assertGreater(result['count'], 0)
        # The 80 px dashes are below the 10% / 50 px threshold on their own,
        # but their merged support length is not.
        self.assertLess(80, result['thresholds_px']['horizontal'])
        self.assertGreater(result['merge']['merged'], 0)
        self.assertLess(result['merge']['merged'], result['merge']['fragments'])
        self.assertTrue(any(line['member_count'] >= 2 for line in lines))
        for line in lines:
            if line['member_count'] >= 2:
                self.assertGreater(line['support_length'],
                                   result['thresholds_px'][line['orientation']])


class OrientationThresholdTests(unittest.TestCase):
    """End-to-end threshold tests with deterministic detector output.

    The detector is stubbed so each merged line's support length is known
    exactly.  The thumbnail, merging, filtering and angle-consensus pipeline
    underneath remains the real implementation, so these tests exercise
    estimate() rather than only the threshold helper.
    """

    @staticmethod
    def _white(width, height):
        return Image.new('RGB', (width, height), 'white')

    def test_wide_image_rejects_horizontal_and_keeps_vertical(self):
        # Analysis image 4000 x 1000 (analysis_size 4000 keeps it unchanged):
        # horizontal threshold = 10% of width = 400, vertical = 10% of
        # height = 100.  Both merged lines have about 150 px support, so only
        # the vertical line may pass.  Endpoints are reversed and skewed.
        segments = [
            (2200, 500, 2050, 502),   # horizontal, reversed, 0.76 degrees
            (1200, 700, 1202, 550),   # vertical, reversed, 0.76 degrees
        ]
        result, small, lines = estimate_with_segments(
            self._white(4000, 1000), segments,
            settings=DetectionSettings(analysis_size=4000))
        self.assertEqual(small.size, (4000, 1000))
        self.assertEqual(result['thresholds_px'],
                         {'horizontal': 400.0, 'vertical': 100.0})
        self.assertEqual(result['merged_count'], 2)
        self.assertEqual(result['merge']['merged'], 2)
        self.assertEqual(result['merge']['selected'], 1)
        self.assertEqual(result['count'], 1)
        self.assertEqual([line['orientation'] for line in lines], ['vertical'])
        self.assertAlmostEqual(lines[0]['support_length'], math.hypot(2, 150),
                               delta=.01)

    def test_tall_image_rejects_vertical_and_keeps_horizontal(self):
        # Mirror image: 1000 x 4000 gives horizontal threshold 100 and
        # vertical threshold 400, so the 150 px horizontal line alone passes.
        segments = [
            (700, 2000, 550, 2002),   # horizontal, reversed, 0.76 degrees
            (500, 3000, 502, 2850),   # vertical, reversed, 0.76 degrees
        ]
        result, small, lines = estimate_with_segments(
            self._white(1000, 4000), segments,
            settings=DetectionSettings(analysis_size=4000))
        self.assertEqual(small.size, (1000, 4000))
        self.assertEqual(result['thresholds_px'],
                         {'horizontal': 100.0, 'vertical': 400.0})
        self.assertEqual(result['merged_count'], 2)
        self.assertEqual(result['merge']['selected'], 1)
        self.assertEqual(result['count'], 1)
        self.assertEqual([line['orientation'] for line in lines],
                         ['horizontal'])

    def test_merged_skewed_fragments_use_each_orientation_threshold(self):
        # Two short, reversed, slightly skewed fragments per direction.  The
        # horizontal pair merges to about 150 px (< 400) and the vertical pair
        # to about 150 px (>= 100).  Filtering must happen after merging and
        # use the merged line's own orientation.
        segments = [
            (2125, 500.0, 2050, 500.25),   # horizontal fragment, reversed
            (2205, 501.0, 2130, 500.6),    # horizontal fragment, reversed
            (1200.5, 625, 1200, 550),      # vertical fragment, reversed
            (1201, 705, 1200.5, 630),      # vertical fragment, reversed
        ]
        result, _, lines = estimate_with_segments(
            self._white(4000, 1000), segments,
            settings=DetectionSettings(analysis_size=4000))
        self.assertEqual(result['fragment_count'], 4)
        self.assertEqual(result['merged_count'], 2)
        self.assertEqual(result['merge']['merged'], 2)
        self.assertEqual(result['merge']['selected'], 1)
        self.assertEqual(result['count'], 1)
        self.assertEqual([line['orientation'] for line in lines], ['vertical'])
        by_orientation = {line['orientation']: line
                          for line in result['merged_lines']}
        self.assertGreater(by_orientation['horizontal']['member_count'], 1)
        self.assertGreater(by_orientation['vertical']['member_count'], 1)
        self.assertLess(by_orientation['horizontal']['support_length'], 400)
        self.assertGreaterEqual(by_orientation['vertical']['support_length'], 100)

    def test_actual_resized_dimensions_set_the_boundaries(self):
        # The 4000 x 1000 source shrinks to 2400 x 600 at the default
        # analysis_size.  Thresholds come from those actual dimensions:
        # horizontal 10% of 2400 = 240; vertical 10% of 600 = 60.  They are
        # neither the original-image values (400/100) nor 10% of the
        # requested 2400 x 2400 (240/240).
        image = self._white(4000, 1000)
        exact = [
            (1040, 300, 800, 300),       # horizontal support exactly 240
            (1600, 330, 1600, 270),      # vertical support exactly 60
        ]
        result, small, lines = estimate_with_segments(image, exact)
        self.assertEqual(small.size, (2400, 600))
        self.assertEqual(result['thresholds_px'],
                         {'horizontal': 240.0, 'vertical': 60.0})
        self.assertEqual(result['count'], 2)
        self.assertEqual({line['orientation'] for line in lines},
                         {'horizontal', 'vertical'})

        below = [
            (1039.9, 300, 800, 300),       # horizontal 239.9 < 240
            (1600, 329.95, 1600, 270.05),  # vertical 59.9 < 60
        ]
        result, small, lines = estimate_with_segments(image, below)
        self.assertEqual(small.size, (2400, 600))
        self.assertEqual(result['thresholds_px'],
                         {'horizontal': 240.0, 'vertical': 60.0})
        self.assertEqual(result['merged_count'], 2)
        self.assertEqual(result['merge']['selected'], 0)
        self.assertEqual(result['count'], 0)
        self.assertIsNone(result['angle'])
        self.assertEqual(lines, [])

    def test_50px_floor_applies_to_both_orientations(self):
        # A 400 x 200 source is not upscaled, so the percentage thresholds
        # are only 40 and 20 px and both directions fall back to the 50 px
        # absolute floor.  Exactly 50 px passes; 49.9 px does not.
        image = self._white(400, 200)
        exact = [
            (170, 100, 120, 100),     # horizontal support exactly 50
            (300, 125, 300, 75),      # vertical support exactly 50
        ]
        result, small, lines = estimate_with_segments(image, exact)
        self.assertEqual(small.size, (400, 200))
        self.assertEqual(result['thresholds_px'],
                         {'horizontal': 50.0, 'vertical': 50.0})
        self.assertEqual(result['count'], 2)
        self.assertEqual({line['orientation'] for line in lines},
                         {'horizontal', 'vertical'})

        below = [
            (169.9, 100, 120, 100),     # horizontal 49.9 < 50
            (300, 124.95, 300, 75.05),  # vertical 49.9 < 50
        ]
        result, small, lines = estimate_with_segments(image, below)
        self.assertEqual(small.size, (400, 200))
        self.assertEqual(result['thresholds_px'],
                         {'horizontal': 50.0, 'vertical': 50.0})
        self.assertEqual(result['merge']['selected'], 0)
        self.assertEqual(result['count'], 0)
        self.assertIsNone(result['angle'])
        self.assertEqual(lines, [])


class MergeSettingPipelineTests(unittest.TestCase):
    """Merge settings resolved against the actual resized analysis image."""

    @staticmethod
    def _white(width, height):
        return Image.new('RGB', (width, height), 'white')

    def test_diagnostics_record_effective_tolerances_on_resized_dimensions(self):
        # 4000x1000 shrinks to 2400x600 at the default analysis size, so the
        # short side is 600 for every tolerance calculation.
        explicit = DetectionSettings(
            analysis_size=2400, merge_angle=1.25,
            merge_distance_percent=0.5, merge_distance_pixels=1.5,
            merge_gap_percent=2.0, merge_gap_pixels=6.0)
        result, small, _ = estimate_with_segments(
            self._white(4000, 1000), [], settings=explicit)
        self.assertEqual(small.size, (2400, 600))
        self.assertEqual(result['merge']['angle_tolerance_deg'], 1.25)
        self.assertEqual(result['merge']['perpendicular_tolerance_px'], 3.0)
        self.assertEqual(result['merge']['gap_tolerance_px'], 12.0)

        # The defaults fall back to their absolute pixel floors here:
        # 0.12% of 600 = 0.72 px and 0.5% of 600 = 3 px.
        result, small, _ = estimate_with_segments(self._white(4000, 1000), [])
        self.assertEqual(small.size, (2400, 600))
        self.assertEqual(result['merge']['angle_tolerance_deg'], 0.75)
        self.assertEqual(result['merge']['perpendicular_tolerance_px'], 1.5)
        self.assertEqual(result['merge']['gap_tolerance_px'], 6.0)

    def test_diagnostics_keep_tiny_positive_tolerances(self):
        # A positive pixel floor far below 0.001 px is used as-is by the
        # merge, so diagnostics must report it instead of rounding it to zero.
        settings = DetectionSettings(
            merge_distance_percent=0, merge_distance_pixels=0.0001,
            merge_gap_percent=0, merge_gap_pixels=0.0001)
        result, _, _ = estimate_with_segments(
            self._white(400, 200), [], settings=settings)
        self.assertGreater(result['merge']['perpendicular_tolerance_px'], 0)
        self.assertGreater(result['merge']['gap_tolerance_px'], 0)
        self.assertEqual(result['merge']['perpendicular_tolerance_px'], 0.0001)
        self.assertEqual(result['merge']['gap_tolerance_px'], 0.0001)

    def test_angle_setting_changes_merged_count(self):
        image = self._white(2000, 1000)
        # The first fragment is deliberately the longest so it seeds the
        # anchor group.  The second is 1.43 degrees away, with both endpoints
        # 10 px from the anchor and a 10 px projected gap.  Distance and gap
        # tolerances are wide enough that only the angle decides grouping.
        segments = [(100, 500, 1100, 500), (1110, 490, 1910, 510)]
        common = dict(merge_distance_percent=0, merge_distance_pixels=20,
                      merge_gap_percent=0, merge_gap_pixels=100)
        narrow, _, _ = estimate_with_segments(
            image, segments,
            settings=DetectionSettings(merge_angle=0.75, **common))
        wide, _, _ = estimate_with_segments(
            image, segments,
            settings=DetectionSettings(merge_angle=1.5, **common))
        self.assertEqual(narrow['merged_count'], 2)
        self.assertEqual(wide['merged_count'], 1)

    def test_distance_percent_and_pixel_floor_change_merging(self):
        # Actual analysis image is 2400x600, short side 600.  The fragments
        # are parallel with a 2 px perpendicular offset and a 10 px gap.
        image = self._white(4000, 1000)
        segments = [(200, 300, 1000, 300), (1010, 302, 1800, 302)]
        common = dict(merge_gap_percent=0, merge_gap_pixels=100)
        floor, _, _ = estimate_with_segments(
            image, segments,
            settings=DetectionSettings(merge_distance_percent=0.12,
                                       merge_distance_pixels=1.5, **common))
        percent, _, _ = estimate_with_segments(
            image, segments,
            settings=DetectionSettings(merge_distance_percent=0.5,
                                       merge_distance_pixels=1.5, **common))
        pixels, _, _ = estimate_with_segments(
            image, segments,
            settings=DetectionSettings(merge_distance_percent=0,
                                       merge_distance_pixels=2.5, **common))
        # 0.12% of 600 = 0.72 px, so the 1.5 px floor cannot bridge 2 px.
        self.assertEqual(floor['merged_count'], 2)
        # 0.5% of 600 = 3 px, and a 2.5 px floor are both wide enough.
        self.assertEqual(percent['merged_count'], 1)
        self.assertEqual(pixels['merged_count'], 1)

    def test_gap_percent_and_pixel_floor_change_merging(self):
        # Actual analysis image is 2400x600, short side 600.  The exactly
        # collinear fragments are separated by a 5 px projected gap.
        image = self._white(4000, 1000)
        segments = [(200, 300, 1000, 300), (1005, 300, 1800, 300)]
        common = dict(merge_distance_percent=0, merge_distance_pixels=0)
        floor, _, _ = estimate_with_segments(
            image, segments,
            settings=DetectionSettings(merge_gap_percent=0.5,
                                       merge_gap_pixels=6, **common))
        percent, _, _ = estimate_with_segments(
            image, segments,
            settings=DetectionSettings(merge_gap_percent=2,
                                       merge_gap_pixels=0, **common))
        below, _, _ = estimate_with_segments(
            image, segments,
            settings=DetectionSettings(merge_gap_percent=0.5,
                                       merge_gap_pixels=0, **common))
        # 0.5% of 600 = 3 px cannot bridge a 5 px gap; both the 6 px floor
        # and 2% (12 px) can.
        self.assertEqual(below['merged_count'], 2)
        self.assertEqual(floor['merged_count'], 1)
        self.assertEqual(percent['merged_count'], 1)

    def test_zero_distance_and_gap_semantics_in_estimate_pipeline(self):
        image = self._white(2000, 1000)
        zero = dict(merge_angle=0.75, merge_distance_percent=0,
                    merge_distance_pixels=0, merge_gap_percent=0,
                    merge_gap_pixels=0)
        touching = [(200, 300, 1000, 300), (1000, 300, 1800, 300)]
        overlapping = [(200, 300, 1000, 300), (900, 300, 1800, 300)]
        separated = [(200, 300, 1000, 300), (1001, 300, 1800, 300)]
        offset = [(200, 300, 1000, 300), (1000, 300.5, 1800, 300.5)]
        expected = [(touching, 1), (overlapping, 1),
                    (separated, 2), (offset, 2)]
        for segments, merged_count in expected:
            with self.subTest(segments=segments):
                result, _, _ = estimate_with_segments(
                    image, segments, settings=DetectionSettings(**zero))
                self.assertEqual(result['merged_count'], merged_count)

    def test_defaults_preserve_existing_merge_results(self):
        image = self._white(2000, 1000)
        segments = [(200, 500, 1000, 500), (1005, 501, 1800, 501),
                    (200, 200, 200, 800)]
        default, _, _ = estimate_with_segments(image, segments)
        explicit, _, _ = estimate_with_segments(
            image, segments, merge_angle=0.75, merge_distance_percent=0.12,
            merge_distance_pixels=1.5, merge_gap_percent=0.5,
            merge_gap_pixels=6.0)
        self.assertEqual(default['angle'], explicit['angle'])
        self.assertEqual(default['merge'], explicit['merge'])
        self.assertEqual(default['merged_lines'], explicit['merged_lines'])


class BatchTests(unittest.TestCase):
    def test_multipage_bad_file_and_output_exclusion(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory)
            image = rectangle()
            image.save(source/'双页.tiff', save_all=True, append_images=[image.rotate(-1, fillcolor='white')])
            (source/'损坏.png').write_bytes(b'invalid')
            destination = source/'results'
            file_counts = []
            rows = run_batch(source, destination, recursive=True,
                             file_progress=lambda completed, total: file_counts.append((completed, total)))
            # A two-page TIFF is one file; the corrupt image also completes
            # one file. Progress must reach 2/2 even though there are 3 rows.
            self.assertEqual(file_counts, [(0, 2), (1, 2), (2, 2)])
            self.assertEqual(len(rows), 3)
            self.assertEqual(sum(row[FIELDS[4]] == '读取失败' for row in rows), 1)
            # Previous previews must not become source images on the next run.
            again = run_batch(source, destination, recursive=True)
            self.assertEqual(len(again), 3)
            self.assertTrue((destination/'angles.csv').read_bytes().startswith(b'\xef\xbb\xbf'))
            self.assertTrue((destination/'details.json').exists())

    def test_exif_orientation_is_normalized(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory)/'input'
            source.mkdir()
            image = rectangle()
            exif = image.getexif()
            exif[274] = 6  # Display orientation: rotate clockwise 90 degrees.
            image.save(source/'oriented.jpg', exif=exif)
            destination = Path(directory)/'output'
            rows = run_batch(source, destination)
            self.assertEqual(len(rows), 1)
            self.assertAlmostEqual(rows[0][FIELDS[2]], 0, delta=.12)
            with Image.open(destination/rows[0][FIELDS[8]]) as preview:
                self.assertGreater(preview.width, preview.height)

    def test_input_cannot_be_output(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(ValueError):
                run_batch(directory, directory)

    def test_batch_writes_settings_merge_diagnostics_and_merged_count(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory)/'input'
            source.mkdir()
            rectangle().save(source/'page.png')
            destination = Path(directory)/'output'
            settings = DetectionSettings(max_angle=8, min_line_percent=20,
                                         min_line_pixels=50, analysis_size=1200,
                                         resample='bilinear')
            rows = run_batch(source, destination, settings=settings)
            self.assertEqual(len(rows), 1)
            details = json.loads((destination/'details.json').read_text(encoding='utf-8'))
            self.assertEqual(len(details), 1)
            entry = details[0]
            self.assertEqual(entry['settings'], settings.as_dict())
            self.assertIn('merge', entry)
            self.assertGreater(entry['count'], 0)
            self.assertGreaterEqual(entry['fragment_count'], entry['merged_count'])
            self.assertGreaterEqual(entry['merged_count'], entry['count'])
            self.assertEqual(entry['count'], len(entry['lines']))
            self.assertEqual(entry['merged_count'], len(entry['merged_lines']))
            self.assertEqual(set(entry['thresholds_px']),
                             {'horizontal', 'vertical'})
            self.assertTrue(all(value > 0
                                for value in entry['thresholds_px'].values()))
            self.assertTrue(all(line in entry['merged_lines'] for line in entry['lines']))
            self.assertEqual(rows[0][FIELDS[5]], entry['count'])
            self.assertEqual(rows[0]['fragment_count'], entry['fragment_count'])
            self.assertEqual(rows[0]['merged_count'], entry['merged_count'])
            for line in entry['lines']:
                self.assertGreaterEqual(line['member_count'], 1)
                self.assertLessEqual(line['support_length'], line['span_length'])
            header = (destination/'angles.csv').read_text(encoding='utf-8-sig').splitlines()[0]
            self.assertIn('合并直线数', header)

    def test_batch_orientation_filter_and_preview_layers_stay_consistent(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory)/'input'
            source.mkdir()
            Image.new('RGB', (2000, 1000), 'white').save(source/'wide.png')
            destination = Path(directory)/'output'
            # 2000 x 1000 is below the default 2400 longest side, so it is
            # not resized: horizontal threshold = 200, vertical = 100.
            # The horizontal line is only ~150 px and must stay blue
            # (merged but not selected); the vertical line passes and is red.
            segments = [
                (1800, 500, 1650, 502),   # horizontal, reversed, ~150 px
                (1200, 700, 1202, 550),   # vertical, reversed, ~150 px
            ]
            detector = _FakeLineSegmentDetector(segments)
            with mock.patch('scan_angle.detection.cv2.createLineSegmentDetector',
                            return_value=detector):
                rows = run_batch(source, destination)
            self.assertEqual(len(rows), 1)
            entry = json.loads((destination/'details.json').read_text(
                encoding='utf-8'))[0]
            self.assertEqual(entry['thresholds_px'],
                             {'horizontal': 200.0, 'vertical': 100.0})
            self.assertEqual(entry['merged_count'], 2)
            self.assertEqual(entry['merged_count'], len(entry['merged_lines']))
            self.assertEqual(entry['merge']['selected'], 1)
            self.assertEqual(entry['count'], 1)
            self.assertEqual(entry['count'], len(entry['lines']))
            self.assertEqual(rows[0][FIELDS[5]], 1)
            self.assertEqual([line['orientation'] for line in entry['lines']],
                             ['vertical'])
            with Image.open(destination/rows[0][FIELDS[8]]) as preview:
                preview = preview.convert('RGB')
                self.assertEqual(preview.size, (2000, 1000))
                # Selected vertical line: red over the blue merged layer.
                self.assertTrue(_has_dominant_channel(preview, (1201, 625), 0))
                # Rejected horizontal line: blue merged layer remains.
                self.assertTrue(_has_dominant_channel(preview, (1725, 501), 2))

    def test_batch_keyword_settings_are_validated_and_recorded(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory)/'input'
            source.mkdir()
            rectangle().save(source/'page.png')
            destination = Path(directory)/'output'
            rows = run_batch(source, destination, max_angle=9,
                             min_line_percent=22, min_line_pixels=60,
                             analysis_size=2000, resample='hamming',
                             merge_angle=1.1, merge_distance_percent=0.3,
                             merge_distance_pixels=2.0,
                             merge_gap_percent=0.8, merge_gap_pixels=4.0)
            self.assertEqual(len(rows), 1)
            details = json.loads((destination/'details.json').read_text(encoding='utf-8'))
            self.assertEqual(details[0]['settings'], {
                'max_angle': 9.0,
                'min_line_percent': 22.0,
                'min_line_pixels': 60.0,
                'analysis_size': 2000,
                'resample': 'hamming',
                'merge_angle': 1.1,
                'merge_distance_percent': 0.3,
                'merge_distance_pixels': 2.0,
                'merge_gap_percent': 0.8,
                'merge_gap_pixels': 4.0,
            })
            self.assertGreater(details[0]['count'], 0)
            # rectangle() is 1400x1700 and is not resized at analysis_size
            # 2000, so the short side is 1400 px: distance max(2.0, 4.2) =
            # 4.2 and gap max(4.0, 11.2) = 11.2.
            self.assertEqual(details[0]['merge']['angle_tolerance_deg'], 1.1)
            self.assertEqual(details[0]['merge']['perpendicular_tolerance_px'], 4.2)
            self.assertEqual(details[0]['merge']['gap_tolerance_px'], 11.2)

    def test_batch_validates_settings_before_creating_output(self):
        invalid = [
            dict(min_line_percent=float('nan')),
            dict(merge_angle=0),
            dict(merge_angle=45),
            dict(merge_distance_percent=-1),
            dict(merge_distance_pixels=float('nan')),
            dict(merge_gap_percent=float('inf')),
            dict(merge_gap_pixels=-0.1),
        ]
        for kwargs in invalid:
            with self.subTest(kwargs=kwargs), tempfile.TemporaryDirectory() as directory:
                source = Path(directory)/'input'
                source.mkdir()
                (source/'bad.png').write_bytes(b'invalid')
                destination = Path(directory)/'output'
                with self.assertRaises(ValueError):
                    run_batch(source, destination, **kwargs)
                self.assertFalse(destination.exists())

    def test_batch_failure_details_keep_settings(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory)/'input'
            source.mkdir()
            (source/'bad.png').write_bytes(b'invalid')
            destination = Path(directory)/'output'
            settings = DetectionSettings(analysis_size=800, resample='box',
                                         merge_angle=1.3,
                                         merge_distance_percent=0.4,
                                         merge_distance_pixels=2.5,
                                         merge_gap_percent=1.25,
                                         merge_gap_pixels=7.5)
            rows = run_batch(source, destination, settings=settings)
            self.assertEqual(len(rows), 1)
            self.assertEqual(rows[0][FIELDS[4]], '读取失败')
            details = json.loads((destination/'details.json').read_text(encoding='utf-8'))
            self.assertEqual(len(details), 1)
            self.assertEqual(details[0]['settings'], settings.as_dict())
            self.assertEqual(details[0]['status'], '读取失败')
            self.assertIsNone(details[0]['thresholds_px'])
            self.assertIn('error', details[0])


if __name__ == '__main__':
    unittest.main()
