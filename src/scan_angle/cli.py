"""Command-line parsing and dispatch to batch mode or the GUI."""
from __future__ import annotations

import argparse
from pathlib import Path

from PIL import Image

from .batch import run_batch
from .detection import (MAX_ANALYSIS_SIZE, MERGE_ANGLE_TOLERANCE_DEG,
                        MERGE_DISTANCE_TOLERANCE_MIN_PX,
                        MERGE_DISTANCE_TOLERANCE_PERCENT,
                        MERGE_GAP_TOLERANCE_MIN_PX,
                        MERGE_GAP_TOLERANCE_PERCENT, MIN_ANALYSIS_SIZE,
                        RESAMPLE_ALGORITHMS, DetectionSettings)
from .gui import gui


_SETTING_OPTIONS = {
    'max_angle': '--max-angle',
    'min_line_percent': '--min-line-percent',
    'min_line_pixels': '--min-line-pixels',
    'analysis_size': '--analysis-size',
    'resample': '--resample',
    'merge_angle': '--merge-angle',
    'merge_distance_percent': '--merge-distance-percent',
    'merge_distance_pixels': '--merge-distance-pixels',
    'merge_gap_percent': '--merge-gap-percent',
    'merge_gap_pixels': '--merge-gap-pixels',
}


def _option_message(message):
    """Translate validation field names into the matching command-line flags."""
    for name, option in _SETTING_OPTIONS.items():
        message = message.replace(name, option)
    return message


def main(argv=None):
    names = '/'.join(RESAMPLE_ALGORITHMS)
    parser=argparse.ArgumentParser(description='扫描偏角检测：正数表示顺时针，输入图不修改。')
    parser.add_argument('folder',nargs='?')
    parser.add_argument('--output','-o')
    parser.add_argument('--recursive','-r',action='store_true')
    parser.add_argument('--max-angle',type=float,default=15)
    parser.add_argument('--min-line-percent',type=float,default=10.0,
                        help='最短直线占对应方向分析图边长的百分比：水平线用宽度、竖直线用高度，默认 10，范围 (0, 100]')
    parser.add_argument('--min-line-pixels',type=float,default=50.0,
                        help='最短直线的绝对长度（分析图像素），默认 50，必须大于 0')
    parser.add_argument('--analysis-size',type=int,default=2400,
                        help=f'分析图最长边像素，默认 2400，范围 {MIN_ANALYSIS_SIZE}～{MAX_ANALYSIS_SIZE}；不放大原图')
    parser.add_argument('--resample',default='lanczos',
                        help=f'分析图缩放算法：{names}，默认 lanczos')
    parser.add_argument('--merge-angle',type=float,default=MERGE_ANGLE_TOLERANCE_DEG,
                        help='共线合并允许的最大方向角差（度），默认 0.75，必须大于 0 且小于 45')
    parser.add_argument('--merge-distance-percent',type=float,
                        default=MERGE_DISTANCE_TOLERANCE_PERCENT,
                        help='垂直距离容差占分析图短边的百分比，默认 0.12（即 0.12%%，不是比例），必须大于等于 0')
    parser.add_argument('--merge-distance-pixels',type=float,
                        default=MERGE_DISTANCE_TOLERANCE_MIN_PX,
                        help='垂直距离容差的绝对下限（分析图像素），默认 1.5，必须大于等于 0')
    parser.add_argument('--merge-gap-percent',type=float,
                        default=MERGE_GAP_TOLERANCE_PERCENT,
                        help='投影间隙容差占分析图短边的百分比，默认 0.5（即 0.5%%，不是比例），必须大于等于 0')
    parser.add_argument('--merge-gap-pixels',type=float,
                        default=MERGE_GAP_TOLERANCE_MIN_PX,
                        help='投影间隙容差的绝对下限（分析图像素），默认 6，必须大于等于 0')
    parser.add_argument('--allow-large-images',action='store_true',help='允许处理超过 Pillow 默认像素保护上限的可信本地扫描图')
    args=parser.parse_args(argv)
    if args.allow_large_images: Image.MAX_IMAGE_PIXELS=None
    try:
        settings=DetectionSettings(
            max_angle=args.max_angle,
            min_line_percent=args.min_line_percent,
            min_line_pixels=args.min_line_pixels,
            analysis_size=args.analysis_size,
            resample=args.resample,
            merge_angle=args.merge_angle,
            merge_distance_percent=args.merge_distance_percent,
            merge_distance_pixels=args.merge_distance_pixels,
            merge_gap_percent=args.merge_gap_percent,
            merge_gap_pixels=args.merge_gap_pixels,
        )
    except ValueError as exc:
        parser.error(_option_message(str(exc)))
    if not 1<=settings.max_angle<=30:
        parser.error('--max-angle 必须在 1～30 之间')
    if args.folder:
        output=args.output or str(Path(args.folder).resolve().parent/(Path(args.folder).name+'_角度结果'))
        run_batch(args.folder,output,args.recursive,progress=print,settings=settings)
    else:
        # Settings supplied on the command line must initialize the GUI
        # controls instead of being silently discarded.
        gui(settings=settings, recursive=args.recursive, output=args.output)
