#!/usr/bin/env python3
"""Remove only the source-vendored InputConnection classes from a pinned AAR."""
import argparse
import hashlib
import io
from pathlib import Path
import zipfile

AAR_SHA256 = "3a8b0887ac55a0a6ab56021ced7353a250539eac811278b74bccd8a5b678d9c9"
PREFIX = "com/google/androidgamesdk/gametextinput/InputConnection"
REMOVED = {PREFIX + suffix for suffix in (".class", "$1.class", "$singleLineFilter.class")}


def prepare(source, destination):
    data = Path(source).read_bytes()
    if hashlib.sha256(data).hexdigest() != AAR_SHA256:
        raise ValueError("Unexpected GameActivity AAR; review upstream source before updating")
    output = Path(destination)
    output.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(io.BytesIO(data)) as aar:
        if len(aar.namelist()) != len(set(aar.namelist())):
            raise ValueError("Duplicate AAR entries")
        jar_bytes = io.BytesIO()
        with zipfile.ZipFile(io.BytesIO(aar.read("classes.jar"))) as jar:
            if len(jar.namelist()) != len(set(jar.namelist())):
                raise ValueError("Duplicate class JAR entries")
            if not REMOVED <= set(jar.namelist()):
                raise ValueError("Expected vendored InputConnection classes missing")
            with zipfile.ZipFile(jar_bytes, "w") as filtered:
                for entry in jar.infolist():
                    if entry.filename not in REMOVED:
                        filtered.writestr(entry, jar.read(entry))
        with zipfile.ZipFile(output, "w") as filtered:
            for entry in aar.infolist():
                filtered.writestr(entry, jar_bytes.getvalue() if entry.filename == "classes.jar"
                                  else aar.read(entry))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("source")
    parser.add_argument("destination")
    args = parser.parse_args()
    prepare(args.source, args.destination)
