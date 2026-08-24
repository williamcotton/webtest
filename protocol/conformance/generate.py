#!/usr/bin/env python3
"""Generate and verify checked-in protocol-1 wire projections."""

from pathlib import Path
import argparse
import json

ROOT = Path(__file__).resolve().parents[1]
REPOSITORY = ROOT.parent
FILES = {
    ROOT / "templates" / "typescript-wire-types.ts": ROOT / "generated" / "typescript-wire-types.ts",
    ROOT / "templates" / "ruby-wire-types.rb": ROOT / "generated" / "ruby-wire-types.rb",
    ROOT / "templates" / "rust-wire.rs": REPOSITORY / "crates" / "app-bridge" / "src" / "wire.rs",
}


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    json.loads((ROOT / "schema.json").read_text())
    json.loads((ROOT / "types.json").read_text())
    dirty = []
    for template, target in FILES.items():
        expected = template.read_bytes()
        if args.check:
            if not target.exists() or target.read_bytes() != expected:
                dirty.append(str(target.relative_to(REPOSITORY)))
        else:
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(expected)
    if dirty:
        raise SystemExit("generated protocol projections are stale: " + ", ".join(dirty))
    print("protocol generated projections are current")


if __name__ == "__main__":
    main()
