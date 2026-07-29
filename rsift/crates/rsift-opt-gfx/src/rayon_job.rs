
//! Rayon work-stealing Chunk Meshing - 32セクション並列
//!
//! 【wave 172 FR 捕捉 110/111】旧 `build_pcore_threadpool` は doc「P-Core
//! 専用・core_affinity クレートでピン留め」と称したが affinity 実装も
//! core_affinity dep (Cargo.toml/vendor 皆無) もなく名実共に虚構、`RayonJobConfig`
//! (threads 読み書きゼロ) と `parallel_for_each_chunk` (薄ラッパ) は全クレート
//! 全域で呼出消費者ゼロの未配線 → 3 件を不可能証明削除 (FH 捕捉 92 恒値
//! スタブ削除・EJ-2 判例)。wiring が実消費する parallel_map_chunks 単機能へ
//! 縮退 (truth: 実稼働しているのは map のみ)。

use rayon::prelude::*;

/// チャンク列を並列 map。`IntoParallelIterator` の indexed collect で順序保持
/// (wiring:863 の morton 局所性コード生成が実消費 = インデックス対応が契約)。
pub fn parallel_map_chunks<I, T, F>(chunks: I, f: F) -> Vec<T>
where
    I: IntoParallelIterator,
    F: Fn(I::Item) -> T + Sync + Send,
    T: Send,
    I::Item: Send,
{
    chunks.into_par_iter().map(f).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallel_map_preserves_order_and_values() {
        let v: Vec<i32> = (0..10_000).collect();
        let out = parallel_map_chunks(v, |x| x * 2 + 1);
        assert_eq!(out.len(), 10_000);
        for (i, x) in out.iter().enumerate() {
            assert_eq!(*x, i as i32 * 2 + 1); // indexed collect は順序保持
        }
    }

    /// 【wave 172 FR】morton 用途の truth: 同一入力の 2 回実行は indexed
    /// collect で bit 完全一致 (順序保持の決定論 pin)。green-today pin。
    #[test]
    fn fr_map_deterministic_twice_and_indexed_order() {
        let keys: Vec<(i32, i32)> = (-200i32..200).map(|i| (i * 7, i * -13)).collect();
        let f = |(cx, cz): (i32, i32)| (cx as u64) & 1023 | (((cz as u64) & 1023) << 10);
        let a = parallel_map_chunks(keys.clone(), f);
        let b = parallel_map_chunks(keys.clone(), f);
        assert_eq!(a, b, "2 回実行 bit 完全一致");
        let expect: Vec<u64> = keys.into_iter().map(f).collect();
        assert_eq!(a, expect, "シリアル参照と順序完全一致");
    }
    /// 【wave 184 GD】削除済み API `build_pcore` の再出現 lexeme pin (adversarial
    /// 172-b・21 例目 非検出回収、wave 26 例未採から 1 回収): 削除 truth 証跡として
    /// 宣言形が再起しないことを機械 pin。自己言及 vacuous 回避のため検出
    /// 語彙は分割記述 (doc 証跡条文には `fn ` 接頭で択定範囲外)。
    #[test]
    fn gd_removed_build_pcore_lexeme() {
        let src = include_str!("rayon_job.rs");
        let lex = concat!("fn build", "_pcore");
        assert!(
            !src.contains(lex),
            "削除 API `build_pcore` の宣言再来を検出 → 死救出は lint/テスト限界で"
        );
    }

}
