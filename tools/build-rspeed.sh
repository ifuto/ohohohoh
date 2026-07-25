#!/bin/bash
# rspeed ビルド (依存クレートなし・std のみ・~10秒)。
# バイナリは /home/user/bin/rspeed (git 追跡外・Turn 間永続)。
set -euo pipefail
export PATH="/home/user/rust/bin:$PATH"
SRC_DIR="$(cd "$(dirname "$0")" && pwd)"
mkdir -p /home/user/bin
rustc -O --edition 2021 "$SRC_DIR/rspeed.rs" -o /home/user/bin/rspeed
echo "built: /home/user/bin/rspeed"
/home/user/bin/rspeed help | head -3
