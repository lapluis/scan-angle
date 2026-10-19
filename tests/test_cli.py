import contextlib
import io
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from PIL import Image

from scan_angle import cli
from scan_angle.detection import DetectionSettings


class CliTests(unittest.TestCase):
    def test_no_folder_opens_gui_with_default_settings(self):
        with mock.patch.object(cli, 'gui') as gui, \
                mock.patch.object(cli, 'run_batch') as run_batch:
            cli.main([])
        gui.assert_called_once()
        self.assertEqual(gui.call_args.kwargs['settings'], DetectionSettings())
        self.assertFalse(gui.call_args.kwargs['recursive'])
        self.assertIsNone(gui.call_args.kwargs['output'])
        run_batch.assert_not_called()

    def test_no_folder_initializes_gui_with_cli_settings(self):
        with mock.patch.object(cli, 'gui') as gui, \
                mock.patch.object(cli, 'run_batch') as run_batch:
            cli.main(['--max-angle', '7.5', '--min-line-percent', '25',
                      '--min-line-pixels', '80', '--analysis-size', '1200',
                      '--resample', 'BICUBIC',
                      '--merge-angle', '1.2',
                      '--merge-distance-percent', '0.3',
                      '--merge-distance-pixels', '2.5',
                      '--merge-gap-percent', '0.8',
                      '--merge-gap-pixels', '4.5',
                      '--recursive', '--output', 'results'])
        gui.assert_called_once()
        self.assertTrue(gui.call_args.kwargs['recursive'])
        self.assertEqual(gui.call_args.kwargs['output'], 'results')
        settings = gui.call_args.kwargs['settings']
        self.assertEqual(settings.as_dict(), {
            'max_angle': 7.5,
            'min_line_percent': 25.0,
            'min_line_pixels': 80.0,
            'analysis_size': 1200,
            'resample': 'bicubic',
            'merge_angle': 1.2,
            'merge_distance_percent': 0.3,
            'merge_distance_pixels': 2.5,
            'merge_gap_percent': 0.8,
            'merge_gap_pixels': 4.5,
        })
        run_batch.assert_not_called()

    def test_folder_runs_batch_with_default_output_and_settings(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory)/'input'
            source.mkdir()
            with mock.patch.object(cli, 'gui') as gui, \
                    mock.patch.object(cli, 'run_batch') as run_batch:
                cli.main([str(source)])
            gui.assert_not_called()
            output = str(source.resolve().parent/(source.name+'_角度结果'))
            args, kwargs = run_batch.call_args
            self.assertEqual(args, (str(source), output, False))
            self.assertIs(kwargs['progress'], print)
            self.assertEqual(kwargs['settings'], DetectionSettings())

    def test_folder_batch_forwards_settings(self):
        with tempfile.TemporaryDirectory() as directory:
            source = str(Path(directory)/'input')
            with mock.patch.object(cli, 'gui') as gui, \
                    mock.patch.object(cli, 'run_batch') as run_batch:
                cli.main([source, '--output', 'results', '--recursive',
                          '--max-angle', '7.5', '--min-line-percent', '30',
                          '--min-line-pixels', '100', '--analysis-size', '800',
                          '--resample', 'nearest',
                          '--merge-angle', '1.1',
                          '--merge-distance-percent', '0.4',
                          '--merge-distance-pixels', '3',
                          '--merge-gap-percent', '1.5',
                          '--merge-gap-pixels', '8'])
            gui.assert_not_called()
            args, kwargs = run_batch.call_args
            self.assertEqual(args, (source, 'results', True))
            self.assertIs(kwargs['progress'], print)
            self.assertEqual(kwargs['settings'].as_dict(), {
                'max_angle': 7.5,
                'min_line_percent': 30.0,
                'min_line_pixels': 100.0,
                'analysis_size': 800,
                'resample': 'nearest',
                'merge_angle': 1.1,
                'merge_distance_percent': 0.4,
                'merge_distance_pixels': 3.0,
                'merge_gap_percent': 1.5,
                'merge_gap_pixels': 8.0,
            })

    def test_invalid_max_angle_is_rejected(self):
        for limit in ['0', '31', 'nan', 'inf']:
            with self.subTest(limit=limit), \
                    contextlib.redirect_stderr(io.StringIO()) as stderr, \
                    mock.patch.object(cli, 'gui') as gui, \
                    mock.patch.object(cli, 'run_batch') as run_batch:
                with self.assertRaises(SystemExit) as raised:
                    cli.main(['some-folder', '--max-angle', limit])
                self.assertEqual(raised.exception.code, 2)
                error_line = stderr.getvalue().splitlines()[-1]
                self.assertIn('--max-angle', error_line)
                self.assertNotIn('max_angle', error_line)
                gui.assert_not_called()
                run_batch.assert_not_called()

    def test_invalid_numeric_settings_are_rejected_before_dispatch(self):
        cases = [
            ('--min-line-percent', ['nan', 'inf', '0', '101', 'abc']),
            ('--min-line-pixels', ['nan', 'inf', '0', '-1', 'abc']),
            ('--analysis-size', ['255', '10001', '2400.5', 'abc']),
            ('--resample', ['cubic', '']),
            ('--merge-angle', ['0', '45', '45.1', '-1', 'nan', 'inf', 'abc']),
            ('--merge-distance-percent', ['-0.1', 'nan', 'inf', 'abc']),
            ('--merge-distance-pixels', ['-0.1', 'nan', 'inf', 'abc']),
            ('--merge-gap-percent', ['-0.1', 'nan', 'inf', 'abc']),
            ('--merge-gap-pixels', ['-0.1', 'nan', 'inf', 'abc']),
        ]
        for flag, values in cases:
            for value in values:
                with self.subTest(flag=flag, value=value), \
                        contextlib.redirect_stderr(io.StringIO()) as stderr, \
                        mock.patch.object(cli, 'gui') as gui, \
                        mock.patch.object(cli, 'run_batch') as run_batch:
                    with self.assertRaises(SystemExit) as raised:
                        cli.main([flag, value])
                    self.assertEqual(raised.exception.code, 2)
                    # The final error line (not the usage block, which always
                    # lists every flag) must name the hyphenated CLI option.
                    error_line = stderr.getvalue().splitlines()[-1]
                    field_name = flag.lstrip('-').replace('-', '_')
                    self.assertIn(flag, error_line)
                    if '_' in field_name:
                        # Multi-word settings must not leak their internal
                        # underscore spelling into user-facing errors.
                        self.assertNotIn(field_name, error_line)
                    gui.assert_not_called()
                    run_batch.assert_not_called()

    def test_zero_merge_distance_and_gap_options_are_valid(self):
        with mock.patch.object(cli, 'gui') as gui, \
                mock.patch.object(cli, 'run_batch') as run_batch:
            cli.main(['--merge-distance-percent', '0',
                      '--merge-distance-pixels', '0',
                      '--merge-gap-percent', '0',
                      '--merge-gap-pixels', '0'])
        gui.assert_called_once()
        settings = gui.call_args.kwargs['settings']
        self.assertEqual(settings.merge_distance_percent, 0.0)
        self.assertEqual(settings.merge_distance_pixels, 0.0)
        self.assertEqual(settings.merge_gap_percent, 0.0)
        self.assertEqual(settings.merge_gap_pixels, 0.0)
        run_batch.assert_not_called()

    def test_allow_large_images_disables_pillow_limit(self):
        original = Image.MAX_IMAGE_PIXELS
        try:
            with mock.patch.object(cli, 'gui') as gui, \
                    mock.patch.object(cli, 'run_batch') as run_batch:
                cli.main(['--allow-large-images'])
            self.assertIsNone(Image.MAX_IMAGE_PIXELS)
            gui.assert_called_once()
            self.assertEqual(gui.call_args.kwargs['settings'], DetectionSettings())
            run_batch.assert_not_called()
        finally:
            Image.MAX_IMAGE_PIXELS = original


if __name__ == '__main__':
    unittest.main()
