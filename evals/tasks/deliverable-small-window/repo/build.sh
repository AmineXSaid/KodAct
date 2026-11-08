#!/bin/sh
# Builds the regional sales report: four stages, each tracing every record.
exec python3 pipeline/runner.py "$@"
