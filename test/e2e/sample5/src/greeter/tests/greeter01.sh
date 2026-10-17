#!/bin/sh
# Greeter tests: prints the standard iter5 result JSON as the last stdout line.
here=$(cd "$(dirname "$0")" && pwd)
pass=0; err=0; total=2; d=""
out=$(sh "$here/../cli/hello.sh")
if [ "$out" = "Hello, World" ]; then pass=$((pass+1)); d="$d{\"name\":\"default\",\"bucket\":\"normal\",\"pass\":true,\"msg\":\"\"},"; else d="$d{\"name\":\"default\",\"bucket\":\"normal\",\"pass\":false,\"msg\":\"got $out\"},"; fi
out=$(sh "$here/../cli/hello.sh" Ada)
if [ "$out" = "Hello, Ada" ]; then pass=$((pass+1)); d="$d{\"name\":\"named\",\"bucket\":\"normal\",\"pass\":true,\"msg\":\"\"}"; else d="$d{\"name\":\"named\",\"bucket\":\"normal\",\"pass\":false,\"msg\":\"got $out\"}"; fi
ok=false; [ "$pass" = "$total" ] && ok=true
echo "ran $total greeter checks"
echo "{\"name\":\"Greeter tests\",\"id\":\"\",\"overall_success\":$ok,\"normal\":{\"total\":$total,\"pass\":$pass,\"err\":$err},\"longtail\":{\"total\":0,\"pass\":0,\"err\":0},\"failure\":{\"total\":0,\"pass\":0,\"err\":0},\"details\":[$d]}"
[ "$ok" = true ]
