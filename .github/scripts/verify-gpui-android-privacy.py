#!/usr/bin/env python3
"""Fail closed on packaged DEX logs or changed GameActivity JNI declarations."""
import argparse
import hashlib
import io
import struct
import zipfile

PREFIX = "Lcom/google/androidgamesdk/"
SENSITIVE_FORMATS = (
    "setState: '", "commitText: ", "getSelectedText: ",
    "setComposingText=", "processKeyEvent(key=", "processKeyEvent: exit",
)


def uleb(data, offset):
    value = 0
    for shift in range(0, 35, 7):
        byte = data[offset]
        offset += 1
        value |= (byte & 127) << shift
        if byte < 128:
            return value, offset
    raise ValueError("Invalid ULEB128")


def dex_symbols(data):
    if data[:4] != b"dex\n" or data[7] != 0:
        raise ValueError("Unsupported DEX")
    def table(header, width):
        count, offset = struct.unpack_from("<II", data, header)
        if offset + count * width > len(data):
            raise ValueError("Invalid DEX table")
        return [offset + i * width for i in range(count)]
    strings = []
    for offset in table(56, 4):
        location = struct.unpack_from("<I", data, offset)[0]
        _, start = uleb(data, location)
        strings.append(data[start:data.index(0, start)].decode("utf-8", errors="replace"))
    types = [strings[struct.unpack_from("<I", data, o)[0]] for o in table(64, 4)]
    prototypes = []
    for offset in table(72, 12):
        _, result, parameters = struct.unpack_from("<III", data, offset)
        args = ""
        if parameters:
            count = struct.unpack_from("<I", data, parameters)[0]
            args = "".join(types[struct.unpack_from("<H", data, parameters + 4 + i * 2)[0]]
                           for i in range(count))
        prototypes.append("(" + args + ")" + types[result])
    fields = []
    for offset in table(80, 8):
        owner, kind, name = struct.unpack_from("<HHI", data, offset)
        fields.append((types[owner], strings[name], types[kind]))
    methods = []
    for offset in table(88, 8):
        owner, proto, name = struct.unpack_from("<HHI", data, offset)
        methods.append((types[owner], strings[name], prototypes[proto]))
    definitions = set()
    classes = set()
    for offset in table(96, 32):
        owner = types[struct.unpack_from("<I", data, offset)[0]]
        classes.add(owner)
        location = struct.unpack_from("<I", data, offset + 24)[0]
        if not location:
            continue
        counts = []
        for _ in range(4):
            count, location = uleb(data, location)
            counts.append(count)
        for group, count in enumerate(counts):
            index = 0
            for _ in range(count):
                delta, location = uleb(data, location)
                index += delta
                _, location = uleb(data, location)  # access flags
                if group >= 2:
                    _, location = uleb(data, location)  # code offset
                definitions.add((methods if group >= 2 else fields)[index])
    return strings, methods, classes, definitions


JAVA8_ACCESS_MARKER = PREFIX + "gametextinput/InputConnection$1;"
JAVA8_ACCESS_CTOR = (
    PREFIX + "gametextinput/InputConnection$singleLineFilter;", "<init>",
    "(" + PREFIX + "gametextinput/InputConnection;" + JAVA8_ACCESS_MARKER + ")V",
)


def normalize_java8_access(owner, class_flags, major, members):
    # The pinned AAR's javac-8 empty access marker and forwarding constructor
    # are replaced by a different synthetic bridge when javac-17/D8 compiles
    # our source. Never exclude other synthetic fields/methods or real JNI ABI.
    if major == 52 and owner == JAVA8_ACCESS_MARKER and class_flags & 0x1000 and not members:
        return None, set()
    required = {member for member, flags in members
                if not (major == 52 and member == JAVA8_ACCESS_CTOR and flags & 0x1000)}
    return owner, required


def class_symbols(data, normalize_access=False):
    stream = io.BytesIO(data)
    def number(size=2):
        return int.from_bytes(stream.read(size), "big")
    if number(4) != 0xCAFEBABE:
        raise ValueError("Invalid Java class")
    number()  # minor version
    major = number()
    pool = [None] * number()
    index = 1
    while index < len(pool):
        tag = number(1)
        if tag == 1:
            pool[index] = stream.read(number()).decode("utf-8", errors="replace")
        elif tag in (7, 8, 16, 19, 20):
            pool[index] = number()
        elif tag in (3, 4, 9, 10, 11, 12, 17, 18):
            stream.read(4)
        elif tag in (5, 6):
            stream.read(8)
            index += 1
        elif tag == 15:
            stream.read(3)
        else:
            raise ValueError("Unsupported constant pool tag")
        index += 1
    class_flags = number()
    owner = "L" + pool[pool[number()]] + ";"
    number()
    stream.read(number() * 2)
    members = []
    for _ in range(2):  # fields, methods
        for _ in range(number()):
            flags = number()
            name, descriptor = pool[number()], pool[number()]
            if name != "<clinit>":  # Pure static initialization can be folded.
                members.append(((owner, name, descriptor), flags))
            for _ in range(number()):
                number()
                stream.read(number(4))
    if normalize_access:
        shape_hashes = {
            JAVA8_ACCESS_MARKER: "50453e540dac650a67e4a58ed6c755f3a6f4e3adfba81a9537e9563bc489432b",
            JAVA8_ACCESS_CTOR[0]: "82a9b2f8b64b65086730ace41c909c7921878168bc115285926e0c7949e8d6c2",
        }
        # Pin the inspected empty-marker / six-byte forwarding-code shapes,
        # not merely a name or ACC_SYNTHETIC claim in an arbitrary dependency.
        if owner in shape_hashes and hashlib.sha256(data).hexdigest() != shape_hashes[owner]:
            raise ValueError("Unreviewed javac access bridge shape")
        return normalize_java8_access(owner, class_flags, major, members)
    return owner, {member for member, _ in members}


