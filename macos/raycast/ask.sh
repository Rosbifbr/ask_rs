#!/bin/bash

# Required parameters:
# @raycast.schemaVersion 1
# @raycast.title Ask
# @raycast.mode silent
# @raycast.packageName Ask
#
# Optional parameters:
# @raycast.icon 💬
# @raycast.argument1 { "type": "text", "placeholder": "Prompt" }
# @raycast.description Send a prompt to the Ask HUD

set -eu

encoded=$(python3 -c 'import urllib.parse,sys; print(urllib.parse.quote(sys.argv[1], safe=""))' "$1")
open "ask://q?text=${encoded}"
