import pathlib
import runpy
import sys
import tempfile
import unittest
from unittest.mock import patch

BRIDGE = pathlib.Path(__file__).resolve().parents[1] / 'crates/coa-core/src/launcher_bridge.py'


class LauncherBridgeTests(unittest.TestCase):
    def launch(self, action, body):
        with tempfile.TemporaryDirectory() as directory:
            script = pathlib.Path(directory) / 'Scripts/manage.py'
            script.parent.mkdir()
            script.write_text(body, encoding='utf8')
            original = runpy.run_path
            def capture(*args, **kwargs):
                self.launcher = original(*args, **kwargs)
                return self.launcher
            with patch.object(sys, 'argv', ['bridge', str(script), action]), patch.object(runpy, 'run_path', side_effect=capture):
                return original(str(BRIDGE))

    def shutdown_fixture(self, *, console_error='RuntimeError', stopped=True, foreign=False, same=True, modern=False, terminate_error=False):
        return f'''
from pathlib import Path
from contextlib import nullcontext
calls = []
world = {{'pid': 123, 'created': 456, 'exe': str(Path(__file__).resolve().parents[1] / {'"foreign.exe"' if foreign else '"Core/worldserver.exe"'})}}
def wait_ready(*args): pass
def start_mysql(): raise AssertionError('must not start')
def main(): raise AssertionError('must not prepare')
def settings(): return {{}}
def control_lock(): return nullcontext()
def process(name): return world if name == 'world' else None
def same_process(record): return {same!r}
def ra(command, config):
    calls.append('console')
    {'raise '+console_error+'("Start this repack worldserver first")' if console_error else 'return "Graceful shutdown"'}
def terminate(record):
    assert record is world
    calls.append('terminate')
    {'raise RuntimeError("identity changed")' if terminate_error else 'pass'}
def wait_stopped(record, seconds):
    assert record is world
    calls.append('wait')
    return {stopped!r}
def stop_all(config):
    record = process('world')
    if record:
        if {modern!r}:
            try: ra('server shutdown 1s 0', config)
            except (OSError, RuntimeError): terminate(record)
        else:
            ra('server shutdown 1s 0', config)
        if not wait_stopped(record, 60): raise RuntimeError('world still alive')
    calls.append('mysql-shutdown')
'''

    def test_unavailable_console_in_restored_legacy_launcher_stops_owned_world_before_mysql(self):
        for error in ['RuntimeError', 'OSError']:
            with self.subTest(error=error):
                result = self.launch('stop-all', self.shutdown_fixture(console_error=error))
                self.assertEqual(result['launcher']['calls'], ['console', 'terminate', 'wait', 'wait', 'mysql-shutdown'])

    def test_healthy_console_keeps_graceful_shutdown(self):
        result = self.launch('stop-all', self.shutdown_fixture(console_error=None))
        self.assertEqual(result['launcher']['calls'], ['console', 'wait', 'mysql-shutdown'])

    def test_foreign_or_reused_process_is_never_terminated_even_by_a_modern_launcher_catch(self):
        for modern in [False, True]:
            for identity in [{'foreign': True}, {'same': False}]:
                with self.subTest(modern=modern, **identity):
                    with self.assertRaisesRegex(Exception, 'ownership could not be verified'):
                        self.launch('stop-all', self.shutdown_fixture(modern=modern, **identity))
                    self.assertEqual(self.launcher['calls'], ['console'])

    def test_world_that_stays_alive_blocks_database_shutdown(self):
        for modern in [False, True]:
            with self.subTest(modern=modern):
                with self.assertRaisesRegex(Exception, 'database was left running'):
                    self.launch('stop-all', self.shutdown_fixture(stopped=False, modern=modern))
                self.assertEqual(self.launcher['calls'], ['console', 'terminate', 'wait'])

    def test_termination_identity_failure_leaves_mysql_untouched(self):
        for modern in [False, True]:
            with self.subTest(modern=modern):
                with self.assertRaisesRegex(Exception, 'could not be stopped safely'):
                    self.launch('stop-all', self.shutdown_fixture(terminate_error=True, modern=modern))
                self.assertEqual(self.launcher['calls'], ['console', 'terminate'])

    def test_slow_mysql_gets_time_to_finish_but_other_waits_are_unchanged(self):
        result = self.launch('start-mysql', '''
calls = []
def wait_ready(name, port, seconds=180):
    calls.append((name, seconds))
    if name == 'mysql' and seconds < 162:
        raise RuntimeError('mysql is still starting')
def start_mysql():
    wait_ready('mysql', 12345, 60)
def main():
    start_mysql()
    wait_ready('world', 23456, 180)
    wait_ready('mysql', 12345, 900)
''')
        self.assertEqual(result['launcher']['calls'], [('mysql', 300), ('world', 180), ('mysql', 900)])

    def test_stop_dispatch_does_not_prepare_or_start_anything(self):
        result = self.launch('stop-all', '''
from contextlib import nullcontext
calls = []
def wait_ready(*args): raise AssertionError('must not wait')
def start_mysql(): raise AssertionError('must not start')
def main(): raise AssertionError('must not prepare')
def settings(): return {'mysqlPort': 12345}
def control_lock(): return nullcontext()
def stop_all(config): calls.append(config['mysqlPort'])
''')
        self.assertEqual(result['launcher']['calls'], [12345])


if __name__ == '__main__':
    unittest.main()
