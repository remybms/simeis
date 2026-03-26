#!/usr/bin/env bash

DIR=$(dirname $0)/../simeis-server/
cat << EOF | python
import os
import json

endpoints="""
$(rg -C 1 "web::get|web::post" $DIR)
"""

results = []
for chunk in endpoints.split("--"):
  chunk = chunk.strip()
  d = {}
  d["method"] = chunk.split("\n")[1].split("::")[1].split("(")[0].upper()
  d["doc"] = chunk.split("\n")[0].split("//")[1].strip()
  d["url"] = chunk.split("\n")[1].split("\"")[-2]
  d["name"] = chunk.split("\n")[2].split("fn ")[1].split("(")[0]
  results.append(d)

with open("doc.json", "w") as f:
  json.dump(results, f)
EOF
