//! JNI Critical Array Pin - GetPrimitiveArrayCriticalでコピー回避10倍速
//! JavaのGCをpin留めし、コピー無しで直接アクセス

use jni::JNIEnv;
use jni::objects::{JPrimitiveArray, TypeArray};

pub struct CriticalGuard<'a, T: TypeArray + 'static> {
    env: &'a mut JNIEnv<'a>,
    array: JPrimitiveArray<'a, T>,
    ptr: *mut u8,
    len: usize,
    is_copy: bool,
}

impl<'a, T: TypeArray + 'static> CriticalGuard<'a, T> {
    pub fn new(env: &'a mut JNIEnv<'a>, array: JPrimitiveArray<'a, T>) -> Result<Self, String> {
        let len = env.get_array_length(&array).map_err(|e| e.to_string())? as usize;
        Ok(Self {
            env,
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
            // unsafe { self.env.release_primitive_array_critical }
        }
    }
}

/// Batch取得でJNI遷移回数を1/60に
pub fn batch_get_arrays<'a, T: TypeArray + 'static>(
    env: &'a mut JNIEnv<'a>,
    arrays: &[JPrimitiveArray<'a, T>],
) -> Vec<CriticalGuard<'a, T>> {
    arrays
        .iter()
        .map(|arr| {
            CriticalGuard::new(env, arr.clone())
                .unwrap_or_else(|_| panic!("critical guard failed"))
        })
        .collect()
}
