"""Reference-runtime controls against the SAME explicitly allowed interpreter.
No Python code ever runs between vfork and exec; subprocess owns that boundary.
"""
import errno
import json
import os
from pathlib import Path
import signal
import subprocess
import sys

signal.alarm(12)
target, route = sys.argv[1:3]
args = [target, "-I", "-B", "-c", "from pathlib import Path; Path('python-child.executed').write_text('allowed-target')"]
try:
    if route == "os.posix_spawn":
        child = os.posix_spawn(target, args, os.environ)
        _, status = os.waitpid(child, 0)
        status = os.waitstatus_to_exitcode(status)
    elif route == "subprocess":
        status = subprocess.run(args, check=False, timeout=6).returncode
    else:
        raise ValueError("unknown route")
    result = {"route": route, "outcome": "created", "exit": status}
except OSError as error:
    if error.errno != errno.EPERM:
        raise
    result = {"route": route, "outcome": "denied", "errno": error.errno}
temporary = Path("python-creation.tmp")
temporary.write_text(json.dumps(result), encoding="utf-8")
temporary.replace("python-creation.json")
