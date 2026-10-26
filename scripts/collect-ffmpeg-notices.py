"""Offline evidence collection; never executes upstream code or approves distribution.

Reads the three pinned source ZIPs, preserves them byte-for-byte, inventories
the dependency cache, and collects verbatim notice files under content hashes.
Use a NEW output directory; only the Python standard library is required.
"""
import argparse
import hashlib
import io
import json
import pathlib
import re
import shutil
import tarfile
import zipfile

MAX_NOTICE = 4 * 1024 * 1024
MAX_NOTICE_TOTAL = 256 * 1024 * 1024
MAX_EXPANDED_ARCHIVE = 32 * 1024**3


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def safe_parts(name):
    if "\\" in name or ":" in name or "\x00" in name:
        raise ValueError(f"Invalid archive name: {name!r}")
    path = pathlib.PurePosixPath(name)
    if path.is_absolute() or ".." in path.parts:
        raise ValueError(f"Invalid archive path: {name!r}")
    return path.parts


def is_notice(name):
    # Source names stay metadata only; Linux trees can contain ':' legitimately.
    # None of these names are materialized on disk (notice output uses SHA-256).
    parts = pathlib.PurePosixPath(name).parts
    if ".git" in parts or ".svn" in parts:
        return False
    base = parts[-1].lower() if parts else ""
    return bool(re.match(r"^(licen[cs]e|copying|copyright|notice|authors|patents|unlicense)([._-]|$)", base)
                or any(part.lower() in ("licenses", "licences") for part in parts[:-1]))


