"""Skew-angle estimation from long straight lines. Positive angles are clockwise.

Merging policy
--------------
Line-segment detector output is first merged along each of the two axis
directions, *before* the minimum-length filter is applied.  Only fragments of
the same orientation (horizontal or vertical) are merged: a horizontal and a
vertical line can have the same normalized skew angle (0 degrees) but are
different physical directions.  The merge tolerances are deliberately
conservative and scale with the analysis image short side:

* angle difference: ``MERGE_ANGLE_TOLERANCE_DEG`` (0.75 degrees);
* perpendicular distance of a fragment's endpoints from the anchor line:
  ``max(1.5 px, 0.12% of the short side)``;
* gap between projected fragment intervals:
  ``max(6 px, 0.5% of the short side)``.

The longest remaining fragment seeds each group.  Every other fragment is
validated directly against that stable anchor line -- orientation, angle and
both endpoint distances -- rather than against a neighbour, so tolerance
cannot accumulate along a chain.  Compatible fragments are projected onto the
anchor direction and the interval-connected component containing the anchor
is taken as one group; the complete group is removed before the next longest
anchor is chosen.  This recovers arbitrarily long collinear chains regardless
of their relative fragment lengths while still preventing gradual drift.
Evidence weight is the union of the projected fragment intervals, so
overlapping fragments are not counted twice and the empty space between
fragments never inflates the length used for the minimum-length filter.
"""
from __future__ import annotations

import math
from dataclasses import dataclass

import cv2
import numpy as np
from PIL import Image


RESAMPLE_ALGORITHMS = {
    'lanczos': Image.Resampling.LANCZOS,
    'bicubic': Image.Resampling.BICUBIC,
    'bilinear': Image.Resampling.BILINEAR,
    'nearest': Image.Resampling.NEAREST,
    'box': Image.Resampling.BOX,
    'hamming': Image.Resampling.HAMMING,
}
MIN_ANALYSIS_SIZE = 256
MAX_ANALYSIS_SIZE = 10000
MERGE_ANGLE_TOLERANCE_DEG = 0.75
MERGE_PERPENDICULAR_TOLERANCE_RATIO = 0.0012
MERGE_PERPENDICULAR_TOLERANCE_MIN_PX = 1.5
MERGE_GAP_TOLERANCE_RATIO = 0.005
MERGE_GAP_TOLERANCE_MIN_PX = 6.0
_FRAME_MARGIN_PX = 4


def weighted_median(values, weights):
    order = np.argsort(values)
    v, w = np.asarray(values)[order], np.asarray(weights)[order]
    return float(v[np.searchsorted(np.cumsum(w), np.sum(w) / 2)])


def _finite_float(name, value):
    try:
        number = float(value)
    except (TypeError, ValueError):
        raise ValueError(f'{name} 必须是数值') from None
    if not math.isfinite(number):
        raise ValueError(f'{name} 必须是有限数值（不能为 NaN 或无穷大）')
    return number


@dataclass(frozen=True)
class DetectionSettings:
    """Validated detection settings shared by the detector, batch, CLI and GUI.

    ``min_line_percent`` is a percentage of the analysis image short side and
    ``min_line_pixels`` is in analysis-image pixels.  The effective threshold
    is the larger of the two (see :func:`effective_min_line_pixels`).
    """

    max_angle: float = 10.0
    min_line_percent: float = 18.0
    min_line_pixels: float = 45.0
    analysis_size: int = 2400
    resample: str = 'lanczos'

    def __post_init__(self):
        max_angle = _finite_float('max_angle', self.max_angle)
        if not 0 < max_angle <= 45:
            raise ValueError('max_angle 必须大于 0 且不超过 45')
        percent = _finite_float('min_line_percent', self.min_line_percent)
        if not 0 < percent <= 100:
            raise ValueError('min_line_percent 必须是 (0, 100] 范围内的有限数值')
        pixels = _finite_float('min_line_pixels', self.min_line_pixels)
        if pixels <= 0:
            raise ValueError('min_line_pixels 必须是大于 0 的有限数值')
        size = self.analysis_size
        if isinstance(size, bool) or not isinstance(size, int):
            raise ValueError(f'analysis_size 必须是 {MIN_ANALYSIS_SIZE}～{MAX_ANALYSIS_SIZE} 之间的整数')
        if not MIN_ANALYSIS_SIZE <= size <= MAX_ANALYSIS_SIZE:
            raise ValueError(f'analysis_size 必须在 {MIN_ANALYSIS_SIZE}～{MAX_ANALYSIS_SIZE} 之间')
        if not isinstance(self.resample, str):
            raise ValueError('resample 必须是算法名称')
        resample = self.resample.strip().lower()
        if resample not in RESAMPLE_ALGORITHMS:
            names = '/'.join(RESAMPLE_ALGORITHMS)
            raise ValueError(f'resample 必须是 {names} 之一')
        object.__setattr__(self, 'max_angle', max_angle)
        object.__setattr__(self, 'min_line_percent', percent)
        object.__setattr__(self, 'min_line_pixels', pixels)
        object.__setattr__(self, 'resample', resample)

    @property
    def resample_enum(self):
        return RESAMPLE_ALGORITHMS[self.resample]

    def as_dict(self):
        return {
            'max_angle': self.max_angle,
            'min_line_percent': self.min_line_percent,
            'min_line_pixels': self.min_line_pixels,
            'analysis_size': self.analysis_size,
            'resample': self.resample,
        }


