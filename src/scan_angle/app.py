"""Local scan skew estimator. Positive image-coordinate angles are clockwise."""
from __future__ import annotations

import csv
import json
import math
import os
from pathlib import Path
import queue
import threading
import traceback

import cv2
import numpy as np
from PIL import Image, ImageDraw, ImageOps

EXTENSIONS = {'.tif', '.tiff', '.png', '.jpg', '.jpeg', '.bmp', '.webp'}
FIELDS = ['文件', '页码', '偏差角度_度_顺时针为正', '转正角度_度_顺时针为正',
          '状态', '一致线段数', '角度离散_度', '说明', '预览']


def weighted_median(values, weights):
    order = np.argsort(values)
    v, w = np.asarray(values)[order], np.asarray(weights)[order]
    return float(v[np.searchsorted(np.cumsum(w), np.sum(w) / 2)])


def estimate(image, max_angle=10.0, analysis_size=2400):
    """Return estimate plus evidence lines on an EXIF-normalized thumbnail."""
    small = image.convert('RGB')
    small.thumbnail((analysis_size, analysis_size), Image.Resampling.LANCZOS)
    gray = cv2.cvtColor(np.asarray(small), cv2.COLOR_RGB2GRAY)
    h, w = gray.shape
    found = cv2.createLineSegmentDetector(cv2.LSD_REFINE_STD).detect(gray)[0]
    candidates = []
    if found is not None:
        for coords in found.reshape(-1, 4):
            x1, y1, x2, y2 = map(float, coords)
            length = math.hypot(x2-x1, y2-y1)
            angle = (math.degrees(math.atan2(y2-y1, x2-x1))+45) % 90-45
            if length >= max(45, min(h, w)*0.18) and abs(angle) <= max_angle:
                # Ignore the image frame itself.
                if (max(x1,x2) < 4 or min(x1,x2) > w-5 or
                        max(y1,y2) < 4 or min(y1,y2) > h-5):
                    continue
                candidates.append(dict(coords=[x1,y1,x2,y2], length=length, angle=angle))
    if not candidates:
        return dict(angle=None, correction=None, status='需人工确认', count=0,
                    spread=None, reason='没有足够的长直线；圆形、无边框或低对比图片可能无法判断'), small, []
    # Choose the strongest angular consensus; long edges carry more weight.
    angles = np.array([x['angle'] for x in candidates])
    weights = np.array([x['length'] for x in candidates])
    support = np.array([weights[np.abs(angles-a) <= .35].sum() for a in angles])
    center = angles[int(np.argmax(support))]
    keep = np.abs(angles-center) <= .35
    center = weighted_median(angles[keep], weights[keep])
    keep = np.abs(angles-center) <= .35
    selected = [x for x, ok in zip(candidates, keep) if ok]
    angle = weighted_median(angles[keep], weights[keep])
    spread = float(np.sqrt(np.average((angles[keep]-angle)**2, weights=weights[keep])))
    # Distinguish separate physical lines from the two sides of a single edge.
    groups = []
    for line in sorted(selected, key=lambda x: -x['length']):
        x1,y1,x2,y2 = line['coords']
        horizontal = abs(x2-x1) > abs(y2-y1)
        intercept = ((y1+y2)/2-math.tan(math.radians(angle))*(x1+x2)/2
                     if horizontal else (x1+x2)/2+math.tan(math.radians(angle))*(y1+y2)/2)
        if all(horizontal != kind or abs(intercept-pos) > min(h,w)*.025 for kind,pos in groups):
            groups.append((horizontal, intercept))
    ratio = float(weights[keep].sum()/weights.sum())
    enough = len(groups) >= 2 and weights[keep].sum() >= min(h,w)*.7
    conflict = ratio < .65
    status = '可信' if enough and not conflict and spread < .18 else '需人工确认'
    reason = ('多条分离直线方向一致' if status == '可信' else
              '直线证据不足或方向存在冲突；请核对预览')
    if abs(angle) > max_angle-.3:
        status, reason = '需人工确认', '角度接近检测范围上限；建议扩大范围复核'
    return dict(angle=round(angle,3), correction=round(-angle,3), status=status,
                count=len(selected), spread=round(spread,3), reason=reason), small, selected


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


