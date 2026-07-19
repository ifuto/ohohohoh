#!/usr/bin/env python3
"""サンドボックス環境の自動ブートストラップ（スナップショットで消える要素を復旧）

 Arenaサンドボックスはセッション間で ~/.cargo の toolchain、~/.jdk21、pip などが
 消える（パーミッション喪失もある）。このスクリプトは冪等に全部を直す。

使い方:
    python3 tools/env_bootstrap.py --all      # rust + jdk21 をまとめて復旧
    python3 tools/env_bootstrap.py --rust     # rustup + stable toolchain (minimal)
    python3 tools/env_bootstrap.py --jdk      # Temurin JDK 21 (~/.jdk21)
"""
from __future__ import annotations

import argparse
import os
import stat
import subprocess
import sys
import tarfile
import urllib.request

HOME = os.path.expanduser("~")
CARGO_BIN = os.path.join(HOME, ".cargo", "bin")
JDK_DIR = os.path.join(HOME, ".jdk21")
ADOPTIUM_JDK21 = ("https://api.adoptium.net/v3/binary/latest/21/ga/"
                  "linux/x64/jdk/hotspot/normal/eclipse")
RUSTUP_INIT = "https://sh.rustup.rs"


def chmod_x(path: str) -> None:
    if os.path.isfile(path):
        os.chmod(path, os.stat(path).st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def run(cmd: list[str], **kw) -> None:
    print("[bootstrap] $", " ".join(cmd))
    subprocess.check_call(cmd, **kw)


def bootstrap_rust() -> None:
    rustup = os.path.join(CARGO_BIN, "rustup")
    if not os.path.isfile(rustup):
        print("[rust] rustup 本体が無い → rustup-init を取得")
        installer = "/tmp/rustup-init.sh"
        urllib.request.urlretrieve(RUSTUP_INIT, installer)
        run(["sh", installer, "-y", "--profile", "minimal",
             "--default-toolchain", "stable"])
    else:
        for name in os.listdir(CARGO_BIN):
            chmod_x(os.path.join(CARGO_BIN, name))
    env = dict(os.environ, PATH=CARGO_BIN + os.pathsep + os.environ.get("PATH", ""))
    if subprocess.call(["rustc", "--version"], env=env,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL) != 0:
        run(["rustup", "toolchain", "install", "stable", "--profile", "minimal"], env=env)
    run(["rustc", "--version"], env=env)
    run(["cargo", "--version"], env=env)
    print("[rust] OK")


def bootstrap_jdk() -> None:
    java = os.path.join(JDK_DIR, "bin", "java")
    if os.path.isfile(java) and subprocess.call(
            [java, "-version"], stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL) == 0:
        print("[jdk] 既に有効:", java)
        return
    print("[jdk] Temurin 21 をダウンロード中（~200MB）…")
    archive = "/tmp/jdk21.tar.gz"
    urllib.request.urlretrieve(ADOPTIUM_JDK21, archive)
    os.makedirs(JDK_DIR, exist_ok=True)
    with tarfile.open(archive, "r:gz") as tar:
        members = tar.getmembers()
        top = members[0].name.split("/")[0]
        for m in members:
            rel = m.name[len(top):].lstrip("/")
            if not rel:
                continue
            m.name = rel
            tar.extract(m, JDK_DIR, filter="data")
    os.remove(archive)
    run([java, "-version"])
    print("[jdk] OK:", java)


def main() -> None:
    parser = argparse.ArgumentParser(description="sandbox 環境ブートストラップ")
    parser.add_argument("--rust", action="store_true")
    parser.add_argument("--jdk", action="store_true")
    parser.add_argument("--all", action="store_true")
    args = parser.parse_args()
    if args.rust or args.all:
        bootstrap_rust()
    if args.jdk or args.all:
        bootstrap_jdk()
    if not (args.rust or args.jdk or args.all):
        parser.print_help()


if __name__ == "__main__":
    main()
