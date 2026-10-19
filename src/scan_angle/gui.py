"""Tkinter window for interactive batch runs."""
from __future__ import annotations

import os
from pathlib import Path
import queue
import threading
import traceback

from .batch import FIELDS, run_batch
from .detection import (MAX_ANALYSIS_SIZE, MIN_ANALYSIS_SIZE, RESAMPLE_ALGORITHMS,
                        DetectionSettings)


def _number_text(value):
    return f'{value:g}' if isinstance(value, float) else str(value)


def gui(settings=None, *, recursive=False, output=None):
    import tkinter as tk
    from tkinter import ttk, filedialog, messagebox
    settings = settings or DetectionSettings()
    root = tk.Tk()
    root.title('扫图偏角检测')
    root.geometry('980x700')
    root.minsize(880,660)
    root.option_add('*Font', ('Microsoft YaHei UI', 10))
    frame = ttk.Frame(root,padding=16)
    frame.pack(fill='both',expand=True)
    ttk.Label(frame,text='正数＝顺时针；负数＝逆时针').pack(anchor='w',pady=(4,10))
    source, dest = tk.StringVar(), tk.StringVar(value=str(output) if output else '')
    def choose_source():
        chosen=filedialog.askdirectory(title='选择扫描图片文件夹')
        if chosen:
            source.set(chosen)
            dest.set(str(Path(chosen).parent/(Path(chosen).name+'_角度结果')))
    for title,var,action in [('输入文件夹',source,choose_source),('输出文件夹',dest,lambda: choose_dest())]:
        row=ttk.Frame(frame);row.pack(fill='x',pady=3)
        ttk.Label(row,text=title,width=11).pack(side='left')
        ttk.Entry(row,textvariable=var).pack(side='left',fill='x',expand=True,padx=6)
        ttk.Button(row,text='浏览…',command=action).pack(side='left')
    def choose_dest():
        chosen=filedialog.askdirectory(title='选择结果保存文件夹')
        if chosen: dest.set(chosen)

    options=ttk.Frame(frame);options.pack(fill='x',pady=(7,2))
    recursive_var=tk.BooleanVar(value=bool(recursive))
    ttk.Checkbutton(options,text='包含子文件夹',variable=recursive_var).pack(side='left')
    angle_options=ttk.Frame(frame);angle_options.pack(fill='x',pady=3)
    ttk.Label(angle_options,text='最大偏角 ±').pack(side='left')
    angle=tk.StringVar(value=_number_text(settings.max_angle))
    ttk.Spinbox(angle_options,from_=1,to=30,width=5,textvariable=angle).pack(side='left')
    ttk.Label(angle_options,text='°').pack(side='left')

    options2=ttk.Frame(frame);options2.pack(fill='x',pady=3)
    ttk.Label(options2,text='最短直线 ≥ 分析图短边').pack(side='left')
    percent=tk.StringVar(value=_number_text(settings.min_line_percent))
    ttk.Entry(options2,width=6,textvariable=percent).pack(side='left',padx=(5,2))
    ttk.Label(options2,text='% 且 ≥').pack(side='left')
    pixels=tk.StringVar(value=_number_text(settings.min_line_pixels))
    ttk.Entry(options2,width=7,textvariable=pixels).pack(side='left',padx=(5,2))
    ttk.Label(options2,text='分析像素').pack(side='left')

    options3=ttk.Frame(frame);options3.pack(fill='x',pady=3)
    ttk.Label(options3,text='分析图最长边').pack(side='left')
    analysis_size=tk.StringVar(value=_number_text(settings.analysis_size))
    ttk.Spinbox(options3,from_=MIN_ANALYSIS_SIZE,to=MAX_ANALYSIS_SIZE,
                increment=100,width=8,textvariable=analysis_size).pack(side='left',padx=(5,2))
    ttk.Label(options3,text='像素（仅缩小）    缩放算法').pack(side='left')
    resample=tk.StringVar(value=settings.resample)
    ttk.Combobox(options3,values=tuple(RESAMPLE_ALGORITHMS),state='readonly',
                 width=10,textvariable=resample).pack(side='left',padx=(5,0))

    tree=ttk.Treeview(frame,columns=('file','page','angle','merged','state'),show='headings',height=9)
    for key,title,width in [('file','文件',320),('page','页',45),('angle','偏差角度（°）',120),
                            ('merged','直线数',115),('state','状态',150)]:
        tree.heading(key,text=title);tree.column(key,width=width)
    tree.pack(fill='both',expand=True,pady=7)
    status=tk.StringVar(value='')
    ttk.Label(frame,textvariable=status,wraplength=930).pack(anchor='w',pady=4)
    bar=ttk.Progressbar(frame,mode='indeterminate');bar.pack(fill='x',pady=4)
    messages=queue.Queue()
    buttons=ttk.Frame(frame);buttons.pack(fill='x',pady=5)
    running=[False]

    def read_number(variable, label):
        text=variable.get().strip()
        try:
            return float(text)
        except ValueError:
            raise ValueError(f'{label}必须是数值') from None

    def read_size(variable):
        text=variable.get().strip()
        try:
            return int(text)
        except ValueError:
            raise ValueError('分析图最长边必须是整数') from None

    def start():
        if running[0]: return
        try:
            config=DetectionSettings(
                max_angle=read_number(angle,'最大偏角'),
                min_line_percent=read_number(percent,'最短直线百分比'),
                min_line_pixels=read_number(pixels,'最短直线像素'),
                analysis_size=read_size(analysis_size),
                resample=resample.get(),
            )
            if not 1 <= config.max_angle <= 30:
                raise ValueError('最大偏角必须在 1～30 之间')
            source_value, dest_value = source.get().strip(), dest.get().strip()
            if not source_value or not dest_value:
                raise ValueError('请选择输入和输出文件夹')
        except (ValueError, TypeError) as exc:
            messagebox.showerror('输入有误', str(exc) or '请检查设置。');return
        # Snapshot every Tk variable on the main thread; the worker thread
        # below must never read Tk state.
        snapshot=(source_value, dest_value, recursive_var.get(), config)
        running[0]=True;start_button.configure(state='disabled');bar.start()
        tree.delete(*tree.get_children())
        def worker():
            try:
                messages.put(('done',run_batch(
                    snapshot[0],snapshot[1],snapshot[2],progress=lambda s: messages.put(('log',s)),
                    settings=snapshot[3])))
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
                            detected=row.get('fragment_count')
                            merged=row.get('merged_count')
                            detected_text='—' if detected is None else str(detected)
                            merged_text='—' if merged is None else str(merged)
                            tree.insert('', 'end',values=(row[FIELDS[0]],row[FIELDS[1]],
                                                          '—' if a is None else f'{a:+.3f}',
                                                          f'{detected_text} / {merged_text}',
                                                          row[FIELDS[4]]))
                        good=sum(r[FIELDS[4]]=='可信' for r in data)
                        merged_total=sum(int(r.get('merged_count') or 0) for r in data)
                        status.set(f'完成：{len(data)} 页；可信 {good} 页；合并直线共 {merged_total} 条；其余请核对。已保存 angles.csv 和预览图。')
        except queue.Empty: pass
        root.after(100,poll)
    def close():
        if running[0] and not messagebox.askyesno('正在检测','退出会中断检测，当前批次可能没有完整 CSV。确定退出？'):return
        root.destroy()
    root.protocol('WM_DELETE_WINDOW',close)
    poll();root.mainloop()
