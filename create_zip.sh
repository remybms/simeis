#!/usr/bin/env bash
nix build
cp ./result/manual.pdf ./
rm -f ./simeis.zip
zip -9 -r simeis.zip ./Cargo.* ./manual.pdf ./doc ./simeis-server ./simeis-data ./.gitignore ./example/client.py
