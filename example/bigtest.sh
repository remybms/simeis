#!/usr/bin/env bash

if [ $# -ne 1 ]; then
    echo "Usage: $0 <num players>"
    exit 1
fi
NPLAYERS=$1
mkdir -p bigtest/logs
cd bigtest
for i in $(seq 1 $NPLAYERS); do
    python -c "import time; time.sleep($i / 5000)" && python3 ../python/client.py "player$i" 0.0.0.0 8080 1>"./logs/$i.out" 2>"./logs/$i.err" &
    echo "Started player $i"
done

python3 ../watch_game.py
kill $(jobs -p)
