"""Batch processing of scanned images and result-file writing."""
from __future__ import annotations

import csv
import json
from pathlib import Path

from PIL import Image, ImageDraw, ImageOps

from .detection import DetectionSettings, estimate

EXTENSIONS = {'.tif', '.tiff', '.png', '.jpg', '.jpeg', '.bmp', '.webp'}
FIELDS = ['文件', '页码', '偏差角度_度_顺时针为正', '转正角度_度_顺时针为正',
          '状态', '合并直线数', '角度离散_度', '说明', '预览']


def _failure_detail(relative, page, error, settings):
    return dict(file=str(relative), page=page, angle=None, correction=None,
                status='读取失败', count=0, spread=None, reason=str(error),
                fragment_count=0, merged_count=0, threshold_px=None,
                merge=None, lines=[], settings=settings.as_dict(),
                error=str(error))


def run_batch(source, destination, recursive=False, max_angle=15,
              progress=lambda s: None, *, min_line_percent=10.0,
              min_line_pixels=50.0, analysis_size=2400, resample='lanczos',
              settings=None):
    """Process every supported image and write CSV, JSON and previews.

    ``settings``, when given, replaces the individual keyword settings.  All
    settings are validated before any output directory is created.  The row
    count field is the number of *merged* straight lines, not the number of
    raw detector fragments. Successful result rows additionally include
    ``fragment_count`` and ``merged_count`` for the GUI; CSV columns stay
    restricted to ``FIELDS``.
    """
    if settings is None:
        settings = DetectionSettings(
            max_angle=max_angle,
            min_line_percent=min_line_percent,
            min_line_pixels=min_line_pixels,
            analysis_size=analysis_size,
            resample=resample,
        )
    source, destination = Path(source).resolve(), Path(destination).resolve()
    if not source.is_dir():
        raise ValueError('输入文件夹不存在')
    if source == destination:
        raise ValueError('输出文件夹必须与输入文件夹不同')
    destination.mkdir(parents=True, exist_ok=True)
    previews = destination/'previews'
    previews.mkdir(exist_ok=True)
    files = sorted(p for p in (source.rglob('*') if recursive else source.iterdir())
                   if p.is_file() and p.suffix.lower() in EXTENSIONS
                   and not p.is_relative_to(destination))
    if not files:
        raise ValueError('文件夹中没有支持的图片')
    rows, detail = [], []
    for index, path in enumerate(files, 1):
        relative = path.relative_to(source)
        progress(f'[{index}/{len(files)}] {relative}')
        try:
            with Image.open(path) as original:
                page_count = getattr(original, 'n_frames', 1)
                for page in range(page_count):
                    try:
                        original.seek(page)
                        oriented = ImageOps.exif_transpose(original)
                        result, small, lines = estimate(oriented, settings=settings)
                        draw = ImageDraw.Draw(small)
                        for line in lines:
                            draw.line(tuple(line['coords']), fill=(255,30,40), width=3)
                        preview_name = f'{index:05d}_{path.stem}_p{page+1}.jpg'
                        small.save(previews/preview_name, quality=90)
                        row = [str(relative),page+1,result['angle'],result['correction'],result['status'],
                               result['count'],result['spread'],result['reason'],f'previews/{preview_name}']
                        rows.append(dict(zip(FIELDS,row),
                                         fragment_count=result['fragment_count'],
                                         merged_count=result['merged_count']))
                        detail.append(dict(file=str(relative),page=page+1,**result,
                                           settings=settings.as_dict(),lines=lines))
                        progress(f"  第 {page+1} 页: {result['angle']}° · {result['status']}")
                    except Exception as exc:
                        rows.append(dict(zip(FIELDS,[str(relative),page+1,None,None,'读取失败',0,None,str(exc),''])))
                        detail.append(_failure_detail(relative, page+1, exc, settings))
        except Exception as exc:
            rows.append(dict(zip(FIELDS,[str(relative),None,None,None,'读取失败',0,None,str(exc),''])))
            detail.append(_failure_detail(relative, None, exc, settings))
    with (destination/'angles.csv').open('w',encoding='utf-8-sig',newline='') as stream:
        writer = csv.DictWriter(stream, fieldnames=FIELDS)
        writer.writeheader()
        writer.writerows({field: row[field] for field in FIELDS} for row in rows)
    (destination/'details.json').write_text(json.dumps(detail,ensure_ascii=False,indent=2),encoding='utf-8')
    return rows
