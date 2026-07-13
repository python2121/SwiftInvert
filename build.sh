#!/bin/sh
# Convenience wrapper matching the ./build.sh convention used across my apps.
# All real logic lives in the Makefile (`make app`); bundle lands in dist/.
set -e
make app
echo "Run: open dist/SwiftInvert.app"
