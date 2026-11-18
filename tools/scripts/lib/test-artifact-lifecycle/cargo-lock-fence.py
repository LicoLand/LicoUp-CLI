import errno
import fcntl
import json
import os
import shlex
import stat
import subprocess
import sys

with open(os.path.join(os.path.dirname(__file__), "cargo-lock-policy.json"), encoding="utf-8") as policy_file:
    POLICY = json.load(policy_file)
LOCK_NAMES = set(POLICY["lockNames"])
COMPILER_DIRECTORIES = set(POLICY["compilerDirectories"])
COMPILER_FILES = set(POLICY["compilerFiles"])
COMPILER_SUFFIXES = set(POLICY["compilerSuffixes"])
REPOSITORY_METADATA = {".git", ".hg", ".svn"}


def inside(root, candidate):
    try:
        return os.path.commonpath((root, candidate)) == root
    except ValueError:
        return False


def canonical_existing_directory(candidate):
    if not isinstance(candidate, str) or not candidate:
        return None
    lexical = os.path.abspath(candidate)
    canonical = os.path.realpath(lexical)
    if lexical != canonical:
        return None
    try:
        info = os.lstat(lexical)
    except OSError:
        return None
    if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
        return None
    return lexical


def unavailable():
    print(json.dumps({"status": "probe-unavailable"}))


def profile_locks(root, profiles):
    locks = []
    directories = profile_directory_identities(root, profiles)
    if directories is None:
        return None
    for profile in profiles:
        relative = profile["relativeProfile"]
        names = profile.get("lockNames")
        directory = os.path.abspath(os.path.join(root, relative))
        if not isinstance(names, list) or not names:
            return None
        for name in names:
            if name not in LOCK_NAMES:
                return None
            lock_path = os.path.join(directory, name)
            try:
                info = os.lstat(lock_path)
            except OSError:
                return None
            if stat.S_ISLNK(info.st_mode) or not stat.S_ISREG(info.st_mode):
                return None
            locks.append(lock_path)
    return sorted(set(locks)), directories


def profile_directory_identities(root, profiles):
    root_info = None
    try:
        root_info = os.lstat(root)
    except OSError:
        return None
    if stat.S_ISLNK(root_info.st_mode) or not stat.S_ISDIR(root_info.st_mode):
        return None
    identities = {"": (root_info.st_dev, root_info.st_ino)}
    for profile in profiles:
        relative = profile.get("relativeProfile")
        if not isinstance(relative, str) or not relative or os.path.isabs(relative):
            return None
        normalized = os.path.normpath(relative)
        if normalized != relative or normalized == "." or normalized.startswith(".." + os.sep):
            return None
        current = root
        for component in relative.split(os.sep):
            if component in {"", ".", ".."}:
                return None
            current = os.path.join(current, component)
            try:
                info = os.lstat(current)
            except OSError:
                return None
            if stat.S_ISLNK(info.st_mode) or not stat.S_ISDIR(info.st_mode):
                return None
            identities[os.path.relpath(current, root)] = (info.st_dev, info.st_ino)
    return identities


def same_profile_directories(root, profiles, expected):
    current = profile_directory_identities(root, profiles)
    return current is not None and current == expected


def open_checked_lock(lock_path):
    if not hasattr(os, "O_NOFOLLOW"):
        return None
    descriptor = None
    try:
        before = os.lstat(lock_path)
        if stat.S_ISLNK(before.st_mode) or not stat.S_ISREG(before.st_mode):
            return None
        descriptor = os.open(lock_path, os.O_RDWR | os.O_NOFOLLOW)
        opened = os.fstat(descriptor)
        linked = os.lstat(lock_path)
        if (not stat.S_ISREG(opened.st_mode) or stat.S_ISLNK(linked.st_mode) or
                (before.st_dev, before.st_ino) != (opened.st_dev, opened.st_ino) or
                (opened.st_dev, opened.st_ino) != (linked.st_dev, linked.st_ino)):
            os.close(descriptor)
            return None
        return os.fdopen(descriptor, "r+b", buffering=0)
    except OSError:
        if descriptor is not None:
            try:
                os.close(descriptor)
            except OSError:
                pass
        return None


