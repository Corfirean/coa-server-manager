import runpy
import sys
import time
from pathlib import Path

script = sys.argv[1]
sys.path.insert(0, str(Path(script).resolve().parent))
sys.argv = sys.argv[1:]
launcher = runpy.run_path(script, run_name="coa_manager_launcher")
original_wait = launcher["wait_ready"]
world_deadline = None


def wait_ready(name, port, seconds=180):
    global world_deadline
    if name == "mysql":
        seconds = max(seconds, 300)
    elif name == "world":
        # Both game and console listeners belong to one startup, including on old repacks.
        # Ordinary CoA worlds can load slowly too; this must not depend on SQUID.
        if world_deadline is None:
            world_deadline = time.monotonic() + max(seconds, 1500)
        seconds = max(0, world_deadline - time.monotonic())
    return original_wait(name, port, seconds)


launcher["start_mysql"].__globals__["wait_ready"] = wait_ready
if sys.argv[1] == "stop-all":
    shutdown = launcher["stop_all"].__globals__
    if "ra" in shutdown:
        original_ra = shutdown["ra"]

        class UnsafeShutdown(Exception):
            pass

        def shutdown_command(command, config):
            try:
                return original_ra(command, config)
            except (OSError, RuntimeError):
                if not command.startswith("server shutdown "):
                    raise
                world = shutdown["process"]("world")
                if not world:
                    return "Worldserver has already stopped."
                expected = Path(script).resolve().parents[1] / "Core" / "worldserver.exe"
                if Path(world.get("exe", "")) != expected or not shutdown["same_process"](world):
                    raise UnsafeShutdown("World process ownership could not be verified; the database was left running.")
                try:
                    shutdown["terminate"](world)
                except (OSError, RuntimeError) as error:
                    raise UnsafeShutdown("Worldserver could not be stopped safely; its database was left running.") from error
                if not shutdown["wait_stopped"](world, 90):
                    raise UnsafeShutdown("Worldserver is still stopping; its database was left running.")
                return "Stopped this server's recorded world process without its unavailable console."

        shutdown["ra"] = shutdown_command
    config = launcher["settings"]()
    with launcher["control_lock"]():
        launcher["stop_all"](config)
else:
    launcher["main"]()
