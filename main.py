"""Project entry point: uv run main.py [folder] [options]."""
import argparse
from pathlib import Path

from PIL import Image

from scan_angle.app import gui, run_batch


def main():
    parser=argparse.ArgumentParser(description='扫描偏角检测：正数表示顺时针，输入图不修改。')
    parser.add_argument('folder',nargs='?')
    parser.add_argument('--output','-o')
    parser.add_argument('--recursive','-r',action='store_true')
    parser.add_argument('--max-angle',type=float,default=10)
    parser.add_argument('--allow-large-images',action='store_true',help='允许处理超过 Pillow 默认像素保护上限的可信本地扫描图')
    args=parser.parse_args()
    if args.allow_large_images: Image.MAX_IMAGE_PIXELS=None
    if args.folder:
        if not 1<=args.max_angle<=30: parser.error('--max-angle 必须在 1～30 之间')
        output=args.output or str(Path(args.folder).resolve().parent/(Path(args.folder).name+'_角度结果'))
        run_batch(args.folder,output,args.recursive,args.max_angle,progress=print)
    else:
        gui()


if __name__=='__main__':
    main()