def effective_min_line_pixels(settings, height, width):
    """Return max(percent of short side, absolute pixels) in analysis pixels."""
    short_side = min(int(height), int(width))
    return max(float(settings.min_line_pixels),
               float(settings.min_line_percent) / 100.0 * short_side)


def _merge_tolerances(height, width):
    short_side = min(int(height), int(width))
    perpendicular = max(MERGE_PERPENDICULAR_TOLERANCE_MIN_PX,
                        MERGE_PERPENDICULAR_TOLERANCE_RATIO * short_side)
    gap = max(MERGE_GAP_TOLERANCE_MIN_PX,
              MERGE_GAP_TOLERANCE_RATIO * short_side)
    return perpendicular, gap


def _normalize_segment(coords):
    """Return a normalized fragment dict, or None for a degenerate segment."""
    try:
        x1, y1, x2, y2 = (float(value) for value in coords)
    except (TypeError, ValueError):
        return None
    if not all(math.isfinite(value) for value in (x1, y1, x2, y2)):
        return None
    horizontal = abs(x2 - x1) >= abs(y2 - y1)
    # Normalize direction so reversed endpoints describe the same fragment.
    if horizontal:
        if x1 > x2 or (x1 == x2 and y1 > y2):
            x1, y1, x2, y2 = x2, y2, x1, y1
    elif y1 > y2 or (y1 == y2 and x1 > x2):
        x1, y1, x2, y2 = x2, y2, x1, y1
    dx, dy = x2 - x1, y2 - y1
    length = math.hypot(dx, dy)
    if length <= 1e-9:
        return None
    ux, uy = dx / length, dy / length
    angle = (math.degrees(math.atan2(dy, dx)) + 45.0) % 90.0 - 45.0
    return {
        'coords': [x1, y1, x2, y2],
        'x1': x1, 'y1': y1, 'x2': x2, 'y2': y2,
        'ux': ux, 'uy': uy,
        'mx': (x1 + x2) / 2.0, 'my': (y1 + y2) / 2.0,
        'length': length,
        'angle': angle,
        'orientation': 'horizontal' if horizontal else 'vertical',
    }


def _point_line_distance(x, y, line):
    return abs(line['ux'] * (y - line['y1']) - line['uy'] * (x - line['x1']))


def _project_interval(fragment, ux, uy):
    t1 = fragment['x1'] * ux + fragment['y1'] * uy
    t2 = fragment['x2'] * ux + fragment['y2'] * uy
    return (t1, t2) if t1 <= t2 else (t2, t1)


def _add_interval(intervals, lo, hi):
    """Add [lo, hi] to a sorted list of disjoint intervals."""
    if hi < lo:
        lo, hi = hi, lo
    result, placed = [], False
    for a, b in intervals:
        if b < lo:
            result.append((a, b))
        elif hi < a:
            if not placed:
                result.append((lo, hi))
                placed = True
            result.append((a, b))
        else:
            lo, hi = min(lo, a), max(hi, b)
    if not placed:
        result.append((lo, hi))
    return result


