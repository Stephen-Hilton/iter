#!/bin/sh
here=$(cd "$(dirname "$0")" && pwd)
sh "$here/../lib/greet.sh" "$@"
