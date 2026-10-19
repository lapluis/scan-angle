import tempfile
import unittest
from pathlib import Path

from PIL import Image, ImageDraw

from scan_angles import FIELDS, estimate, run_batch


def rectangle():
    image = Image.new('RGB', (1400, 1700), '#dfeefb')
    ImageDraw.Draw(image).rectangle((140, 170, 1250, 1510), fill='white', outline='#666666', width=3)
    return image


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


class BatchTests(unittest.TestCase):
    def test_multipage_bad_file_and_output_exclusion(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory)
            image = rectangle()
            image.save(source/'双页.tiff', save_all=True, append_images=[image.rotate(-1, fillcolor='white')])
            (source/'损坏.png').write_bytes(b'invalid')
            destination = source/'results'
            rows = run_batch(source, destination, recursive=True)
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


if __name__ == '__main__':
    unittest.main()
