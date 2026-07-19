#!/usr/bin/env python3
"""HF データセット `ifuto/rsift` との Push/Pull 自動化（運用ルール.md §1 の実装）

使い方:
    export HF_TOKEN="hf_..."            # 必須。ファイルには絶対書かない
    python3 tools/hf_sync.py pull [--subdir smpsystem/]   # HF最新をローカルへ取得
    python3 tools/hf_sync.py status                        # ローカル⇔HF のMD5差分一覧
    python3 tools/hf_sync.py push ファイル [ファイル...] [-m MSG]  # push＋再DLでMD5検証

設計メモ:
    - huggingface_hub が無ければ自前で pip install（サンドボックスは pip 非永続のため）
    - ローカルミラーは /home/user/hf_rsift（HF_RSIFT_DIR で変更可）
    - 絶対にpushしてはいけない: ルート Cargo.toml（ローカル縮小メンバーで運用）
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
import sys
import urllib.request

REPO_ID = "ifuto/rsift"
API_TREE = f"https://huggingface.co/api/datasets/{REPO_ID}/tree/main?recursive=true"
RESOLVE = f"https://huggingface.co/datasets/{REPO_ID}/resolve/main/"
MIRROR = os.environ.get("HF_RSIFT_DIR", "/home/user/hf_rsift")
NEVER_PUSH = {"Cargo.toml"}


def require_token() -> str:
    token = os.environ.get("HF_TOKEN", "").strip()
    if not token:
        sys.exit("HF_TOKEN が未設定です。export HF_TOKEN=\"hf_...\" してから実行してね")
    return token


def http_json(url: str, token: str):
    req = urllib.request.Request(url, headers={"Authorization": f"Bearer {token}"})
    return json.load(urllib.request.urlopen(req, timeout=120))


def http_bytes(url: str, token: str) -> bytes:
    req = urllib.request.Request(url, headers={"Authorization": f"Bearer {token}"})
    return urllib.request.urlopen(req, timeout=180).read()


def md5(data: bytes) -> str:
    return hashlib.md5(data).hexdigest()


def ensure_hub():
    try:
        import huggingface_hub  # noqa: F401
    except ImportError:
        print("[hf_sync] huggingface_hub をインストール中（pip 非永続対策）…")
        subprocess.check_call([sys.executable, "-m", "pip", "install", "-q", "huggingface_hub"])


def tree_files(token: str, subdir: str = "") -> list[str]:
    tree = http_json(API_TREE, token)
    paths = [t["path"] for t in tree if t.get("type") == "file"]
    if subdir:
        prefix = subdir.rstrip("/") + "/"
        paths = [p for p in paths if p.startswith(prefix)]
    return sorted(paths)


def cmd_pull(args) -> None:
    token = require_token()
    paths = tree_files(token, args.subdir)
    print(f"[pull] {len(paths)} files from HF …")
    ok = 0
    for i, path in enumerate(paths, 1):
        data = http_bytes(RESOLVE + urllib.parse.quote(path), token)
        dest = os.path.join(MIRROR, path)
        os.makedirs(os.path.dirname(dest), exist_ok=True)
        with open(dest, "wb") as fh:
            fh.write(data)
        ok += 1
        if i % 100 == 0:
            print(f"  …{i}/{len(paths)}")
    print(f"[pull] 完了: {ok} files → {MIRROR}")


def cmd_status(args) -> None:
    token = require_token()
    paths = tree_files(token, args.subdir)
    print(f"[status] {len(paths)} files を照合中 …")
    diffs, missing_local = [], []
    for path in paths:
        local = os.path.join(MIRROR, path)
        if not os.path.exists(local):
            missing_local.append(path)
            continue
        remote = md5(http_bytes(RESOLVE + urllib.parse.quote(path), token))
        with open(local, "rb") as fh:
            if md5(fh.read()) != remote:
                diffs.append(path)
    # ローカルにあってHFに無いもの（push候補）
    remote_set = set(paths)
    local_only = []
    for root, _, names in os.walk(MIRROR):
        if any(x in root for x in ("target/", ".git/", "build/", "bench_out_local/")):
            continue
        for name in names:
            full = os.path.join(root, name)
            rel = os.path.relpath(full, MIRROR).replace(os.sep, "/")
            if rel not in remote_set and not rel.startswith("target/"):
                local_only.append(rel)
    print(f"  MD5不一致(ローカルが新しい/違う): {len(diffs)}")
    for p in diffs:
        print("   ~", p)
    print(f"  HF側にのみ存在: {len(missing_local)}")
    for p in missing_local:
        print("   -", p)
    print(f"  ローカルのみ存在(push候補): {len(local_only)}")
    for p in local_only:
        print("   +", p)


def cmd_push(args) -> None:
    token = require_token()
    ensure_hub()
    from huggingface_hub import HfApi
    from huggingface_hub.hf_api import CommitOperationAdd

    files = []
    for f in args.files:
        rel = os.path.relpath(os.path.abspath(f), os.path.abspath(MIRROR)).replace(os.sep, "/") \
            if os.path.isabs(f) else f.replace(os.sep, "/")
        if rel in NEVER_PUSH:
            sys.exit(f"絶対にpushしないでね（ローカル縮小版だから）: {rel}")
        if not os.path.isfile(os.path.join(MIRROR, rel)):
            sys.exit(f"ファイルがミラーに無いよ: {rel}")
        files.append(rel)

    ops, md5s = [], {}
    for rel in files:
        with open(os.path.join(MIRROR, rel), "rb") as fh:
            data = fh.read()
        md5s[rel] = md5(data)
        ops.append(CommitOperationAdd(path_in_repo=rel, path_or_fileobj=data))

    message = args.message or ("hf_sync push: " + ", ".join(files[:3])
                               + (" …" if len(files) > 3 else ""))
    api = HfApi(token=token)
    info = api.create_commit(repo_id=REPO_ID, repo_type="dataset",
                             operations=ops, commit_message=message)
    print("[push] commit:", info.commit_url)

    bad = 0
    for rel in files:
        remote = md5(http_bytes(RESOLVE + urllib.parse.quote(rel), token))
        state = "OK " if remote == md5s[rel] else "BAD"
        if state == "BAD":
            bad += 1
        print(f"  {state} {rel}")
    print(f"[push] 検証: BAD={bad}")
    sys.exit(1 if bad else 0)


def main() -> None:
    parser = argparse.ArgumentParser(description="HF ifuto/rsift push/pull 自動化")
    sub = parser.add_subparsers(dest="command", required=True)
    p_pull = sub.add_parser("pull", help="HF最新をローカルミラーへ取得")
    p_pull.add_argument("--subdir", default="")
    p_pull.set_defaults(func=cmd_pull)
    p_status = sub.add_parser("status", help="ローカル⇔HF のMD5差分一覧")
    p_status.add_argument("--subdir", default="")
    p_status.set_defaults(func=cmd_status)
    p_push = sub.add_parser("push", help="変更ファイルをpush＋MD5検証")
    p_push.add_argument("files", nargs="+")
    p_push.add_argument("-m", "--message", default="")
    p_push.set_defaults(func=cmd_push)
    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
