#!/usr/bin/env bash

if [ $# -ne 1 ]; then
    echo "Usage: $0 <num players>"
    exit 1
fi
NPLAYERS=$1
mkdir -p bigtest/logs
cd bigtest
for i in $(seq 1 $NPLAYERS); do
    python3 ../python/client.py "player$i" 0.0.0.0 8080 1>"./logs/$i.out" 2>"./logs/$i.err" &
    echo "Started player $i"
    #../rust/target/release/simeis-rust-example "player$i" 0.0.0.0 8080 1>"./logs/$i.out" 2>"./logs/$i.err" &
done

python3 ../watch_game.py
kill $(jobs -p)
