#!/usr/bin/env python3
"""The server uses the same artwork as the client. See make_icon.py."""
import os
import runpy

runpy.run_path(os.path.join(os.path.dirname(__file__), "make_icon.py"), run_name="__main__")
