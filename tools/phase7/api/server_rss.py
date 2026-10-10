#!/usr/bin/env python3
"""The API server stand-in of the memory pass (tools/phase7/api/capture.py).

The pinned client spawns `getExePath()`; during the memory pass that path is
this script. It runs the server named by PHASE7_API_SERVER with the client's
arguments and stdio, forwards termination signals, and when the server ends
appends its own peak RSS in bytes to PHASE7_API_RSS_LOG.
"""
import os
import signal
import subprocess
import sys

server = subprocess.Popen([os.environ['PHASE7_API_SERVER'], *sys.argv[1:]])
for number in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
    signal.signal(number, lambda received, _frame: server.send_signal(received))
while True:
    try:
        _, status, usage = os.wait4(server.pid, 0)
        break
    except InterruptedError:
        continue
rss = usage.ru_maxrss * (1024 if sys.platform == 'linux' else 1)
with open(os.environ['PHASE7_API_RSS_LOG'], 'a') as log:
    log.write(f'{rss}\n')
sys.exit(os.waitstatus_to_exitcode(status))
