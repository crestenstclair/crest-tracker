"""Independent timing expectations for the native ALSA probe's evidence reader."""
import unittest

from linux_midi_clock_probe import summarize


def trace():
    rows = ['timer,0,1,3,0,0', 'port,1,1,7']
    for index in range(202):
        queue = (index + 1) * 5_000_000
        rows.append(f'sample,{queue + 999_999_900},{queue},{queue + 1_000_000_100},1')
    rows.extend(['event,1005000000,1007000000,1010000000,2040000100,1,7',
                 'sample,2040001000,500,2040001200,1'])
    return '\n'.join(rows)


class AlsaProbeEvidenceTests(unittest.TestCase):
    def test_known_epoch_and_delayed_read_do_not_claim_live_qualification(self):
        report = summarize(trace())
        self.assertTrue(report['probe_passed'])
        self.assertFalse(report['live_capture_qualified'])
        self.assertFalse(report['physical_workflow_qualified'])
        self.assertEqual(report['observed_offset_span_ns'], 0)
        self.assertEqual(report['max_query_span_ns'], 200)
        self.assertEqual(report['delayed_receive_ns'], 30_000_000)

    def test_wrong_queue_retimed_event_stopped_queue_and_missing_reset_reject(self):
        invalid = [
            trace().replace('2040000100,1,7', '2040000100,1,8'),
            trace().replace('1007000000', '1040000000'),
            trace().replace('1007000000', '1004000000'),
            trace().replace('sample,1004999900,5000000,1005000100,1',
                            'sample,1004999900,5000000,1005000100,0'),
            trace().replace('2040001000,500,2040001200', '2040001000,1040000000,2040001200'),
            trace().replace('2040000100,1,7', '2020000100,1,7'),
            trace().rsplit('\n', 1)[0],
        ]
        for candidate in invalid:
            with self.subTest(candidate=candidate[-150:]):
                with self.assertRaises(ValueError):
                    summarize(candidate)