def _anchor_compatible(fragment, anchor, angle_tolerance, perpendicular_tolerance):
    """Return whether ``fragment`` may join the group seeded by ``anchor``.

    Every constraint is evaluated against the anchor itself, never against a
    neighbouring fragment, so neither angular nor perpendicular tolerance can
    accumulate along a chain.
    """
    if fragment['orientation'] != anchor['orientation']:
        return False
    if abs(fragment['angle'] - anchor['angle']) > angle_tolerance:
        return False
    return (_point_line_distance(fragment['x1'], fragment['y1'], anchor) <= perpendicular_tolerance and
            _point_line_distance(fragment['x2'], fragment['y2'], anchor) <= perpendicular_tolerance)


def _group_around_anchor(fragments, anchor, angle_tolerance,
                         perpendicular_tolerance, gap_tolerance):
    """Return the anchor's interval-connected component.

    Candidates that pass all anchor checks are projected onto the anchor
    direction.  Projected intervals separated by at most ``gap_tolerance``
    form connected chains, and the whole chain containing the anchor joins
    the group.  Taking the component (instead of only fragments already
    adjacent to accepted members) recovers long connected chains independent
    of fragment processing order or relative lengths.
    """
    intervals = [(*_project_interval(anchor, anchor['ux'], anchor['uy']), anchor)]
    for fragment in fragments:
        if fragment is anchor or not _anchor_compatible(
                fragment, anchor, angle_tolerance, perpendicular_tolerance):
            continue
        lo, hi = _project_interval(fragment, anchor['ux'], anchor['uy'])
        intervals.append((lo, hi, fragment))
    intervals.sort(key=lambda item: (item[0], item[1], item[2]['coords']))

    components = []
    current = []
    current_hi = None
    for lo, hi, fragment in intervals:
        if current and lo - current_hi > gap_tolerance:
            components.append(current)
            current = []
            current_hi = None
        current.append((lo, hi, fragment))
        current_hi = hi if current_hi is None else max(current_hi, hi)
    if current:
        components.append(current)

    for component in components:
        if any(fragment is anchor for _, _, fragment in component):
            return [fragment for _, _, fragment in component]
    return [anchor]  # pragma: no cover - the anchor has its own interval


def _anchor_groups(fragments, angle_tolerance, perpendicular_tolerance, gap_tolerance):
    """Greedily split same-orientation fragments into stable-anchor groups.

    The longest remaining fragment seeds each group and the complete group is
    removed before the next seed is chosen.  Sorting by length with coordinate
    tie-breaking makes the result deterministic and independent of input
    order.
    """
    pending = sorted(fragments, key=lambda fragment: (-fragment['length'], fragment['coords']))
    groups = []
    while pending:
        anchor = pending[0]
        members = _group_around_anchor(pending, anchor, angle_tolerance,
                                       perpendicular_tolerance, gap_tolerance)
        groups.append((anchor, members))
        member_ids = {id(fragment) for fragment in members}
        pending = [fragment for fragment in pending if id(fragment) not in member_ids]
    return groups


def _finalize_cluster(seed, members):
    # Weight fragments by their unique (non-overlapping) contribution.  A
    # duplicate fragment contributes no extra direction evidence.
    initial_ux, initial_uy = seed['ux'], seed['uy']
    ordered = sorted(range(len(members)),
                     key=lambda i: (-members[i]['length'], members[i]['coords']))
    union = []
    weight_by_index = {}
    for index in ordered:
        t_lo, t_hi = _project_interval(members[index], initial_ux, initial_uy)
        overlap = sum(max(0.0, min(t_hi, b) - max(t_lo, a)) for a, b in union)
        weight_by_index[index] = max(0.0, (t_hi - t_lo) - overlap)
        union = _add_interval(union, t_lo, t_hi)

    total_weight = sum(weight_by_index.values())
    if total_weight > 0:
        ux = sum(weight_by_index[i] * members[i]['ux'] for i in range(len(members))) / total_weight
        uy = sum(weight_by_index[i] * members[i]['uy'] for i in range(len(members))) / total_weight
        norm = math.hypot(ux, uy)
        if norm <= 1e-9:
            ux, uy = initial_ux, initial_uy
        else:
            ux, uy = ux / norm, uy / norm
    else:  # pragma: no cover - the longest member always has positive weight
        ux, uy = initial_ux, initial_uy

    normal_x, normal_y = -uy, ux
    offset = sum(weight_by_index[i] * (members[i]['mx'] * normal_x + members[i]['my'] * normal_y)
                 for i in range(len(members))) / max(total_weight, 1e-9)

    intervals = []
    for member in members:
        t_lo, t_hi = _project_interval(member, ux, uy)
        intervals.append((t_lo, t_hi))
    union = []
    for t_lo, t_hi in intervals:
        union = _add_interval(union, t_lo, t_hi)
    support_length = sum(hi - lo for lo, hi in union)
    t_min = min(lo for lo, hi in intervals)
    t_max = max(hi for lo, hi in intervals)
    span_length = t_max - t_min

    start = (normal_x * offset + ux * t_min, normal_y * offset + uy * t_min)
    end = (normal_x * offset + ux * t_max, normal_y * offset + uy * t_max)
    angle = (math.degrees(math.atan2(uy, ux)) + 45.0) % 90.0 - 45.0
    return {
        'coords': [round(start[0], 4), round(start[1], 4),
                   round(end[0], 4), round(end[1], 4)],
        'length': round(support_length, 6),
        'support_length': round(support_length, 6),
        'span_length': round(span_length, 6),
        'gap_length': round(max(0.0, span_length - support_length), 6),
        'angle': round(angle, 6),
        'member_count': len(members),
        'orientation': seed['orientation'],
    }


