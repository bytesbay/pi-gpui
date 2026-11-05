#!/bin/sh
# Starts the DOOM plugin; sets up its Python environment on first run.
cd "$(dirname "$0")"
if [ ! -x .venv/bin/python ]; then
    python3 -m venv .venv >&2 && .venv/bin/pip -q install vizdoom pillow >&2 || exit 1
fi
exec .venv/bin/python doom.py
