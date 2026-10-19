import json
import math
import tempfile
import unittest
from pathlib import Path

from PIL import Image, ImageDraw

from scan_angle.batch import FIELDS, run_batch
from scan_angle.detection import RESAMPLE_ALGORITHMS, DetectionSettings, estimate


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
        self.assertGreater(detected['threshold_px'], 0)

        too_short, _, lines = estimate(image, min_line_pixels=5000)
        self.assertIsNone(too_short['angle'])
        self.assertEqual(too_short['count'], 0)
        self.assertEqual(lines, [])

        # 99.9% of the 1400 px short side is longer than any rectangle edge
        # that survives merging on its own.
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
        self.assertGreater(result['threshold_px'], 100)
        self.assertGreater(result['merge']['fragments'],
                           4 * result['merge']['selected'])
        self.assertTrue(any(line['member_count'] >= 3 for line in lines))
        for line in lines:
            if line['member_count'] >= 2:
                self.assertGreater(line['support_length'],
                                   result['threshold_px'])

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
        self.assertLess(80, result['threshold_px'])
        self.assertGreater(result['merge']['merged'], 0)
        self.assertLess(result['merge']['merged'], result['merge']['fragments'])
        self.assertTrue(any(line['member_count'] >= 2 for line in lines))
        for line in lines:
            if line['member_count'] >= 2:
                self.assertGreater(line['support_length'], result['threshold_px'])


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
            self.assertEqual(rows[0][FIELDS[5]], entry['count'])
            self.assertEqual(rows[0]['fragment_count'], entry['fragment_count'])
            self.assertEqual(rows[0]['merged_count'], entry['merged_count'])
            for line in entry['lines']:
                self.assertGreaterEqual(line['member_count'], 1)
                self.assertLessEqual(line['support_length'], line['span_length'])
            header = (destination/'angles.csv').read_text(encoding='utf-8-sig').splitlines()[0]
            self.assertIn('合并直线数', header)

    def test_batch_keyword_settings_are_validated_and_recorded(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory)/'input'
            source.mkdir()
            rectangle().save(source/'page.png')
            destination = Path(directory)/'output'
            rows = run_batch(source, destination, max_angle=9,
                             min_line_percent=22, min_line_pixels=60,
                             analysis_size=1000, resample='hamming')
            self.assertEqual(len(rows), 1)
            details = json.loads((destination/'details.json').read_text(encoding='utf-8'))
            self.assertEqual(details[0]['settings'], {
                'max_angle': 9.0,
                'min_line_percent': 22.0,
                'min_line_pixels': 60.0,
                'analysis_size': 1000,
                'resample': 'hamming',
            })
            self.assertGreater(details[0]['count'], 0)

    def test_batch_validates_settings_before_creating_output(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory)/'input'
            source.mkdir()
            (source/'bad.png').write_bytes(b'invalid')
            destination = Path(directory)/'output'
            with self.assertRaises(ValueError):
                run_batch(source, destination, min_line_percent=float('nan'))
            self.assertFalse(destination.exists())

    def test_batch_failure_details_keep_settings(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory)/'input'
            source.mkdir()
            (source/'bad.png').write_bytes(b'invalid')
            destination = Path(directory)/'output'
            settings = DetectionSettings(analysis_size=800, resample='box')
            rows = run_batch(source, destination, settings=settings)
            self.assertEqual(len(rows), 1)
            self.assertEqual(rows[0][FIELDS[4]], '读取失败')
            details = json.loads((destination/'details.json').read_text(encoding='utf-8'))
            self.assertEqual(len(details), 1)
            self.assertEqual(details[0]['settings'], settings.as_dict())
            self.assertEqual(details[0]['status'], '读取失败')
            self.assertIn('error', details[0])


if __name__ == '__main__':
    unittest.main()
