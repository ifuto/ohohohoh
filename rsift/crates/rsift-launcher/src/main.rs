//! # Rsift Launcher Main Entry Point
//!
//! ユーザーが起動する純粋なRust製実行バイナリ。
//! clap の CLI パーサー設定に allow_hyphen_values = true を付加し、
//! `-Xms4G` や `-XX:...` などのハイフンで始まる JVM 引数も正確にパースします。

use clap::Parser;
use rsift_jvm::JvmConfig;
use std::path::PathBuf;
use tracing::Level;
use tracing_subscriber::FmtSubscriber;

#[derive(Parser, Debug)]
#[command(name = "rsift")]
#[command(author = "Rsift Project Contributors")]
#[command(version = "0.1.0-alpha (1.21.11 Edition)")]
#[command(about = "World's First Native-Injection Rust Mod Loader for Minecraft 1.21.11", long_about = None)]
struct Args {
    /// Mod ディレクトリパス (ネイティブ .dll / .so / .dylib の格納先)
    #[arg(short, long, default_value = "./mods")]
    mod_dir: PathBuf,

    /// Minecraft クラスパス
    #[arg(short, long, default_value = "minecraft_1.21.11.jar:libraries/*")]
    classpath: String,

    /// 最小ヒープメモリサイズ (ハイフン開始の値を許可)
    #[arg(long, default_value = "-Xms2G", allow_hyphen_values = true)]
    min_heap: String,

    /// 最大ヒープメモリサイズ (ハイフン開始の値を許可)
    #[arg(long, default_value = "-Xmx8G", allow_hyphen_values = true)]
    max_heap: String,

    /// オフヒープメモリ制限 (ハイフン開始の値を許可)
    #[arg(long, default_value = "-XX:MaxDirectMemorySize=4G", allow_hyphen_values = true)]
    max_direct_memory: String,

    /// ウィンドウ幅
    #[arg(long, default_value_t = 1920)]
    width: u32,

    /// ウィンドウ高さ
    #[arg(long, default_value_t = 1080)]
    height: u32,

    /// ログレベル (trace, debug, info, warn, error)
    #[arg(short, long, default_value = "info")]
    log_level: String,

    /// Minecraft本体に渡す追加引数
    #[arg(last = true)]
    game_args: Vec<String>,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // ロガーの初期化
    let log_level = match args.log_level.to_lowercase().as_str() {
        "trace" => Level::TRACE,
        "debug" => Level::DEBUG,
        "warn" => Level::WARN,
        "error" => Level::ERROR,
        _ => Level::INFO,
    };

    let subscriber = FmtSubscriber::builder()
        .with_max_level(log_level)
        .with_target(false)
        .with_thread_ids(true)
        .with_file(true)
        .with_line_number(true)
        .finish();

    tracing::subscriber::set_global_default(subscriber)
        .expect("Failed to set tracing subscriber");

    let jvm_config = JvmConfig {
        classpath: args.classpath,
        min_heap: args.min_heap,
        max_heap: args.max_heap,
        max_direct_memory: args.max_direct_memory,
        ..Default::default()
    };

    let mut orchestrator = rsift_launcher::lifecycle::RsiftOrchestrator::new(
        args.mod_dir,
        jvm_config,
        args.width,
        args.height,
    );

    if let Err(e) = orchestrator.run(args.game_args) {
        tracing::error!("Rsift execution terminated with error: {}", e);
        std::process::exit(1);
    }

    Ok(())
}
