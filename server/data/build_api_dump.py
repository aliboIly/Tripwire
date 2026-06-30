#!/usr/bin/env python3
# Regenerates api-dump.min.json, the trimmed Roblox class reflection that the
# server embeds for get_class_info. Source is Roblox's own published API dump,
# mirrored by MaximumADHD/Roblox-Client-Tracker. We keep only what an agent needs
# to reason about a class (members, their kind, type, params, security, tags) and
# drop descriptions and engine-internal metadata, which cuts the file by more than
# half. The committed output is what ships; this script just refreshes it.
#
# Usage:  python3 server/data/build_api_dump.py
#
# It writes server/data/api-dump.min.json next to this script.

import json
import os
import urllib.request

SOURCE = "https://raw.githubusercontent.com/MaximumADHD/Roblox-Client-Tracker/roblox/Mini-API-Dump.json"
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "api-dump.min.json")


def value_type(vt):
    # ValueType / ReturnType is {Category, Name}; the Name is what a script writer sees.
    if not isinstance(vt, dict):
        return None
    return vt.get("Name")


def trim_member(m):
    kind = m.get("MemberType")
    out = {"name": m.get("Name"), "kind": kind}

    if kind == "Property":
        t = value_type(m.get("ValueType"))
        if t:
            out["type"] = t
    elif kind in ("Function", "Callback"):
        t = value_type(m.get("ReturnType"))
        if t:
            out["type"] = t
        params = []
        for p in m.get("Parameters", []) or []:
            param = {"name": p.get("Name"), "type": value_type(p.get("Type"))}
            params.append(param)
        out["params"] = params

    tags = m.get("Tags")
    if tags:
        out["tags"] = tags

    # Security is "None" for ordinary scriptable members; surface it only when it
    # actually restricts access, so the agent knows a member is plugin/roblox-only.
    # Normalized to a single string so the server parses one shape for every member.
    sec = m.get("Security")
    if isinstance(sec, dict):
        read, write = sec.get("Read", "None"), sec.get("Write", "None")
        if read == write and read != "None":
            out["security"] = read
        elif read != "None" or write != "None":
            out["security"] = f"read={read} write={write}"
    elif isinstance(sec, str) and sec != "None":
        out["security"] = sec

    return out


def main():
    with urllib.request.urlopen(SOURCE) as r:
        dump = json.load(r)

    classes = {}
    for c in dump.get("Classes", []):
        name = c.get("Name")
        if not name:
            continue
        members = [trim_member(m) for m in c.get("Members", []) or []]
        # Stable order so the diff is small when only a few members change.
        members.sort(key=lambda m: (m["kind"], m["name"]))
        classes[name] = {
            "superclass": c.get("Superclass"),
            "members": members,
        }

    out = {
        "apiDumpFormatVersion": dump.get("Version"),
        "source": SOURCE,
        "classes": classes,
    }
    with open(OUT, "w", encoding="utf-8") as f:
        json.dump(out, f, separators=(",", ":"), sort_keys=True)
        f.write("\n")

    size = os.path.getsize(OUT)
    print(f"wrote {OUT} ({len(classes)} classes, {size} bytes)")


if __name__ == "__main__":
    main()
