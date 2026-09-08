#!/usr/bin/env python3
"""Deprecated command spelling; pairing belongs exclusively to the product CLI."""
import os
import sys
os.execv('/opt/sunshine-client/sunshine-client', ['sunshine-client', 'pair', '--interactive', *sys.argv[1:]])
