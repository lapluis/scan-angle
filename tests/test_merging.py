import math
import random
import unittest

from scan_angle.detection import (DetectionSettings, effective_merge_tolerances,
                                  effective_min_line_pixels,
                                  merge_collinear_segments)


class MergeTests(unittest.TestCase):
    def test_collinear_reversed_and_overlapping_fragments_merge(self):
        # A and B are separated by a 10 px gap; B and C overlap by 15 px.
        # C is supplied with reversed endpoints.
        segments = [(100, 500, 300, 500),
                    (375, 500, 310, 500),
                    (500, 500, 360, 500)]
        merged = merge_collinear_segments(segments, (2400, 2400))
        self.assertEqual(len(merged), 1)
        line = merged[0]
        self.assertEqual(line['orientation'], 'horizontal')
        self.assertAlmostEqual(line['angle'], 0, delta=0.01)
        self.assertEqual(line['member_count'], 3)
        # Union of [100,300], [310,375] and [360,500] is 200 + 190 = 390.
        self.assertAlmostEqual(line['support_length'], 390, delta=0.01)
        self.assertAlmostEqual(line['span_length'], 400, delta=0.01)
        # The overlap must not be counted twice and the 10 px gap is not
        # evidence: total raw fragment length is 405.
        self.assertLess(line['support_length'], 405)
        self.assertLess(line['support_length'], line['span_length'])

    def test_reversed_vertical_endpoints_merge(self):
        segments = [(500, 100, 500, 300),
                    (500, 310, 500, 380),
                    (500, 500, 500, 390)]
        merged = merge_collinear_segments(segments, (2400, 2400))
        self.assertEqual(len(merged), 1)
        line = merged[0]
        self.assertEqual(line['orientation'], 'vertical')
        self.assertEqual(line['member_count'], 3)
        self.assertAlmostEqual(line['support_length'], 380, delta=0.01)
        self.assertAlmostEqual(line['span_length'], 400, delta=0.01)

    def test_parallel_perpendicular_and_crossing_lines_do_not_merge(self):
        # Two horizontal fragments 4 px apart are distinct parallels; the
        # vertical line has the same normalized 0-degree angle but a different
        # orientation and must never merge with them.
        segments = [(100, 500, 600, 500),
                    (100, 504, 600, 504),
                    (300, 100, 300, 600)]
        merged = merge_collinear_segments(segments, (2400, 2400))
        self.assertEqual(len(merged), 3)
        orientations = sorted(line['orientation'] for line in merged)
        self.assertEqual(orientations, ['horizontal', 'horizontal', 'vertical'])
        self.assertTrue(all(line['member_count'] == 1 for line in merged))

    def test_chained_drift_is_split_into_distinct_lines(self):
        # A-B and B-C are each within tolerance, but A-C is not.  The anchor
        # check must keep A+B together without swallowing C.
        segments = [(100, 500, 300, 500),
                    (310, 503, 510, 503),
                    (520, 506, 720, 506)]
        merged = merge_collinear_segments(
            segments, (2400, 2400), angle_tolerance_deg=1.0,
            perpendicular_tolerance_px=3.0, gap_tolerance_px=20.0)
        self.assertEqual(len(merged), 2)
        self.assertEqual(merged[0]['member_count'], 2)
        self.assertEqual(merged[1]['member_count'], 1)
        self.assertAlmostEqual(merged[0]['support_length'], 400, delta=0.01)

    def test_large_unobserved_gap_is_not_bridged_or_counted(self):
        segments = [(100, 500, 200, 500), (1000, 500, 1100, 500)]
        merged = merge_collinear_segments(segments, (2400, 2400))
        self.assertEqual(len(merged), 2)
        self.assertTrue(all(line['member_count'] == 1 for line in merged))

    def test_long_connected_chain_merges_despite_uneven_fragment_lengths(self):
        # Exact regression for the two-pass clustering bug: every adjacent
        # gap is <= 5 px (default tolerance is 6 px at short side 1000), but
        # the earlier greedy passes split this 9-fragment chain into 7 + 2
        # because a short bridge was visited after a farther fragment.
        segments = [
            (20, 100, 72, 100),
            (77, 100, 220, 100),
            (221, 100, 310, 100),
            (313, 100, 395, 100),
            (397, 100, 451, 100),
            (453, 100, 502, 100),
            (503, 100, 581, 100),
            (583, 100, 605, 100),
            (609, 100, 747, 100),
        ]
        merged = merge_collinear_segments(segments, (1000, 2000))
        self.assertEqual(len(merged), 1)
        line = merged[0]
        self.assertEqual(line['orientation'], 'horizontal')
        self.assertEqual(line['member_count'], 9)
        self.assertAlmostEqual(line['angle'], 0.0, delta=0.01)
        # 727 px span minus 20 px total gap; gaps are not counted as support.
        self.assertAlmostEqual(line['support_length'], 707, delta=0.01)
        self.assertAlmostEqual(line['span_length'], 727, delta=0.01)
        self.assertAlmostEqual(line['gap_length'], 20, delta=0.01)

    @staticmethod
    def _signature(lines):
        return [(line['orientation'], line['member_count'],
                 round(line['support_length'], 6),
                 round(line['span_length'], 6),
                 tuple(round(value, 6) for value in line['coords']))
                for line in lines]

    def test_random_connected_chains_are_order_independent(self):
        rng = random.Random(20261020)
        for trial in range(40):
            count = rng.randint(3, 14)
            horizontal = trial % 2 == 0
            cursor = rng.uniform(120, 400)
            base = rng.uniform(350, 650)
            segments = []
            for _ in range(count):
                length = rng.uniform(20, 180)
                # Perpendicular jitter stays well inside the 1.5 px default;
                # adjacent gaps stay inside the 6 px default.
                jitter = rng.uniform(-0.5, 0.5)
                if horizontal:
                    segments.append((cursor, base + jitter,
                                     cursor + length, base + jitter))
                else:
                    segments.append((base + jitter, cursor,
                                     base + jitter, cursor + length))
                cursor += length + rng.uniform(0.0, 5.9)
            reference = merge_collinear_segments(segments, (1000, 2000))
            shuffled = list(segments)
            rng.shuffle(shuffled)
            permuted = merge_collinear_segments(shuffled, (1000, 2000))

            with self.subTest(trial=trial, count=count, horizontal=horizontal):
                self.assertEqual(len(reference), 1)
                line = reference[0]
                self.assertEqual(line['member_count'], count)
                self.assertAlmostEqual(line['angle'], 0.0, delta=0.01)
                axis = 0 if horizontal else 1
                starts = [segment[axis] for segment in segments]
                ends = [segment[axis + 2] for segment in segments]
                self.assertAlmostEqual(line['span_length'],
                                       max(ends) - min(starts), delta=1e-6)
                self.assertAlmostEqual(line['support_length'],
                                       sum(end - start for start, end in
                                           zip(starts, ends)), delta=1e-6)
                self.assertEqual(self._signature(reference),
                                 self._signature(permuted))

    def test_random_chain_break_above_gap_tolerance_stays_separate(self):
        rng = random.Random(314159)
        for trial in range(10):
            counts = [rng.randint(2, 6), rng.randint(2, 6)]
            cursor = rng.uniform(100, 300)
            segments = []
            for chain, count in enumerate(counts):
                if chain:
                    # An unobserved break that exceeds the 6 px tolerance.
                    cursor += rng.uniform(6.5, 30.0)
                for _ in range(count):
                    length = rng.uniform(20, 100)
                    segments.append((cursor, 500, cursor + length, 500))
                    cursor += length + rng.uniform(0.0, 5.0)
            rng.shuffle(segments)
            merged = merge_collinear_segments(segments, (1000, 2000))
            with self.subTest(trial=trial, counts=counts):
                self.assertEqual(len(merged), 2)
                self.assertEqual(sorted(line['member_count'] for line in merged),
                                 sorted(counts))

    def test_angle_tolerance_does_not_accumulate_through_neighbours(self):
        # The anchor is horizontal.  B is +0.7 degrees and C is +1.4
        # degrees: C is within 0.75 degrees of B but not of the anchor, so
        # it must not join through transitive angular tolerance growth.
        anchor = (100, 500, 500, 500)
        rise_b = 100 * math.tan(math.radians(0.7))
        b = (505, 500 - rise_b / 2, 605, 500 + rise_b / 2)
        rise_c = 100 * math.tan(math.radians(1.4))
        c = (610, 500 - rise_c / 2, 710, 500 + rise_c / 2)
        merged = merge_collinear_segments([anchor, b, c], (1000, 2000))
        self.assertEqual(len(merged), 2)
        by_count = {line['member_count']: line for line in merged}
        self.assertEqual(sorted(by_count), [1, 2])
        # The anchor+B line is nearly horizontal; C keeps its own angle.
        self.assertAlmostEqual(by_count[2]['angle'], 0.14, delta=0.03)
        self.assertAlmostEqual(by_count[1]['angle'], 1.4, delta=0.05)

    def test_configured_angle_tolerance_controls_grouping(self):
        # Both fragments have endpoints within 10 px of the anchor and a
        # 10 px projected gap.  Their angle difference is about 1.43
        # degrees, so only a tolerance above that may join them.
        # The anchor is deliberately the longest fragment so that both
        # endpoint-distance checks are made against the horizontal line.
        anchor = (100, 500, 1100, 500)
        sloped = (1110, 490, 1910, 510)
        narrow = merge_collinear_segments(
            [anchor, sloped], (1000, 2000), angle_tolerance_deg=0.75,
            perpendicular_tolerance_px=20.0, gap_tolerance_px=100.0)
        wide = merge_collinear_segments(
            [anchor, sloped], (1000, 2000), angle_tolerance_deg=1.5,
            perpendicular_tolerance_px=20.0, gap_tolerance_px=100.0)
        self.assertEqual(len(narrow), 2)
        self.assertEqual(len(wide), 1)
        self.assertEqual(wide[0]['member_count'], 2)

    def test_zero_distance_tolerance_requires_exact_collinearity(self):
        exact = [(100, 500, 200, 500), (210, 500, 300, 500)]
        offset = [(100, 500, 200, 500), (210, 501, 300, 501)]
        merged = merge_collinear_segments(
            exact, (1000, 1000), perpendicular_tolerance_px=0.0,
            gap_tolerance_px=10.0)
        separated = merge_collinear_segments(
            offset, (1000, 1000), perpendicular_tolerance_px=0.0,
            gap_tolerance_px=10.0)
        self.assertEqual(len(merged), 1)
        self.assertEqual(merged[0]['member_count'], 2)
        self.assertEqual(len(separated), 2)
        self.assertTrue(all(line['member_count'] == 1 for line in separated))

    def test_zero_gap_tolerance_requires_touching_or_overlapping_intervals(self):
        touching = [(100, 500, 200, 500), (200, 500, 300, 500)]
        overlapping = [(100, 500, 200, 500), (150, 500, 300, 500)]
        positively_separated = [(100, 500, 200, 500), (200.001, 500, 300, 500)]
        for segments in (touching, overlapping):
            with self.subTest(segments=segments):
                merged = merge_collinear_segments(
                    segments, (1000, 1000), perpendicular_tolerance_px=0.0,
                    gap_tolerance_px=0.0)
                self.assertEqual(len(merged), 1)
                self.assertEqual(merged[0]['member_count'], 2)
        separated = merge_collinear_segments(
            positively_separated, (1000, 1000), perpendicular_tolerance_px=0.0,
            gap_tolerance_px=0.0)
        self.assertEqual(len(separated), 2)

    def test_merge_tolerances_validate_range_and_finiteness(self):
        segments = [(100, 500, 200, 500)]
        invalid = [
            {'angle_tolerance_deg': 0},
            {'angle_tolerance_deg': -0.1},
            {'angle_tolerance_deg': 45},
            {'angle_tolerance_deg': 46},
            {'angle_tolerance_deg': float('nan')},
            {'angle_tolerance_deg': float('inf')},
            {'perpendicular_tolerance_px': -0.1},
            {'perpendicular_tolerance_px': float('nan')},
            {'perpendicular_tolerance_px': float('inf')},
            {'gap_tolerance_px': -0.1},
            {'gap_tolerance_px': float('nan')},
            {'gap_tolerance_px': float('inf')},
        ]
        for kwargs in invalid:
            with self.subTest(kwargs=kwargs), self.assertRaises(ValueError):
                merge_collinear_segments(segments, (1000, 1000), **kwargs)
        # Zero distance/gap and any angle strictly inside (0, 45) are valid.
        merged = merge_collinear_segments(
            segments, (1000, 1000), angle_tolerance_deg=44.9,
            perpendicular_tolerance_px=0.0, gap_tolerance_px=0.0)
        self.assertEqual(len(merged), 1)

    def test_default_low_level_tolerances_match_explicit_legacy_values(self):
        segments = [(100, 500, 300, 500), (305, 500, 500, 500),
                    (100, 600, 300, 601)]
        default = merge_collinear_segments(segments, (1000, 2000))
        explicit = merge_collinear_segments(
            segments, (1000, 2000), angle_tolerance_deg=0.75,
            perpendicular_tolerance_px=max(1.5, 0.0012 * 1000),
            gap_tolerance_px=max(6.0, 0.005 * 1000))
        self.assertEqual(default, explicit)

    def test_duplicate_fragments_do_not_inflate_support(self):
        segments = [(100, 500, 300, 500),
                    (100, 500, 300, 500),
                    (310, 500, 400, 500)]
        merged = merge_collinear_segments(segments, (2400, 2400))
        self.assertEqual(len(merged), 1)
        line = merged[0]
        self.assertEqual(line['member_count'], 3)
        # [100, 300] union [310, 400] = 290; the duplicate adds no evidence.
        self.assertAlmostEqual(line['support_length'], 290, delta=0.01)
        self.assertAlmostEqual(line['span_length'], 300, delta=0.01)

    def test_short_fragments_cross_threshold_only_after_merging(self):
        # Six 40 px dashes separated by 5 px gaps on a 600 px analysis image.
        segments = [(start, 300, start + 40, 300)
                    for start in range(100, 100 + 6 * 45, 45)]
        merged = merge_collinear_segments(segments, (600, 600))
        self.assertEqual(len(merged), 1)
        line = merged[0]
        self.assertEqual(line['member_count'], 6)
        self.assertAlmostEqual(line['support_length'], 240, delta=0.01)
        self.assertAlmostEqual(line['span_length'], 265, delta=0.01)

        threshold = effective_min_line_pixels(DetectionSettings(), 600, 600,
                                              'horizontal')
        self.assertEqual(threshold, 60)
        self.assertTrue(all(40 < threshold for _ in segments))
        self.assertGreaterEqual(line['support_length'], threshold)

    def test_effective_threshold_uses_orientation_axis_and_larger_value(self):
        percent_wins = DetectionSettings(min_line_percent=18, min_line_pixels=45)
        # A horizontal line's available extent is the width; a vertical
        # line's is the height.
        self.assertEqual(
            effective_min_line_pixels(percent_wins, 1000, 2000, 'horizontal'), 360)
        self.assertEqual(
            effective_min_line_pixels(percent_wins, 1000, 2000, 'vertical'), 180)
        pixels_win = DetectionSettings(min_line_percent=1, min_line_pixels=45)
        self.assertEqual(
            effective_min_line_pixels(pixels_win, 1000, 2000, 'horizontal'), 45)
        self.assertEqual(
            effective_min_line_pixels(pixels_win, 1000, 2000, 'vertical'), 45)

    def test_effective_threshold_rejects_unknown_orientation(self):
        settings = DetectionSettings()
        for orientation in ['diagonal', 'Horizontal', '', None, 0]:
            with self.subTest(orientation=orientation), \
                    self.assertRaises(ValueError):
                effective_min_line_pixels(settings, 1000, 2000, orientation)


