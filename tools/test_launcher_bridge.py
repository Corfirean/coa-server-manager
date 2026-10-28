import contextlib
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
            script = pathlib.Path(directory) / 'manage.py'
            script.write_text(body, encoding='utf8')
            with patch.object(sys, 'argv', ['bridge', str(script), action]):
                return runpy.run_path(str(BRIDGE))

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