def write_json(path, data):
    path.write_text(json.dumps(data, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def write_notice_index(root, rows):
    lines = ["# Source notice index", "",
        "Verbatim source notices and attribution files from ALL cached build variants.",
        "This includes tests and unused code; inclusion here does not mean a license applies to the shipped binary.",
        "See inventory/notices.json for source archive names and SHA-256 values.", ""]
    for component in sorted({row["component"] for row in rows}):
        lines.extend(["## " + component, ""])
        for row in rows:
            if row["component"] == component:
                label = row["original_path"].replace("[", "\\[").replace("]", "\\]")
                lines.append(f"- [{label}]({row['file']})")
        lines.append("")
    (root / "NOTICE-INDEX.md").write_text("\n".join(lines), encoding="utf-8")


class Notices:
    def __init__(self, root):
        self.root = root
        self.rows = []
        self.total = 0

    def add(self, component, archive, name, data):
        self.total += len(data)
        if len(data) > MAX_NOTICE or self.total > MAX_NOTICE_TOTAL:
            raise ValueError("Notice byte limit exceeded")
        sha = hashlib.sha256(data).hexdigest()
        dest = self.root / "licenses" / (sha + ".txt")
        if not dest.exists():
            dest.write_bytes(data)
        self.rows.append(dict(component=component, archive=archive, original_path=name,
                              bytes=len(data), sha256=sha, file="licenses/" + dest.name))


def scan_tar(path, stage, notices):
    heads, nested, skipped, count, expanded, vendors = [], [], [], 0, 0, set()
    with tarfile.open(path, "r|xz") as archive:
        for member in archive:
            count += 1
            if count > 1_000_000:
                raise ValueError("Too many source entries")
            # Source trees may contain normal symbolic links. Never extract them.
            parts = pathlib.PurePosixPath(member.name).parts
            if member.name.startswith('/') or '..' in parts or '\x00' in member.name:
                raise ValueError("Invalid source member path")
            expanded += member.size
            if expanded > MAX_EXPANDED_ARCHIVE:
                raise ValueError("Expanded source byte limit exceeded")
            if "vendor" in parts:
                vendors.add("/".join(parts[:parts.index("vendor") + 1]))
            if not member.isfile():
                continue
            if member.name.endswith((".tar.gz", ".tar.xz", ".tar.bz2", ".zip", ".crate")):
                nested.append(member.name)
            if parts[-2:] == (".git", "HEAD") and member.size < 1024:
                heads.append(dict(path=member.name, value=archive.extractfile(member).read().decode("ascii", "replace").strip()))
            # nv-codec-headers puts its copyright/permission terms in headers,
            # rather than a standalone LICENSE file. Preserve those whole files.
            header_notice = stage == "50-ffnvcodec" and "/include/ffnvcodec/" in member.name and member.name.endswith(".h")
            if is_notice(member.name) or header_notice:
                if member.size > MAX_NOTICE:
                    skipped.append(member.name)
                else:
                    notices.add(stage, path.name, member.name, archive.extractfile(member).read())
            # GLib's download recipe embeds this source package. Other nested
            # archives in the pinned corpus are inventoried, never auto-executed.
            if stage == "10-glib" and member.name == "./subprojects/packagecache/proxy-libintl-0.5.tar.gz":
                if member.size > 32 * 1024 * 1024:
                    raise ValueError("Nested source package too large")
                data = archive.extractfile(member).read()
                with tarfile.open(fileobj=io.BytesIO(data), mode="r|gz") as nested_source:
                    nested_bytes = 0
                    for child in nested_source:
                        nested_bytes += child.size
                        if nested_bytes > 64 * 1024 * 1024:
                            raise ValueError("Nested source expansion too large")
                        if child.isfile() and is_notice(child.name):
                            if child.size > MAX_NOTICE:
                                skipped.append(member.name + "!" + child.name)
                            else:
                                notices.add(stage, path.name, member.name + "!" + child.name, nested_source.extractfile(child).read())
    return dict(entries=count, expanded_bytes=expanded, git_heads=heads,
                nested_archives=nested, vendor_directories=sorted(vendors), oversized_notices=skipped)


def collect(cache, output, lock_path, runtime):
    pin = json.loads(lock_path.read_text(encoding="utf-8-sig"))
    # This collector is for the historical upstream binary. Do not combine its
    # source lock with configuration reports from the new local audio build.
    if "binary" in pin:
        runtime_record = json.loads((runtime / "bundle-provenance.json").read_text(encoding="utf-8-sig"))
        if runtime_record.get("binary_archive_sha256") != pin["binary"]["sha256"]:
            raise ValueError("Runtime does not match this historical source lock; use the preserved matching runtime backup")
    assets = {name: pin[name] for name in ("ffmpeg_source", "builder_source", "dependency_sources")}
    # Verify all input bytes BEFORE creating any deliverable.
    for asset in assets.values():
        source = cache / asset["name"]
        if digest(source) != asset["sha256"]:
            raise ValueError("Pinned archive SHA-256 mismatch: " + source.name)
    if output.exists():
        raise FileExistsError("Choose a new output directory; no existing material is overwritten")
    output.mkdir(parents=True)
    for folder in ("sources", "licenses", "inventory", "recipes"):
        (output / folder).mkdir()
    notices = Notices(output)
    source_rows, recipes, skipped_zip_notices = [], {}, []
    for component, asset in assets.items():
        source = cache / asset["name"]
        target = output / "sources" / source.name
        shutil.copyfile(source, target)
        if digest(target) != asset["sha256"]:
            raise ValueError("Copied source changed: " + target.name)
        source_rows.append(dict(component=component, file="sources/" + source.name,
                                sha256=asset["sha256"], bytes=target.stat().st_size))
        if component == "dependency_sources":
            continue
        with zipfile.ZipFile(target) as archive:
            for entry in archive.infolist():
                parts = safe_parts(entry.filename)
                if entry.is_dir():
                    continue
                if is_notice(entry.filename):
                    if entry.file_size <= MAX_NOTICE:
                        notices.add(component, target.name, entry.filename, archive.read(entry))
                    else:
                        skipped_zip_notices.append(dict(archive=target.name, path=entry.filename, bytes=entry.file_size))
                if component == "builder_source" and len(parts) > 2 and parts[1] == "scripts.d" and entry.filename.endswith(".sh"):
                    text = archive.read(entry).decode("utf-8")
                    stage = pathlib.PurePosixPath(entry.filename).stem
                    variables = dict(re.findall(r'^([A-Z_]+)="([^"\n]*)"', text, re.M))
                    configure = re.search(r'^ffbuild_configure\(\) \{\n(.*?)^\}', text, re.M | re.S)
                    recipes.setdefault(stage, []).append(dict(file="/".join(parts[1:]), variables=variables,
                        enable_flags=sorted(set(re.findall(r"--enable-[a-zA-Z0-9_-]+", configure.group(1) if configure else "")))))
                    with (output / "recipes" / "__".join(parts[1:])).open('xb') as dst:
                        dst.write(archive.read(entry))
    shutil.copyfile(lock_path, output / "upstream-lock.json")
    for name in ("build-configuration.txt", "license-report.txt", "bundle-provenance.json"):
        shutil.copyfile(runtime / name, output / name)
    buildconf = (runtime / "build-configuration.txt").read_text(encoding="utf-8-sig")
    actual_flags = set(re.findall(r"--enable-[a-zA-Z0-9_-]+", buildconf))
    # Do not unpack the TAR tree. Materialize only flat, regular compressed source
    # archives into a single scratch file that is deleted after each scan.
    outer_rows, components = [], []
    with zipfile.ZipFile(output / "sources" / assets["dependency_sources"]["name"]) as archive:
        entries = archive.infolist()
        if len(entries) != 1 or entries[0].filename != "cache.tar.gz":
            raise ValueError("Unexpected source cache ZIP structure")
        with archive.open(entries[0]) as compressed, tarfile.open(fileobj=compressed, mode="r|gz") as outer:
            for member in outer:
                parts = safe_parts(member.name)
                if len(parts) != 3 or parts[:2] != (".cache", "downloads"):
                    raise ValueError("Unexpected download cache member")
                row = dict(path=member.name, bytes=member.size, type=member.type.decode("ascii"), link_target=member.linkname)
                if member.issym():
                    target_parts = safe_parts(member.linkname)
                    if len(target_parts) != 1:
                        raise ValueError("Unexpected cache alias target")
                elif member.isfile():
                    match = re.fullmatch(r"([\w-]+)_([0-9a-f]{64})\.tar\.xz", parts[-1])
                    if not match or member.size > 4 * 1024**3:
                        raise ValueError("Unexpected source archive name/size")
                    stage, recipe_hash = match.groups()
                    scratch = output / "source-scan.tmp.tar.xz"
                    with scratch.open("xb") as dst, outer.extractfile(member) as src:
                        shutil.copyfileobj(src, dst, 1024 * 1024)
                    try:
                        row["sha256"] = digest(scratch)
                        print("Scanning " + stage, flush=True)
                        start = len(notices.rows)
                        details = scan_tar(scratch, stage, notices)
                        for notice in notices.rows[start:]:
                            notice["archive"] = parts[-1]
                        candidates = recipes.get(stage, [])
                        expected_refs = {recipe["variables"].get("SCRIPT_COMMIT") for recipe in candidates}
                        expected = next(iter(expected_refs)) if len(expected_refs) == 1 else None
                        flags = {flag for recipe in candidates for flag in recipe["enable_flags"]}
                        heads = [head["value"] for head in details["git_heads"]]
                        components.append(dict(stage=stage, cache_entry=member.name, source_sha256=row["sha256"],
                            download_recipe_hash=recipe_hash, recipes=candidates,
                            observed_enable_flags=sorted(actual_flags.intersection(flags)),
                            expected_commit_present_in_git_heads=expected in heads if expected and re.fullmatch(r"[0-9a-f]{40}", expected) else None,
                            notice_count=len(notices.rows)-start, **details))
                    finally:
                        scratch.unlink()
                else:
                    raise ValueError("Unexpected cache entry type")
                outer_rows.append(row)
    regular = {pathlib.PurePosixPath(row["path"]).name for row in outer_rows if row["type"] == "0"}
    for row in outer_rows:
        if row["type"] == "2" and row["link_target"] not in regular:
            raise ValueError("Dangling cache alias")
    write_json(output / "inventory/sources.json", source_rows)
    write_json(output / "inventory/cache.json", outer_rows)
    write_json(output / "inventory/components.json", components)
    write_json(output / "inventory/notices.json", notices.rows)
    write_notice_index(output, notices.rows)
    write_json(output / "inventory/oversized-zip-notices.json", skipped_zip_notices)
    manifest = dict(schema=1, collection_complete=True, redistribution_ready=False,
        source_archives=len(source_rows), dependency_archives=len(components), notice_files=len(notices.rows),
        unique_notice_contents=len(list((output / "licenses").iterdir())),
        limitations=["Filename hashes describe upstream download recipes, not source bytes; source_sha256 is independently calculated.",
            "Enable flag matches are evidence, not a complete binary SBOM. All cached variants are retained.",
            "Notice collection uses filenames and preserves original bytes; it does not approve legal compatibility.",
            "Nested archives, toolchain/runtime dependencies and build-time network downloads require separate correspondence review."])
    write_json(output / "collection.json", manifest)
    lines = ["# FFmpeg source and notice collection", "", "Review material; NOT a cleared release package.", "",
        "Original source ZIPs are preserved under sources/. licenses/ stores verbatim content by SHA-256;",
        "inventory/notices.json maps every file back to its component, source archive and original path.",
        "Upstream build scripts, patches and toolchain settings remain in the builder ZIP. Nothing was executed.", "",
        "| Cache component | Observed enable flags | Expected commit present in Git HEADs | Notices |", "| --- | --- | --- | --- |"]
    for c in components:
        lines.append(f"| {c['stage']} | {', '.join(c['observed_enable_flags']) or 'not a direct flag match'} | {c['expected_commit_present_in_git_heads']} | {c['notice_count']} |")
    (output / "README.md").write_text("\n".join(lines)+"\n", encoding="utf-8")
    print(json.dumps(manifest, indent=2))


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    repository = pathlib.Path(__file__).resolve().parent.parent
    parser.add_argument("--cache", type=pathlib.Path, default=repository / "build/ffmpeg-cache")
    parser.add_argument("--runtime", type=pathlib.Path, default=repository / "apps/desktop/src-tauri/resources/ffmpeg")
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()
    collect(args.cache.resolve(), args.output.resolve(), repository / "scripts/ffmpeg-bundle.lock.json", args.runtime.resolve())