class EffectiveMergeToleranceTests(unittest.TestCase):
    def test_percentages_and_pixel_floors_use_the_short_side(self):
        # Short side 600: 0.5% = 3 px beats the 1.5 px floor, and 2% = 12 px
        # beats the 6 px floor.  The long side must not be used.
        percent_wins = DetectionSettings(merge_distance_percent=0.5,
                                         merge_distance_pixels=1.5,
                                         merge_gap_percent=2.0,
                                         merge_gap_pixels=6.0)
        self.assertEqual(effective_merge_tolerances(percent_wins, 600, 2400),
                         (3.0, 12.0))

        # Defaults on the same dimensions: 0.12% = 0.72 px and 0.5% = 3 px,
        # so both absolute floors win.
        defaults = DetectionSettings()
        self.assertEqual(effective_merge_tolerances(defaults, 600, 2400),
                         (1.5, 6.0))

        # Zero percent and zero pixels produce a genuine zero tolerance, not
        # an epsilon-substituted positive tolerance.
        zero = DetectionSettings(merge_distance_percent=0,
                                 merge_distance_pixels=0,
                                 merge_gap_percent=0,
                                 merge_gap_pixels=0)
        self.assertEqual(effective_merge_tolerances(zero, 600, 2400),
                         (0.0, 0.0))


