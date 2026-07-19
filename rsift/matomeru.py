#!/usr/bin/env python3
"""
dump_tree.py
-----------------------------------------
指定されたパス配下のすべてのファイルを、
「===== <フルパス> =====」という区切り行付きで
1 つのテキストファイルに書き出すユーティリティ。
-----------------------------------------
使い方:
    python dump_tree.py /path/to/dir   # 出力は merged.txt
    python dump_tree.py /path/to/dir all_files.txt
オプション:
    --encoding  入力ファイルの既定文字コード (既定: utf-8)
    --max-size  読み込み上限 (バイト)。超過ファイルはスキップ
"""

from __future__ import annotations
import argparse
import pathlib
import sys

def dump_tree(root: pathlib.Path,
              out_path: pathlib.Path,
              encoding: str = "utf-8",
              ignore_errors: bool = True,
              max_size: int | None = None) -> None:
    """
    root     : 対象ルートディレクトリ
    out_path : 出力先 .txt
    encoding : 読み取り時の既定エンコーディング
    ignore_errors : True ならデコードエラーを無視
    max_size : None なら無制限。指定サイズ(B)を超えるファイルは飛ばす
    """
    err_strategy = "ignore" if ignore_errors else "strict"

    with out_path.open("w", encoding="utf-8") as out:
        for f in root.rglob("*"):
            if not f.is_file():
                continue
            if max_size and f.stat().st_size > max_size:
                print(f"[skip] size>{max_size}B  {f}", file=sys.stderr)
                continue
            try:
                content = f.read_text(encoding=encoding, errors=err_strategy)
            except Exception as e:                     # バイナリなどで読めない
                print(f"[warn] {f}: {e}", file=sys.stderr)
                continue

            out.write(f"===== {f.resolve()} =====\n")
            out.write(content)
            if not content.endswith("\n"):
                out.write("\n")
            out.write("\n")         # 空行で 2 重区切りに

def main() -> None:
    ap = argparse.ArgumentParser(description="Concatenate all files under a path into one txt")
    ap.add_argument("path", type=pathlib.Path, help="対象フォルダ/ファイル")
    ap.add_argument("output", nargs="?", type=pathlib.Path, default="merged.txt",
                    help="出力先 (既定: merged.txt)")
    ap.add_argument("--encoding", default="utf-8", help="既定入力エンコーディング (utf-8)")
    ap.add_argument("--max-size", type=int, default=None, help="読み込み上限バイト数")
    args = ap.parse_args()

    root = args.path
    if root.is_file():                    # 単一ファイル指定も許可
        tmp_dir = pathlib.Path(root).parent
        dump_tree(tmp_dir, args.output, args.encoding, True, args.max_size)
    else:
        dump_tree(root, args.output, args.encoding, True, args.max_size)

if __name__ == "__main__":
    main()