def gui():
    import tkinter as tk
    from tkinter import ttk, filedialog, messagebox
    root = tk.Tk()
    root.title('扫描偏角检测 · 本地批处理')
    root.geometry('940x630')
    root.minsize(800,540)
    root.option_add('*Font', ('Microsoft YaHei UI', 10))
    frame = ttk.Frame(root,padding=18)
    frame.pack(fill='both',expand=True)
    ttk.Label(frame,text='扫描偏角检测',font=('Microsoft YaHei UI',19,'bold')).pack(anchor='w')
    ttk.Label(frame,text='按纸边或长直线估算偏角。正数＝顺时针；负数＝逆时针。原图不会修改。').pack(anchor='w',pady=(4,12))
    source, dest = tk.StringVar(), tk.StringVar()
    def choose_source():
        chosen=filedialog.askdirectory(title='选择扫描图片文件夹')
        if chosen:
            source.set(chosen)
            dest.set(str(Path(chosen).parent/(Path(chosen).name+'_角度结果')))
    for title,var,action in [('输入文件夹',source,choose_source),('输出文件夹',dest,lambda: choose_dest())]:
        row=ttk.Frame(frame);row.pack(fill='x',pady=4)
        ttk.Label(row,text=title,width=11).pack(side='left')
        ttk.Entry(row,textvariable=var).pack(side='left',fill='x',expand=True,padx=6)
        ttk.Button(row,text='浏览…',command=action).pack(side='left')
    def choose_dest():
        chosen=filedialog.askdirectory(title='选择结果保存文件夹')
        if chosen: dest.set(chosen)
    options=ttk.Frame(frame);options.pack(fill='x',pady=8)
    recursive=tk.BooleanVar(value=False)
    ttk.Checkbutton(options,text='包含子文件夹',variable=recursive).pack(side='left')
    ttk.Label(options,text='    最大偏角 ±').pack(side='left')
    angle=tk.StringVar(value='10')
    ttk.Spinbox(options,from_=1,to=30,width=5,textvariable=angle).pack(side='left')
    ttk.Label(options,text='°（不判断 90° / 180° 文字朝向）').pack(side='left')
    tree=ttk.Treeview(frame,columns=('file','page','angle','state'),show='headings',height=11)
    for key,title,width in [('file','文件',360),('page','页',50),('angle','偏差角度（°）',140),('state','状态',170)]:
        tree.heading(key,text=title);tree.column(key,width=width)
    tree.pack(fill='both',expand=True,pady=8)
    status=tk.StringVar(value='选择图片文件夹后开始。结果包含 CSV 和红色标线预览。')
    ttk.Label(frame,textvariable=status,wraplength=870).pack(anchor='w',pady=5)
    bar=ttk.Progressbar(frame,mode='indeterminate');bar.pack(fill='x',pady=5)
    messages=queue.Queue()
    buttons=ttk.Frame(frame);buttons.pack(fill='x',pady=6)
    running=[False]
    def start():
        if running[0]: return
        try:
            limit=float(angle.get())
            if not 1 <= limit <= 30: raise ValueError()
            if not source.get() or not dest.get(): raise ValueError()
        except ValueError:
            messagebox.showerror('输入有误','请选择输入、输出文件夹，最大偏角填写 1～30。');return
        args=(source.get(),dest.get(),recursive.get(),limit)
        running[0]=True;start_button.configure(state='disabled');bar.start()
        tree.delete(*tree.get_children())
        def worker():
            try: messages.put(('done',run_batch(*args,progress=lambda s: messages.put(('log',s)))))
            except Exception: messages.put(('error',traceback.format_exc()))
        threading.Thread(target=worker,daemon=True).start()
    start_button=ttk.Button(buttons,text='开始检测',command=start);start_button.pack(side='left')
    def open_result():
        if Path(dest.get()).is_dir(): os.startfile(dest.get())
    ttk.Button(buttons,text='打开结果文件夹',command=open_result).pack(side='left',padx=10)
    def poll():
        try:
            while True:
                kind,data=messages.get_nowait()
                if kind=='log': status.set(data)
                else:
                    running[0]=False;bar.stop();start_button.configure(state='normal')
                    if kind=='error': status.set('检测失败');messagebox.showerror('检测失败',data)
                    else:
                        for row in data:
                            a=row[FIELDS[2]]
                            tree.insert('', 'end',values=(row[FIELDS[0]],row[FIELDS[1]],'—' if a is None else f'{a:+.3f}',row[FIELDS[4]]))
                        good=sum(r[FIELDS[4]]=='可信' for r in data)
                        status.set(f'完成：{len(data)} 页；可信 {good} 页；其余请核对。已保存 angles.csv 和预览图。')
        except queue.Empty: pass
        root.after(100,poll)
    def close():
        if running[0] and not messagebox.askyesno('正在检测','退出会中断检测，当前批次可能没有完整 CSV。确定退出？'):return
        root.destroy()
    root.protocol('WM_DELETE_WINDOW',close)
    poll();root.mainloop()