class SettingsValidationTests(unittest.TestCase):
    def test_defaults(self):
        settings = DetectionSettings()
        self.assertEqual(settings.as_dict(), {
            'max_angle': 15.0,
            'min_line_percent': 10.0,
            'min_line_pixels': 50.0,
            'analysis_size': 2400,
            'resample': 'lanczos',
            'merge_angle': 0.75,
            'merge_distance_percent': 0.12,
            'merge_distance_pixels': 1.5,
            'merge_gap_percent': 0.5,
            'merge_gap_pixels': 6.0,
        })

    def test_invalid_numeric_settings_are_rejected(self):
        invalid = [
            {'min_line_percent': 0},
            {'min_line_percent': -1},
            {'min_line_percent': 100.1},
            {'min_line_percent': float('nan')},
            {'min_line_percent': float('inf')},
            {'min_line_pixels': 0},
            {'min_line_pixels': -5},
            {'min_line_pixels': float('nan')},
            {'min_line_pixels': float('inf')},
            {'analysis_size': 255},
            {'analysis_size': 10001},
            {'analysis_size': 2400.5},
            {'analysis_size': True},
            {'resample': 'cubic'},
            {'resample': ''},
            {'max_angle': float('nan')},
            {'merge_angle': 0},
            {'merge_angle': -0.1},
            {'merge_angle': 45},
            {'merge_angle': 45.1},
            {'merge_angle': float('nan')},
            {'merge_angle': float('inf')},
            {'merge_distance_percent': -0.01},
            {'merge_distance_percent': float('nan')},
            {'merge_distance_percent': float('inf')},
            {'merge_distance_pixels': -0.01},
            {'merge_distance_pixels': float('nan')},
            {'merge_distance_pixels': float('inf')},
            {'merge_gap_percent': -0.01},
            {'merge_gap_percent': float('nan')},
            {'merge_gap_percent': float('inf')},
            {'merge_gap_pixels': -0.01},
            {'merge_gap_pixels': float('nan')},
            {'merge_gap_pixels': float('inf')},
        ]
        for kwargs in invalid:
            with self.subTest(kwargs=kwargs), self.assertRaises(ValueError):
                DetectionSettings(**kwargs)

    def test_valid_boundaries_and_case_insensitive_resample(self):
        self.assertEqual(DetectionSettings(min_line_percent=100,
                                           min_line_pixels=0.1).min_line_percent, 100)
        self.assertEqual(DetectionSettings(analysis_size=256).analysis_size, 256)
        self.assertEqual(DetectionSettings(analysis_size=10000).analysis_size, 10000)
        self.assertEqual(DetectionSettings(resample=' LANCZOS ').resample, 'lanczos')
        self.assertEqual(DetectionSettings(resample='bicubic').resample_enum.name,
                         'BICUBIC')
        settings = DetectionSettings(merge_angle=0.001,
                                     merge_distance_percent=0,
                                     merge_distance_pixels=0,
                                     merge_gap_percent=0,
                                     merge_gap_pixels=0)
        self.assertEqual(settings.merge_angle, 0.001)
        self.assertEqual(settings.merge_distance_percent, 0.0)
        self.assertEqual(settings.merge_distance_pixels, 0.0)
        self.assertEqual(settings.merge_gap_percent, 0.0)
        self.assertEqual(settings.merge_gap_pixels, 0.0)
        self.assertEqual(DetectionSettings(merge_angle=44.999).merge_angle, 44.999)


if __name__ == '__main__':
    unittest.main()