def lock_path_matches_handle(lock_path, handle):
    try:
        opened = os.fstat(handle.fileno())
        linked = os.lstat(lock_path)
    except OSError:
        return False
    return stat.S_ISREG(opened.st_mode) and not stat.S_ISLNK(linked.st_mode) and (
        (opened.st_dev, opened.st_ino) == (linked.st_dev, linked.st_ino)
    )


def parse_process_usage_macos(
    target_root,
    repo_root,
    process_stdout,
    process_stderr,
    process_status,
    open_stdout,
    open_stderr,
    open_status,
):
    if (process_status != 0 or open_status != 0 or process_stderr.strip() or
            open_stderr.strip()):
        return "probe-unavailable"

    arguments_by_pid = {}
    for line in process_stdout.splitlines():
        try:
            pid, arguments = line.strip().split(None, 1)
            arguments_by_pid[int(pid)] = arguments
        except (ValueError, TypeError):
            continue

    entries = {}
    pid = None
    command = ""
    descriptor = ""
    for line in open_stdout.splitlines():
        if line.startswith("p"):
            try:
                pid = int(line[1:])
            except ValueError:
                pid = None
            command = ""
            descriptor = ""
        elif line.startswith("c"):
            command = line[1:]
        elif line.startswith("f"):
            descriptor = line[1:]
        elif line.startswith("n") and pid is not None:
            current = entries.setdefault(pid, {"command": command, "cwd": "", "executables": []})
            if descriptor == "cwd":
                current["cwd"] = line[1:]
            elif descriptor == "txt" and line[1:] not in current["executables"]:
                current["executables"].append(line[1:])

    for current_pid, entry in entries.items():
        if any(inside(
            target_root,
            os.path.realpath(executable).removesuffix(" (deleted)"),
        ) for executable in entry["executables"]):
            return "running-binary"
        arguments = arguments_by_pid.get(current_pid, "")
        try:
            command_name = os.path.basename(shlex.split(arguments)[0]).lower()
        except (IndexError, ValueError):
            command_name = entry["command"].lower()
        if command_name.removesuffix(".exe") in {"cargo", "rustc"} and (
            (entry["cwd"] and inside(repo_root, os.path.realpath(entry["cwd"])))
            or target_root in arguments
        ):
            return "active-build"
    return None


def process_usage_macos(target_root, repo_root):
    process_list = subprocess.run(
        ["ps", "-axo", "pid=,command="],
        check=False,
        capture_output=True,
        text=True,
    )
    open_files = subprocess.run(
        ["lsof", "-nP", "-Fpcfn", "-d", "txt,cwd"],
        check=False,
        capture_output=True,
        text=True,
    )
    return parse_process_usage_macos(
        target_root,
        repo_root,
        process_list.stdout,
        process_list.stderr,
        process_list.returncode,
        open_files.stdout,
        open_files.stderr,
        open_files.returncode,
    )


def process_usage(target_root, repo_root):
    if sys.platform == "darwin":
        return process_usage_macos(target_root, repo_root)
    return "probe-unavailable"


def clear_directory(directory):
    removed = False
    for entry in os.scandir(directory):
        if entry.name in LOCK_NAMES:
            continue
        try:
            if entry.is_dir(follow_symlinks=False):
                removed = clear_directory(entry.path) or removed
                try:
                    os.rmdir(entry.path)
                    removed = True
                except OSError as error:
                    if error.errno not in (errno.ENOTEMPTY, errno.EEXIST):
                        raise
            elif entry.is_file(follow_symlinks=False) or entry.is_symlink():
                os.unlink(entry.path)
                removed = True
        except FileNotFoundError:
            continue
    return removed