def merge_collinear_segments(segments, image_shape, *, angle_tolerance_deg=None,
                             perpendicular_tolerance_px=None, gap_tolerance_px=None):
    """Merge collinear fragments and return merged lines.

    ``segments`` is an iterable of ``(x1, y1, x2, y2)`` tuples or dicts with a
    ``coords`` key.  ``image_shape`` is ``(height, width)`` of the analysis
    image; it determines the default pixel tolerances.  Returned dicts carry
    ``coords``, ``angle``, ``length``/``support_length`` (union of observed
    fragment lengths), ``span_length`` (end-to-end extent, gaps included),
    ``gap_length``, ``member_count`` and ``orientation``.

    Horizontal and vertical fragments are kept in separate groups even when
    their normalized skew angle is identical.
    """
    height, width = image_shape[0], image_shape[1]
    default_perpendicular, default_gap = _merge_tolerances(height, width)
    angle_tolerance = (MERGE_ANGLE_TOLERANCE_DEG if angle_tolerance_deg is None
                       else float(angle_tolerance_deg))
    perpendicular_tolerance = (default_perpendicular if perpendicular_tolerance_px is None
                               else float(perpendicular_tolerance_px))
    gap_tolerance = default_gap if gap_tolerance_px is None else float(gap_tolerance_px)
    if not all(math.isfinite(value) and value > 0 for value in
               (angle_tolerance, perpendicular_tolerance, gap_tolerance)):
        raise ValueError('合并容差必须是大于 0 的有限数值')

    fragments = []
    for segment in segments:
        coords = segment.get('coords') if isinstance(segment, dict) else segment
        fragment = _normalize_segment(coords)
        if fragment is not None:
            fragments.append(fragment)
    if not fragments:
        return []

    merged = []
    for orientation in ('horizontal', 'vertical'):
        oriented = [fragment for fragment in fragments
                    if fragment['orientation'] == orientation]
        for seed, members in _anchor_groups(oriented, angle_tolerance,
                                            perpendicular_tolerance, gap_tolerance):
            merged.append(_finalize_cluster(seed, members))
    merged.sort(key=lambda line: (-line['support_length'], line['orientation'],
                                  line['coords']))
    return merged


def _touches_frame(fragment, height, width):
    x1, y1, x2, y2 = fragment['coords']
    return (max(x1, x2) < _FRAME_MARGIN_PX or
            min(x1, x2) > width - _FRAME_MARGIN_PX - 1 or
            max(y1, y2) < _FRAME_MARGIN_PX or
            min(y1, y2) > height - _FRAME_MARGIN_PX - 1)


