#!/usr/bin/env python3
"""Deprecated command spelling; pairing belongs exclusively to the product CLI."""
import os
import sys
os.execv('/opt/xscc/xscc', ['xscc', 'pair', '--interactive', *sys.argv[1:]])