def clear_profile(profile):
    removed = False
    for entry in os.scandir(profile):
        if entry.name in LOCK_NAMES:
            continue
        try:
            if entry.is_dir(follow_symlinks=False):
                if entry.name not in COMPILER_DIRECTORIES:
                    continue
                removed = clear_directory(entry.path) or removed
                try:
                    os.rmdir(entry.path)
                    removed = True
                except OSError as error:
                    if error.errno not in (errno.ENOTEMPTY, errno.EEXIST):
                        raise
            elif entry.is_file(follow_symlinks=False):
                info = entry.stat(follow_symlinks=False)
                extension = os.path.splitext(entry.name)[1].lower()
                is_executable = bool(info.st_mode & (stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH))
                if entry.name in COMPILER_FILES or extension in COMPILER_SUFFIXES or (is_executable and not extension):
                    os.unlink(entry.path)
                    removed = True
        except FileNotFoundError:
            continue
    return removed


def contains_repository_metadata(directory):
    for entry in os.scandir(directory):
        if entry.name in REPOSITORY_METADATA:
            return True
        if entry.is_dir(follow_symlinks=False) and contains_repository_metadata(entry.path):
            return True
    return False


def profiles_contain_repository_metadata(target_root, profiles):
    for profile in profiles:
        directory = os.path.join(target_root, profile["relativeProfile"])
        for entry in os.scandir(directory):
            if (entry.name in COMPILER_DIRECTORIES and entry.is_dir(follow_symlinks=False) and
                    contains_repository_metadata(entry.path)):
                return True
    return False


def main():
    if sys.platform != "darwin":
        print(json.dumps({"status": "lock-unavailable"}))
        return
    request = json.load(sys.stdin)
    target_root = canonical_existing_directory(request.get("targetRoot"))
    repo_root = canonical_existing_directory(request.get("repoRoot"))
    profiles = request.get("profiles")
    if target_root is None or repo_root is None or not isinstance(profiles, list) or not profiles:
        print(json.dumps({"status": "lock-unavailable"}))
        return
    lock_state = profile_locks(target_root, profiles)
    if not lock_state:
        print(json.dumps({"status": "lock-unavailable"}))
        return
    locks, directory_identities = lock_state
    if not locks:
        print(json.dumps({"status": "lock-unavailable"}))
        return

    handles = []
    try:
        for lock_path in locks:
            handle = open_checked_lock(lock_path)
            if handle is None:
                print(json.dumps({"status": "lock-unavailable"}))
                return
            handles.append((lock_path, handle))
            try:
                fcntl.flock(handle.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                print(json.dumps({"status": "active-build"}))
                return
            if not lock_path_matches_handle(lock_path, handle):
                print(json.dumps({"status": "lock-unavailable"}))
                return

        if not same_profile_directories(target_root, profiles, directory_identities):
            print(json.dumps({"status": "lock-unavailable"}))
            return

        usage = process_usage(target_root, repo_root)
        if usage:
            print(json.dumps({"status": usage}))
            return

        if (not same_profile_directories(target_root, profiles, directory_identities) or
                any(not lock_path_matches_handle(lock_path, handle) for lock_path, handle in handles)):
            print(json.dumps({"status": "lock-unavailable"}))
            return
        if profiles_contain_repository_metadata(target_root, profiles):
            print(json.dumps({"status": "lock-unavailable"}))
            return

        removed = False
        for profile in profiles:
            directory = os.path.abspath(os.path.join(target_root, profile["relativeProfile"]))
            removed = clear_profile(directory) or removed
        print(json.dumps({"status": "removed" if removed else "empty"}))
    except (OSError, ValueError, KeyError, TypeError):
        print(json.dumps({"status": "lock-unavailable"}))
    finally:
        for _, handle in handles:
            handle.close()


if __name__ == "__main__":
    main()
