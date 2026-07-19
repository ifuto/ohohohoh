# CI (GitHub Actions) — 有効化手順

`ci/build.yml` / `ci/vendor.yml` は完成済みの GitHub Actions ワークフローです。Arena の GitHub App トークンは
`workflows` 権限を持たないため `.github/workflows/` への直接 push が拒否される関係で、
ここに配置しています (このパスは権限不要)。

> 実測 (2026-07-20): git push / REST API の両方で
> `refusing to allow a GitHub App to create or update workflow ... without 'workflows' permission` /
> `Resource not accessible by integration (HTTP 403)` を確認。
> エージェント側での有効化は不可能なため、以下のどちらかをお願いします。

## 有効化 (どれか1つで OK — a が一番手軽)

1. **ブラウザのみ (推奨・1分):**
   GitHub のリポジトリ → **Actions タブ** → **New workflow** → **set up a workflow yourself** →
   エディタに **`ci/build.yml` または `ci/vendor.yml` の中身を貼り付け** (ファイル名はそれぞれ
   `build.yml` / `vendor.yml`) → **Commit changes**。
   あなたのアカウントからのコミットには workflows 権限が要らないので即座に動き始めます。
2. **ローカルで 3 コマンド:**
   ```cmd
   git mv ci/build.yml .github/workflows/build.yml
   git commit -m "ci: enable build workflow"
   git push
   ```
3. **Arena で GitHub を workflows 権限つきで再接続** → その後エージェントに「CI を有効化して」と依頼。

## ワークフロー一覧

### vendor.yml — cargo vendor を Web 完結で生成 (**今のブロッカー解消用・これだけ先に**)

検証 sandbox は crates.io を遮断されているため `cargo check` の依存取得ができません。
このワークフローは GitHub ランナー (crates.io に接続可) で `cargo vendor --locked` を実行し、
成果物 (依存クレートのソース一式) を **90MB 分割 tarball として `vendor` ブランチへ push** します。
sandbox 側は `ci/fetch_vendor.sh` が git 経由 (=唯一の疎通経路) で受け取り、結合・SHA-256 検証・
展開・`.cargo/config.toml` 生成まで自動で行います。

- 起動: このファイルを Web UI からコミットした瞬間に自動実行 (どこかに移す必要なし)。再実行は Actions → vendor → Run workflow。
- 成果物: `vendor` ブランチ (force push・常に単一コミット。`vendor.tar.gz.part-*` + `SHA256SUMS`)。
- **ローカルに cargo / Rust は一切不要。ブラウザ操作のみで完結します。**

### build.yml — 型検査 + Windows インストーラ EXE ビルド

| ジョブ | ランナー | 内容 | 実行タイミング |
|---|---|---|---|
| `check` | ubuntu-latest | `cargo fmt --check` (参考) / `cargo check --workspace --all-targets --locked` / `cargo clippy` | push 毎 (md/docs 単独 push は除外) |
| `windows-installer` | windows-latest | `tools\build_windows_setup_exe.bat` で **Rsift-1.21.11-Setup.exe** を実ビルドし `rsift-windows-binaries` アーティファクトとしてアップロード | **手動のみ** (Actions → Build & Verify → Run workflow) |

## 無料枠について (private リポジトリ)

このリポジトリは **private** = Actions は月 2,000 分無料 (共有ランナ。Windows は **2倍消費**)。
クォータ節約のため windows-installer ジョブは手動起動のみ + 連続 push 時は古い実行を自動キャンセル
(`concurrency.cancel-in-progress`) する設計にしてあります。目安消費量 (推定):

| 実行 | ランナー時間 | 請求換算 | 月2,000分だと |
|---|---|---|---|
| **vendor (1回きりでOK)** | **~5–15 分** | **×1 = 5–15 分** | **月枠の 1% 未満** |
| check (cargo check+clippy, キャッシュ効き) | ~5–10 分 | ×1 = 5–10 分 | 月200回+ |
| windows exe ビルド (LTO fat フル) | ~30–60 分 | ×2 = 60–120 分 | 月15–30回 |

普段の exe はローカルの `tools\build_windows_setup_exe.bat` (消費ゼロ)、CI は「型検査とリリース候補の
確度的確認が欲しい時だけ手動ポチ」がクォータ最適解です。パブリック公開に切り替えれば標準ランナーは
使い放題 (無制限) になります — ただしコードが公開されます。

JDK 21 (pack-jar.ps1 による bootstrap jar 実ビルド用) もランナー側で自動セットアップされます。
ローカルでビルドする場合は `setup_dev_env.bat` (一括) または `tools\build_windows_setup_exe.bat` (installer 集中) を実行してください。