def estimate(image, max_angle=10.0, analysis_size=2400, *, min_line_percent=18.0,
             min_line_pixels=45.0, resample='lanczos', settings=None):
    """Return estimate plus evidence lines on an EXIF-normalized thumbnail.

    The length filter is applied to merged lines, so individually short
    collinear fragments can form a valid long line.  ``settings``, when given,
    replaces the individual keyword values.  The result retains the original
    keys (``angle``, ``correction``, ``status``, ``count``, ``spread``,
    ``reason``) and adds merge diagnostics; ``count`` is the number of merged
    straight lines selected as evidence.
    """
    if settings is None:
        settings = DetectionSettings(
            max_angle=max_angle,
            min_line_percent=min_line_percent,
            min_line_pixels=min_line_pixels,
            analysis_size=analysis_size,
            resample=resample,
        )
    small = image.convert('RGB')
    small.thumbnail((settings.analysis_size, settings.analysis_size),
                    settings.resample_enum)
    gray = cv2.cvtColor(np.asarray(small), cv2.COLOR_RGB2GRAY)
    height, width = gray.shape
    threshold = effective_min_line_pixels(settings, height, width)
    perpendicular_tolerance, gap_tolerance = _merge_tolerances(height, width)

    found = cv2.createLineSegmentDetector(cv2.LSD_REFINE_STD).detect(gray)[0]
    fragments, ignored_frame = [], 0
    if found is not None:
        for coords in found.reshape(-1, 4):
            fragment = _normalize_segment(coords)
            if fragment is None or abs(fragment['angle']) > settings.max_angle:
                continue
            if _touches_frame(fragment, height, width):
                ignored_frame += 1
                continue
            fragments.append(fragment)

    merged_lines = merge_collinear_segments(
        [fragment['coords'] for fragment in fragments], (height, width),
        angle_tolerance_deg=MERGE_ANGLE_TOLERANCE_DEG,
        perpendicular_tolerance_px=perpendicular_tolerance,
        gap_tolerance_px=gap_tolerance)
    selected = [line for line in merged_lines
                if line['support_length'] >= threshold
                and abs(line['angle']) <= settings.max_angle]

    diagnostics = {
        'angle_tolerance_deg': MERGE_ANGLE_TOLERANCE_DEG,
        'perpendicular_tolerance_px': round(perpendicular_tolerance, 3),
        'gap_tolerance_px': round(gap_tolerance, 3),
        'fragments': len(fragments),
        'merged': len(merged_lines),
        'selected': len(selected),
        'ignored_frame_fragments': ignored_frame,
    }
    common = {
        'fragment_count': len(fragments),
        'merged_count': len(merged_lines),
        'threshold_px': round(threshold, 3),
        'merge': diagnostics,
    }

    if not selected:
        return dict(angle=None, correction=None, status='需人工确认', count=0,
                    spread=None,
                    reason='没有足够的长直线；圆形、无边框或低对比图片可能无法判断',
                    **common), small, []

    # Choose the strongest angular consensus; long merged lines carry more
    # weight.  Support length excludes overlaps and gaps, so evidence is not
    # inflated by merging.
    angles = np.array([line['angle'] for line in selected])
    weights = np.array([line['support_length'] for line in selected])
    support = np.array([weights[np.abs(angles - a) <= .35].sum() for a in angles])
    center = angles[int(np.argmax(support))]
    keep = np.abs(angles - center) <= .35
    center = weighted_median(angles[keep], weights[keep])
    keep = np.abs(angles - center) <= .35
    selected = [line for line, ok in zip(selected, keep) if ok]
    angle = weighted_median(angles[keep], weights[keep])
    spread = float(np.sqrt(np.average((angles[keep] - angle) ** 2,
                                      weights=weights[keep])))
    # Distinguish separate physical lines from the two sides of a single edge.
    groups = []
    for line in sorted(selected, key=lambda item: -item['support_length']):
        x1, y1, x2, y2 = line['coords']
        horizontal = line['orientation'] == 'horizontal'
        intercept = ((y1 + y2) / 2 - math.tan(math.radians(angle)) * (x1 + x2) / 2
                     if horizontal else
                     (x1 + x2) / 2 + math.tan(math.radians(angle)) * (y1 + y2) / 2)
        if all(horizontal != kind or abs(intercept - position) > min(height, width) * .025
               for kind, position in groups):
            groups.append((horizontal, intercept))
    ratio = float(weights[keep].sum() / weights.sum())
    enough = len(groups) >= 2 and weights[keep].sum() >= min(height, width) * .7
    conflict = ratio < .65
    status = '可信' if enough and not conflict and spread < .18 else '需人工确认'
    reason = ('多条分离直线方向一致' if status == '可信' else
              '直线证据不足或方向存在冲突；请核对预览')
    if abs(angle) > settings.max_angle - .3:
        status, reason = '需人工确认', '角度接近检测范围上限；建议扩大范围复核'
    common['merge']['selected'] = len(selected)
    return dict(angle=round(angle, 3), correction=round(-angle, 3), status=status,
                count=len(selected), spread=round(spread, 3), reason=reason,
                **common), small, selected
