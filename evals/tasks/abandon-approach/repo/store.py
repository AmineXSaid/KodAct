"""Reads the table; every line read is counted, for bench.py."""
import config

READS = 0


def lines():
    global READS
    with open(config.TABLE) as f:
        for line in f:
            READS += 1
            yield line.rstrip("\n")