def check_privacy(strings, methods):
    for owner, name, _ in methods:
        if owner == "Landroid/util/Log;" and name in ("d", "v"):
            raise ValueError("Packaged DEX still references Log." + name)
    for value in strings:
        if any(marker in value for marker in SENSITIVE_FORMATS):
            raise ValueError("Sensitive log-format string remains after R8 (code-reference investigation required): " + repr(value))


def check_jni(classes, definitions):
    bridge = "Lorg/xscc/gpui/AndroidImeBridge;"
    required_classes = {bridge, bridge[:-1] + "$ImeSurfaceView;",
                        bridge[:-1] + "$SessionConnection;"}
    required = {
        (bridge, "configureIme", "(JIIILjava/lang/String;IIIIZ)V"),
        (bridge, "hideIme", "(J)V"),
        (bridge, "nativeTextChanged", "(JLjava/lang/String;IIII)V"),
        (bridge, "nativeAction", "(JI)V"),
    }
    required.update(("Landroidx/core/view/WindowInsetsCompat$Type;", name, "()I")
                    for name in ("systemBars", "displayCutout", "ime"))
    required.update(("Landroidx/core/graphics/Insets;", name, "I")
                    for name in ("left", "top", "right", "bottom"))
    if required_classes - classes or required - definitions:
        raise ValueError("Required Android IME/insets JNI declarations missing: " +
                         repr(sorted(required_classes - classes)) + " " +
                         repr(sorted(required - definitions)))


def check_playback_jni(classes, definitions):
    bridge = "Lorg/xscc/gpui/AndroidPlaybackAudio;"
    required = {
        (bridge, "<init>", "(Landroid/content/Context;J)V"),
        (bridge, "activate", "()Z"),
        (bridge, "close", "()V"),
        (bridge, "nativeStopped", "(JI)V"),
    }
    if bridge not in classes or required - definitions:
        raise ValueError("Required Android playback JNI declarations missing: " +
                         repr(sorted(required - definitions)))


def check_game_abi(expected_classes, expected_members, classes, definitions):
    if expected_classes - classes or expected_members - definitions:
        raise ValueError("GameActivity JNI classes/members removed or renamed: " +
                         repr(sorted((expected_classes - classes))[:3]) + " " +
                         repr(sorted(expected_members - definitions)[:3]))


def verify(apk, aar, management=False):
    classes, definitions = set(), set()
    dex_count = 0
    with zipfile.ZipFile(apk) as archive:
        for name in archive.namelist():
            if name.startswith("classes") and name.endswith(".dex"):
                dex_count += 1
                strings, methods, found_classes, found_definitions = dex_symbols(archive.read(name))
                check_privacy(strings, methods)
                classes.update(found_classes)
                definitions.update(found_definitions)
    if not dex_count:
        raise ValueError("APK has no DEX")
    with zipfile.ZipFile(aar) as library:
        with zipfile.ZipFile(io.BytesIO(library.read("classes.jar"))) as jar:
            expected_classes, expected_members = set(), set()
            for name in jar.namelist():
                if name.startswith(PREFIX[1:]) and name.endswith(".class"):
                    owner, members = class_symbols(jar.read(name), normalize_access=True)
                    if owner is None:
                        continue
                    expected_classes.add(owner)
                    expected_members.update(members)
    if not expected_classes or not expected_members:
        raise ValueError("GameActivity reference AAR is empty")
    check_game_abi(expected_classes, expected_members, classes, definitions)
    check_jni(classes, definitions)
    if management:
        check_playback_jni(classes, definitions)
    return len(expected_classes), len(expected_members)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("apk")
    parser.add_argument("aar", help="Unmodified Maven games-activity 4.4.0 AAR")
    parser.add_argument("--management", action="store_true", help="Require playback JNI declarations in the management APK")
    args = parser.parse_args()
    classes, members = verify(args.apk, args.aar, management=args.management)
    print(f"DEX privacy gate passed; preserved {classes} GameActivity classes and {members} members")
