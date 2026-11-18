"""Reproduce bundled starters from their portable checked-in sources."""

import argparse
import hashlib
import io
import json
from pathlib import Path
import tarfile

import yaml
from bundle_paths import archive_path, read_bundle_files, registry_prompt_paths

REPO = Path(__file__).resolve().parents[2]
SOURCE = REPO / "flows/starters/text-summary"
ASSET = REPO / "apps/android/app/src/main/assets/flows/text-summary.tar"
FILES = ("text-summary.flow.yaml", "text-summary.tasks.yaml", "setup.json", "prompts/minimal-summary.md")
BROWSER_SOURCE = REPO / "flows/starters/browser-summary"
BROWSER_ASSET = ASSET.with_name("browser-summary.tar")
BROWSER_FILES = ("browser-summary.flow.yaml", "browser-summary.tasks.yaml", "setup.json",
                 "prompts/open-page.md", "prompts/read-page.md", "prompts/summarize-page.md")
CHECK_TOTAL_SOURCE = REPO / "flows/starters/check-total"
CHECK_TOTAL_ASSET = ASSET.with_name("check-total.tar")
CHECK_TOTAL_FILES = ("check-total.flow.yaml", "check-total.tasks.yaml", "setup.json",
                     "prompts/sum.md", "prompts/advise.md", "prompts/report.md")
STARTERS = {"text-summary": (SOURCE, ASSET, FILES),
            "browser-summary": (BROWSER_SOURCE, BROWSER_ASSET, BROWSER_FILES),
            "check-total": (CHECK_TOTAL_SOURCE, CHECK_TOTAL_ASSET, CHECK_TOTAL_FILES)}


def build_bundle(source=SOURCE, *, starter="text-summary"):
    names = STARTERS[starter][2]
    if source == SOURCE and starter != "text-summary":
        source = STARTERS[starter][0]
    # Keep source bytes unchanged, but select prompt payloads from actual model
    # tasks. Explicit direct tasks need no placeholder prompt file.
    files = read_bundle_files(source, [name for name in names
                                      if not archive_path(name).startswith("prompts/")])
    registry_path = f"{starter}.tasks.yaml"
    registry = yaml.safe_load(files[registry_path])
    prompts = registry_prompt_paths(registry, registry_path)
    if set(prompts) & set(files):
        raise ValueError("Prompt path collides with a bundle source file")
    files.update(read_bundle_files(source, prompts))
    flow = yaml.safe_load(files[f"{starter}.flow.yaml"])
    pin = flow["setup"]
    if pin["path"] != "setup.json" or pin["sha256"] != hashlib.sha256(files["setup.json"]).hexdigest():
        raise ValueError("Setup changed: update the checked-in root flow's setup SHA-256 pin.")
    manifest = {
        "format": "trollm.flow-bundle.v1", "entryFlow": f"{starter}.flow.yaml",
        "taskRegistry": f"{starter}.tasks.yaml",
        "files": [{"path": path, "sha256": hashlib.sha256(data).hexdigest()}
                  for path, data in sorted(files.items())],
    }
    files["manifest.json"] = (json.dumps(manifest, indent=2, sort_keys=True) + "\n").encode("utf-8")
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode="w", format=tarfile.USTAR_FORMAT) as archive:
        for path, data in sorted(files.items()):
            member = tarfile.TarInfo(path)
            member.size, member.mode = len(data), 0o644
            archive.addfile(member, io.BytesIO(data))
    return output.getvalue()


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--starter", choices=STARTERS)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--check", action="store_true", help="Check an existing asset without writing it.")
    args = parser.parse_args(argv)
    # An explicit output retains the original single text-starter CLI behavior.
    selected = [args.starter or "text-summary"] if args.starter or args.output else STARTERS
    for starter in selected:
        output = args.output or STARTERS[starter][1]
        data = build_bundle(starter=starter)
        if args.check:
            if not output.is_file() or output.read_bytes() != data:
                parser.error("Starter asset differs from source; regenerate it with this script.")
        else:
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_bytes(data)
        print(output)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
