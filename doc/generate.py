#-*-encoding:utf-8*-

ALL_METHODS = ["get", "post"]

import os
import json

with open("../simeis-server/src/api.rs", "r") as f:
    code = f.read().split("\n")

results = []
path = None
method = None

comments = []
for line in code:
    if line.startswith("//"):
        if "TODO" in line:
            continue
        comments.append(line[2:].strip())
    elif any([f"web::{method}" in line for method in ALL_METHODS]):
        path = line.split("(\"")[1].split("\")")[0]
        method = line.split("web::")[1].split("(")[0].upper()
    else:
        if path is not None:
            d = {}
            d["name"] = line.split("fn ")[1].split("(")[0]
            d["doc"] = "\n".join(comments)
            d["url"] = path
            d["method"] = method
            results.append(d)
            comments = []
            path = None
            method = None

with open("doc.json", "w") as f:
  json.dump(results, f)
