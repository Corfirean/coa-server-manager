import runpy
import sys
from pathlib import Path

script = sys.argv[1]
sys.path.insert(0, str(Path(script).resolve().parent))
sys.argv = sys.argv[1:]
launcher = runpy.run_path(script, run_name="coa_manager_launcher")
original_wait = launcher["wait_ready"]


def wait_ready(name, port, seconds=180):
    return original_wait(name, port, max(seconds, 300) if name == "mysql" else seconds)


launcher["start_mysql"].__globals__["wait_ready"] = wait_ready
if sys.argv[1] == "stop-all":
    config = launcher["settings"]()
    with launcher["control_lock"]():
        launcher["stop_all"](config)
else:
    launcher["main"]()
