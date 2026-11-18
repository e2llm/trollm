"""Portable names for bundle sources; file bytes and checksum pins stay untouched."""

from pathlib import Path, PurePath, PurePosixPath, PureWindowsPath


def archive_path(path):
    """Serialize a relative filesystem Path, or validate an existing archive name."""
    if isinstance(path, PurePath):
        if path.anchor:
            raise ValueError("Bundle paths must be relative")
        name = path.as_posix()
    else:
        name = str(path)
    if (not name or name.startswith("/") or "\\" in name or "\0" in name
            or PureWindowsPath(name).drive
            or any(part in ("", ".", "..") for part in name.split("/"))):
        raise ValueError(f"Unsafe bundle path: {name!r}")
    return name


def read_bundle_files(source, names):
    root = Path(source).resolve()
    files = {}
    for path in names:
        name = archive_path(path)
        if name in files:
            raise ValueError(f"Duplicate bundle path: {name!r}")
        target = (root / name).resolve()
        if not target.is_relative_to(root):
            raise ValueError(f"Bundle source escapes its directory: {name!r}")
        files[name] = target.read_bytes()
    return files


def registry_prompt_paths(registry, registry_path):
    """Collect model-task prompts; direct tasks have no prompt payload to read."""
    parent = PurePosixPath(archive_path(registry_path)).parent
    names = set()
    for name, task in registry["tasks"].items():
        execution = task.get("execution", "model")
        if execution == "tool":
            if any(field in task for field in (
                    "model", "prompt", "maxOutputTokens", "generationTimeoutS", "loop")):
                raise ValueError(f"{name}: direct tool tasks cannot contain model/prompt settings")
            continue
        if execution != "model":
            raise ValueError(f"{name}: unknown execution mode")
        if not isinstance(task.get("model"), str) or not task["model"]:
            raise ValueError(f"{name}: model task requires a model slot")
        if not isinstance(task.get("prompt"), str) or not task["prompt"]:
            raise ValueError(f"{name}: model task requires a prompt")
        names.add(archive_path(parent / archive_path(task["prompt"])))
    return sorted(names)
