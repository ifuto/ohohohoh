# CI (GitHub Actions) — 有効化手順

`ci/build.yml` は完成済みの GitHub Actions ワークフローです。Arena の GitHub App トークンは
`workflows` 権限を持たないため `.github/workflows/` への直接 push が拒否される関係で、
ここに配置しています (このパスは権限不要)。

## 有効化 (どちらか一方で OK)

1. **このファイルを移動して push:**
   ```cmd
   git mv ci/build.yml .github/workflows/build.yml
   git commit -m "ci: enable build workflow"
   git push
   ```
2. または GitHub 上で新規 workflow を作成し、`ci/build.yml` の内容を貼り付け。

## 動作内容

| ジョブ | ランナー | 内容 |
|---|---|---|
| `check` | ubuntu-latest | `cargo fmt --check` (参考) / `cargo check --workspace --all-targets --locked` / `cargo clippy` |
| `windows-installer` | windows-latest | `tools\build_windows_setup_exe.bat` で **Rsift-1.21.11-Setup.exe** を実ビルドし `rsift-windows-binaries` アーティファクトとしてアップロード (Actions → 該当 run → Artifacts からダウンロード) |

JDK 21 (pack-jar.ps1 による bootstrap jar 実ビルド用) もランナー側で自動セットアップされます。
ローカルでビルドする場合は `setup_dev_env.bat` (一括) または `tools\build_windows_setup_exe.bat` (installer 集中) を実行してください。
