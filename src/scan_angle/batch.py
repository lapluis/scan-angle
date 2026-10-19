"""Batch processing of scanned images and result-file writing."""
from __future__ import annotations

import csv
import json
from pathlib import Path

from PIL import Image, ImageDraw, ImageOps

from .detection import estimate

EXTENSIONS = {'.tif', '.tiff', '.png', '.jpg', '.jpeg', '.bmp', '.webp'}
FIELDS = ['文件', '页码', '偏差角度_度_顺时针为正', '转正角度_度_顺时针为正',
          '状态', '一致线段数', '角度离散_度', '说明', '预览']


def run_batch(source, destination, recursive=False, max_angle=10, progress=lambda s: None):
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
                        result, small, lines = estimate(oriented, max_angle)
                        draw = ImageDraw.Draw(small)
                        for line in lines:
                            draw.line(tuple(line['coords']), fill=(255,30,40), width=3)
                        preview_name = f'{index:05d}_{path.stem}_p{page+1}.jpg'
                        small.save(previews/preview_name, quality=90)
                        row = [str(relative),page+1,result['angle'],result['correction'],result['status'],
                               result['count'],result['spread'],result['reason'],f'previews/{preview_name}']
                        rows.append(dict(zip(FIELDS,row)))
                        detail.append(dict(file=str(relative),page=page+1,**result,lines=lines))
                        progress(f"  第 {page+1} 页: {result['angle']}° · {result['status']}")
                    except Exception as exc:
                        rows.append(dict(zip(FIELDS,[str(relative),page+1,None,None,'读取失败',0,None,str(exc),''])))
        except Exception as exc:
            rows.append(dict(zip(FIELDS,[str(relative),None,None,None,'读取失败',0,None,str(exc),''])))
    with (destination/'angles.csv').open('w',encoding='utf-8-sig',newline='') as stream:
        writer = csv.DictWriter(stream, fieldnames=FIELDS)
        writer.writeheader()
        writer.writerows(rows)
    (destination/'details.json').write_text(json.dumps(detail,ensure_ascii=False,indent=2),encoding='utf-8')
    return rows
