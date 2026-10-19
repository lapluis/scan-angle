"""Compatibility facade for the historical scan_angle.app module."""
from .batch import EXTENSIONS, FIELDS, run_batch
from .detection import (RESAMPLE_ALGORITHMS, DetectionSettings,
                        effective_merge_tolerances, effective_min_line_pixels,
                        estimate, merge_collinear_segments, weighted_median)
from .gui import gui

__all__ = ['DetectionSettings', 'EXTENSIONS', 'FIELDS', 'RESAMPLE_ALGORITHMS',
           'effective_merge_tolerances', 'effective_min_line_pixels', 'estimate',
           'gui', 'merge_collinear_segments', 'run_batch', 'weighted_median']
