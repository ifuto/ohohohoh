# CI (GitHub Actions) — 有効化手順

`ci/build.yml` は完成済みの GitHub Actions ワークフローです。Arena の GitHub App トークンは
`workflows` 権限を持たないため `.github/workflows/` への直接 push が拒否される関係で、
ここに配置しています (このパスは権限不要)。

> 実測 (2026-07-20): git push / REST API の両方で
> `refusing to allow a GitHub App to create or update workflow ... without 'workflows' permission` /
> `Resource not accessible by integration (HTTP 403)` を確認。
> エージェント側での有効化は不可能なため、以下のどちらかをお願いします。

## 有効化 (どちらか一方で OK)

1. **ローカルで 3 コマンドを実行 (推奨・即時):**
   ```cmd
   git mv ci/build.yml .github/workflows/build.yml
   git commit -m "ci: enable build workflow"
   git push
   ```
2. **Arena で GitHub を workflows 権限つきで再接続** → その後エージェントに「CI を有効化して」と依頼。

## 動作内容

| ジョブ | ランナー | 内容 |
|---|---|---|
| `check` | ubuntu-latest | `cargo fmt --check` (参考) / `cargo check --workspace --all-targets --locked` / `cargo clippy` |
| `windows-installer` | windows-latest | `tools\build_windows_setup_exe.bat` で **Rsift-1.21.11-Setup.exe** を実ビルドし `rsift-windows-binaries` アーティファクトとしてアップロード (Actions → 該当 run → Artifacts からダウンロード) |

JDK 21 (pack-jar.ps1 による bootstrap jar 実ビルド用) もランナー側で自動セットアップされます。
ローカルでビルドする場合は `setup_dev_env.bat` (一括) または `tools\build_windows_setup_exe.bat` (installer 集中) を実行してください。
