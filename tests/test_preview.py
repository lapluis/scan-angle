import unittest

from PIL import Image

from scan_angle.batch import _draw_preview


class PreviewTests(unittest.TestCase):
    def test_all_merged_lines_remain_visible_without_selected_lines(self):
        original = Image.new('RGB', (100, 100), 'white')
        lines = [{'coords': [10, 20, 90, 20]}, {'coords': [10, 60, 90, 60]}]
        preview = _draw_preview(original, lines, [])
        self.assertEqual(preview.getpixel((50, 20)), (30, 120, 255))
        self.assertEqual(preview.getpixel((50, 60)), (30, 120, 255))
        self.assertEqual(original.getpixel((50, 20)), (255, 255, 255))

    def test_selected_lines_are_highlighted_over_merged_layer(self):
        image = Image.new('RGB', (100, 100), 'white')
        merged = [{'coords': [10, 20, 90, 20]}, {'coords': [10, 60, 90, 60]}]
        preview = _draw_preview(image, merged, [merged[0]])
        self.assertEqual(preview.getpixel((50, 20)), (255, 30, 40))
        self.assertEqual(preview.getpixel((50, 60)), (30, 120, 255))
        self.assertEqual(preview.getpixel((50, 40)), (255, 255, 255))


if __name__ == '__main__':
    unittest.main()
