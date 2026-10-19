import contextlib
import io
import tempfile
import unittest
from pathlib import Path
from unittest import mock

from PIL import Image

from scan_angle import cli


class CliTests(unittest.TestCase):
    def test_no_folder_opens_gui(self):
        with mock.patch.object(cli, 'gui') as gui, \
                mock.patch.object(cli, 'run_batch') as run_batch:
            cli.main([])
        gui.assert_called_once_with()
        run_batch.assert_not_called()

    def test_folder_runs_batch_with_default_output(self):
        with tempfile.TemporaryDirectory() as directory:
            source = Path(directory)/'input'
            source.mkdir()
            with mock.patch.object(cli, 'gui') as gui, \
                    mock.patch.object(cli, 'run_batch') as run_batch:
                cli.main([str(source)])
            gui.assert_not_called()
            output = str(source.resolve().parent/(source.name+'_角度结果'))
            run_batch.assert_called_once_with(str(source), output, False, 10, progress=print)

    def test_folder_batch_forwards_arguments(self):
        with tempfile.TemporaryDirectory() as directory:
            source = str(Path(directory)/'input')
            with mock.patch.object(cli, 'gui') as gui, \
                    mock.patch.object(cli, 'run_batch') as run_batch:
                cli.main([source, '--output', 'results', '--recursive',
                          '--max-angle', '7.5'])
            gui.assert_not_called()
            run_batch.assert_called_once_with(source, 'results', True, 7.5, progress=print)

    def test_invalid_max_angle_is_rejected(self):
        for limit in ['0', '31']:
            with self.subTest(limit=limit), \
                    contextlib.redirect_stderr(io.StringIO()) as stderr, \
                    mock.patch.object(cli, 'gui') as gui, \
                    mock.patch.object(cli, 'run_batch') as run_batch:
                with self.assertRaises(SystemExit) as raised:
                    cli.main(['some-folder', '--max-angle', limit])
                self.assertEqual(raised.exception.code, 2)
                self.assertIn('--max-angle', stderr.getvalue())
                gui.assert_not_called()
                run_batch.assert_not_called()

    def test_allow_large_images_disables_pillow_limit(self):
        original = Image.MAX_IMAGE_PIXELS
        try:
            with mock.patch.object(cli, 'gui') as gui, \
                    mock.patch.object(cli, 'run_batch') as run_batch:
                cli.main(['--allow-large-images'])
            self.assertIsNone(Image.MAX_IMAGE_PIXELS)
            gui.assert_called_once_with()
            run_batch.assert_not_called()
        finally:
            Image.MAX_IMAGE_PIXELS = original


if __name__ == '__main__':
    unittest.main()
