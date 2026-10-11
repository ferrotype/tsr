#!/usr/bin/env python3
"""The API server stand-in of the memory pass (tools/phase7/api/capture.py).

The pinned client spawns `getExePath()`; during the memory pass that path is
this script, so it has to stay executable. It runs the server named by
PHASE7_API_SERVER with the client's arguments and stdio, forwards termination
signals, and when the server ends appends its own peak RSS in bytes to
PHASE7_API_RSS_LOG.
"""
import os
import signal
import subprocess
import sys

server = subprocess.Popen([os.environ['PHASE7_API_SERVER'], *sys.argv[1:]])


def forward(received, _frame):
    # os.kill, not Popen.send_signal: send_signal polls, and a poll that reaps
    # the server leaves the wait4 below without a child.
    try:
        os.kill(server.pid, received)
    except ProcessLookupError:
        pass


for number in (signal.SIGTERM, signal.SIGINT, signal.SIGHUP):
    signal.signal(number, forward)
_, status, usage = os.wait4(server.pid, 0)
rss = usage.ru_maxrss * (1024 if sys.platform == 'linux' else 1)
with open(os.environ['PHASE7_API_RSS_LOG'], 'a') as log:
    log.write(f'{rss}\n')
sys.exit(os.waitstatus_to_exitcode(status))
