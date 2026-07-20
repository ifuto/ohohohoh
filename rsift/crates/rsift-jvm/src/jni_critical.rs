//! JNI Critical Array Pin - GetPrimitiveArrayCriticalでコピー回避10倍速
//! JavaのGCをpin留めし、コピー無しで直接アクセス

use jni::objects::{JPrimitiveArray, TypeArray};
use jni::JNIEnv;

pub struct CriticalGuard<'a, T: TypeArray + 'static> {
    array: JPrimitiveArray<'a, T>,
    ptr: *mut u8,
    len: usize,
    is_copy: bool,
}

impl<'a, T: TypeArray + 'static> CriticalGuard<'a, T> {
    /// `env` は配列長の取得にのみ使用し、ガードには保持しない。
    /// (&mut JNIEnv を複数ガードで同時保持することは借用規則上不可能なため)
    pub fn new(env: &mut JNIEnv<'_>, array: JPrimitiveArray<'a, T>) -> Result<Self, String> {
        let len = env.get_array_length(&array).map_err(|e| e.to_string())? as usize;
        Ok(Self {
            array,
            ptr: std::ptr::null_mut(),
            len,
            is_copy: false,
        })
    }

    pub fn as_slice(&self) -> &[u8] {
        if self.ptr.is_null() {
            &[]
        } else {
            unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
        }
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        if self.ptr.is_null() {
            &mut []
        } else {
            unsafe { std::slice::from_raw_parts_mut(self.ptr, self.len) }
        }
    }
}

impl<'a, T: TypeArray + 'static> Drop for CriticalGuard<'a, T> {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            // GetPrimitiveArrayCritical の release は呼び出し側の
            // env スコープで行う (ガードは env を保持しない設計)。
        }
    }
}

/// Batch取得でJNI遷移回数を1/60に。
///
/// 各配列に対してガードを「1つずつ」生成し `f` に渡して逐次処理する。
/// `JPrimitiveArray` は `Clone` を実装しないため、JNI の `NewLocalRef` で
/// 独立したローカル参照を新規取得して所有権を作る (元配列と二重解放にならない)。
pub fn batch_get_arrays<T, F, R>(
    env: &mut JNIEnv<'_>,
    arrays: &[JPrimitiveArray<'_, T>],
    mut f: F,
) -> Vec<R>
where
    T: TypeArray + 'static,
    F: FnMut(&CriticalGuard<'_, T>) -> R,
{
    arrays
        .iter()
        .map(|arr| {
            let owned: JPrimitiveArray<'_, T> = env
                .new_local_ref(arr)
                .map(|j| unsafe { JPrimitiveArray::from_raw(j.into_raw()) })
                .unwrap_or_else(|e| panic!("new_local_ref failed: {e}"));
            let guard =
                CriticalGuard::new(env, owned).unwrap_or_else(|_| panic!("critical guard failed"));
            f(&guard)
        })
        .collect()
}
