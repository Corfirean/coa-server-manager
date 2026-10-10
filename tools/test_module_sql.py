"""Exercise raid module initialization against disposable MySQL data only."""
import argparse
import socket
import subprocess
import tempfile
import time
from pathlib import Path


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--mysql-bin', required=True, type=Path)
    parser.add_argument('--core', required=True, type=Path)
    parser.add_argument('--before', required=True)
    args = parser.parse_args()
    suffix = '.exe' if (args.mysql_bin / 'mysqld.exe').exists() else ''
    server = args.mysql_bin / ('mysqld' + suffix)
    client = args.mysql_bin / ('mysql' + suffix)
    admin = args.mysql_bin / ('mysqladmin' + suffix)
    relative = 'modules/mod-coa-raid-difficulty/data/sql/db-world/base/03_boss_schedule.sql'
    original = subprocess.check_output(['git', '-C', str(args.core), 'show', args.before + ':' + relative], text=True)
    current = (args.core / relative).read_text()
    split = (args.core / 'data/sql/updates/pending_db_world/rev_20261001_g1_molten_core_shazzrah_nova_area.sql').read_text()
    with tempfile.TemporaryDirectory(prefix='coa-module-sql-fixture-') as temp:
        data = Path(temp) / 'data'
        flags = subprocess.CREATE_NO_WINDOW if hasattr(subprocess, 'CREATE_NO_WINDOW') else 0
        subprocess.run([str(server), '--no-defaults', '--initialize-insecure', '--datadir=' + str(data)],
                       check=True, capture_output=True, creationflags=flags)
        with socket.socket() as sock:
            sock.bind(('127.0.0.1', 0))
            port = sock.getsockname()[1]
        process = subprocess.Popen([str(server), '--no-defaults', '--datadir=' + str(data),
                                    '--bind-address=127.0.0.1', '--port=' + str(port), '--mysqlx=0',
                                    '--skip-log-bin', '--log-error=' + str(Path(temp) / 'mysql.log')],
                                   creationflags=flags, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        connection = ['--no-defaults', '--protocol=TCP', '--host=127.0.0.1', '--port=' + str(port), '--user=root']

        def sql(statement, schema=None, check=True):
            result = subprocess.run([str(client), *connection, '--batch', '--skip-column-names', *([schema] if schema else [])],
                                    input=statement, text=True, capture_output=True, creationflags=flags)
            if check and result.returncode:
                raise RuntimeError(result.stderr)
            return result

        try:
            deadline = time.monotonic() + 60
            while sql('SELECT 1;', check=False).returncode:
                if process.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError('Disposable MySQL did not start: ' + (Path(temp) / 'mysql.log').read_text())
                time.sleep(0.5)
            for schema in ('fresh', 'old', 'upgraded'):
                sql('CREATE DATABASE `' + schema + '`;')
                sql('CREATE TABLE creature_template (entry INT PRIMARY KEY, ScriptName VARCHAR(128));', schema)
            sql(original, 'old')
            sql('CREATE TABLE coa_spell_damage_info (spell_id INT); CREATE TABLE spell_script_names (spell_id INT, ScriptName VARCHAR(128));', 'old')
            sql(split, 'old')
            broken = sql(original, 'old', check=False)
            assert broken.returncode and '1136' in broken.stderr, broken.stderr
            sql(current, 'fresh')
            sql(current, 'fresh')
            sql(original, 'upgraded')
            sql(current, 'upgraded')
            for schema in ('fresh', 'upgraded'):
                count = sql("SELECT COUNT(*) FROM information_schema.columns WHERE table_schema=DATABASE() AND table_name='coa_boss_schedule';", schema).stdout.strip()
                assert count == '15', count
                effects = sql('SELECT effect_d0,effect_d1,effect_d2,effect_d3 FROM coa_boss_schedule WHERE entry=12264 AND idx=6;', schema).stdout.strip()
                assert effects == '2105613\t2105614\t2105615\t2105616', effects
            print('PASS: original SQL reproduces ERROR 1136 after migration; fixed SQL supports fresh, repeated and old-layout initialization')
        finally:
            subprocess.run([str(admin), *connection, 'shutdown'], capture_output=True, creationflags=flags)
            try:
                process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()


if __name__ == '__main__':
    main()
