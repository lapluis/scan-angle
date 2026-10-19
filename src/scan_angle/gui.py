"""Tkinter window for interactive batch runs."""
from __future__ import annotations

import os
from pathlib import Path
import queue
import threading
import traceback

from .batch import FIELDS, run_batch


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
