"""Skew-angle estimation from long straight lines. Positive angles are clockwise."""
from __future__ import annotations

import math

import cv2
import numpy as np
from PIL import Image


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
