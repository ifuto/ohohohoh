//! rsift-setup 実行エントリポイント。
//! 使い方: DLL/dylib と同じ階層に置いて実行するだけ。
//!   rsift-setup [--self-test] [--dry-run] [--dir <path>]
//! 終了コード: 0=準備完了 / 2=lib 無し / 3=検証不一致 / 4=IO 失敗 / 5=自己診断失敗。
//! 全工程が rsift_setup_log.txt / rsift_setup_log.jsonl に記録される。

fn main() {
    let cli = match rsift_setup::Cli::parse(std::env::args().skip(1)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("rsift-setup: {e}");
            eprintln!("usage: rsift-setup [--self-test] [--dry-run] [--dir <path>]");
            std::process::exit(64);
        }
    };
    let code = rsift_setup::run(&cli);
    if code == 0 {
        println!("rsift-setup: 準備完了 (詳細は rsift_setup_log.txt)");
    } else {
        println!("rsift-setup: 完了コード {code} (原因は rsift_setup_log.txt を確認)");
    }
    std::process::exit(code);
}
