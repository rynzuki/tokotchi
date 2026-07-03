#!/bin/sh
# Live-reloading TUI dev loop.
#
# Run this in a real terminal window:
#   ./dev.sh
#
# Edit any src/*.rs and save — the running TUI exits cleanly (terminal restored) and
# relaunches with your changes recompiled. Press q to stop. No file-watcher tools needed:
# the binary watches its own source (via TOKOTCHI_DEV) and self-exits with code 69, which
# this loop treats as "recompile + relaunch".
cd "$(dirname "$0")" || exit 1
while :; do
  TOKOTCHI_DEV=1 cargo run -q
  case $? in
    0)  break ;;                                            # q → quit
    69) ;;                                                  # source changed → relaunch
    *)  printf '\n[dev] build failed — fix and save to retry…\n'; sleep 2 ;;
  esac
done
