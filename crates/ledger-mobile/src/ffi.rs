use std::collections::BTreeMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{LazyLock, Mutex};

const MAX_INPUT: usize = 4096;
const MAX_BUFFERS: usize = 16;
const MAX_OUTPUT: usize = 1024 * 1024;

struct Buffers {
    next: u64,
    values: BTreeMap<u64, Vec<u8>>,
}
static BUFFERS: LazyLock<Mutex<Buffers>> = LazyLock::new(|| {
    Mutex::new(Buffers {
        next: 1,
        values: BTreeMap::new(),
    })
});

fn error(code: &str, message: &str) -> Vec<u8> {
    serde_json::json!({"ok":false,"error":{"code":code,"message":message}})
        .to_string()
        .into_bytes()
}

fn insert(bytes: Vec<u8>) -> u64 {
    let Ok(mut buffers) = BUFFERS.lock() else {
        return 0;
    };
    if buffers.values.len() >= MAX_BUFFERS || bytes.len() > MAX_OUTPUT {
        return 0;
    }
    let Some(next) = buffers.next.checked_add(1) else {
        return 0;
    };
    let id = buffers.next;
    buffers.next = next;
    buffers.values.insert(id, bytes);
    id
}

#[unsafe(no_mangle)]
pub extern "C" fn radishledger_probe_abi() -> u32 {
    radishledger_core::validation::ABI
}

/// 当前已加载代码资产的实际路径，供 FRB 使用同一库，不猜测打包目录。
#[unsafe(no_mangle)]
pub extern "C" fn radishledger_probe_library_path() -> u64 {
    catch_unwind(|| {
        let mut info = std::mem::MaybeUninit::<libc::Dl_info>::uninit();
        // SAFETY: 当前导出函数地址有效，info 为足够空间的可写 Dl_info。
        if unsafe {
            libc::dladdr(
                radishledger_probe_abi as *const () as *const libc::c_void,
                info.as_mut_ptr(),
            )
        } == 0
        {
            return 0;
        }
        let info = unsafe { info.assume_init() };
        if info.dli_fname.is_null() {
            return 0;
        }
        // SAFETY: dladdr 成功后的 dli_fname 是动态链接器管理的 NUL 结尾路径。
        let path = unsafe { std::ffi::CStr::from_ptr(info.dli_fname) };
        insert(path.to_bytes().to_vec())
    })
    .unwrap_or(0)
}

/// # Safety
/// 非空 input 必须指向 length 字节的可读内存，且整个调用期间存活。内存仅借用。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn radishledger_probe_run(input: *const u8, length: usize) -> u64 {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if input.is_null() || length == 0 || length > MAX_INPUT {
            return error("invalid_argument", "输入长度或指针无效");
        }
        // SAFETY: C 调用者按契约提供有效、存活的 length 字节。
        let bytes = unsafe { std::slice::from_raw_parts(input, length) };
        let Ok(directory) = std::str::from_utf8(bytes) else {
            return error("invalid_utf8", "输入不是有效 UTF-8");
        };
        radishledger_core::probe::run_json(directory).into_bytes()
    }));
    insert(result.unwrap_or_else(|_| error("internal_error", "核心调用异常")))
}

/// # Safety
/// input 必须在整个调用期间指向 length 字节的可读内存；只借用，不保留。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn radishledger_probe_validate(input: *const u8, length: usize) -> u64 {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if input.is_null() || length == 0 || length > MAX_INPUT {
            return error("invalid_argument", "输入长度或指针无效");
        }
        // SAFETY: 调用者按 C 契约提供存活的 length 字节。
        let bytes = unsafe { std::slice::from_raw_parts(input, length) };
        let Ok(request) = std::str::from_utf8(bytes) else {
            return error("invalid_utf8", "输入不是有效 UTF-8");
        };
        crate::api::run_validation(request.to_owned()).into_bytes()
    }));
    insert(result.unwrap_or_else(|_| error("internal_error", "核心调用异常")))
}

/// 当前结果句柄数，仅供实验观测；毒化明确返回 -1。
#[unsafe(no_mangle)]
pub extern "C" fn radishledger_probe_buffer_count() -> i32 {
    BUFFERS.lock().map(|b| b.values.len() as i32).unwrap_or(-1)
}

#[unsafe(no_mangle)]
pub extern "C" fn radishledger_probe_buffer_len(handle: u64) -> usize {
    catch_unwind(|| {
        BUFFERS
            .lock()
            .ok()
            .and_then(|values| values.values.get(&handle).map(Vec::len))
            .unwrap_or(0)
    })
    .unwrap_or(0)
}

/// # Safety
/// 非空 output 必须指向 capacity 字节可写内存。短容量 / 无效 handle 不写内存。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn radishledger_probe_buffer_copy(
    handle: u64,
    output: *mut u8,
    capacity: usize,
) -> usize {
    catch_unwind(AssertUnwindSafe(|| {
        let Ok(buffers) = BUFFERS.lock() else {
            return 0;
        };
        let Some(bytes) = buffers.values.get(&handle) else {
            return 0;
        };
        if output.is_null() || capacity < bytes.len() {
            return 0;
        }
        // SAFETY: 调用者保证足够可写容量；Rust buffer 与输出独立，锁定期间不释放。
        unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), output, bytes.len()) };
        bytes.len()
    }))
    .unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn radishledger_probe_buffer_free(handle: u64) -> i32 {
    catch_unwind(|| {
        BUFFERS
            .lock()
            .ok()
            .and_then(|mut values| values.values.remove(&handle))
            .map_or(0, |_| 1)
    })
    .unwrap_or(0)
}

/// M0-C 单独的秘密通道；普通句柄永远只保存非秘密 JSON。
/// # Safety
/// metadata / secret 分别指向声明长度的可读内存；output 指向至少 128 字节的独立
/// 可写内存，written 指向可写 usize。指针在整个同步调用期间存活且不重叠。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn radishledger_bootstrap_execute(
    metadata: *const u8,
    metadata_length: usize,
    secret: *const u8,
    secret_length: usize,
    output: *mut u8,
    capacity: usize,
    written: *mut usize,
) -> u64 {
    if written.is_null() {
        return insert(error("invalid_argument", "秘密输出长度指针无效"));
    }
    // SAFETY: 调用者保证 written 是独立有效的 usize。
    unsafe {
        *written = 0;
    }
    if metadata.is_null()
        || metadata_length == 0
        || metadata_length > MAX_INPUT
        || (secret.is_null() && secret_length != 0)
        || secret_length > 1104
        || output.is_null()
        || capacity < 128
    {
        return insert(error("invalid_argument", "bootstrap 缓冲区长度或指针无效"));
    }
    // 先预留普通结果句柄，容量不足时不执行具有副作用的核心动作。
    let handle = insert(Vec::new());
    if handle == 0 {
        return 0;
    }
    let response = catch_unwind(AssertUnwindSafe(|| {
        // SAFETY: 调用者保证有效长度和非重叠内存。
        let bytes = unsafe { std::slice::from_raw_parts(metadata, metadata_length) };
        let request = match std::str::from_utf8(bytes) {
            Ok(text) => text,
            Err(_) => {
                return crate::api::BootstrapReply {
                    status_json: String::from_utf8(error("invalid_utf8", "请求不是 UTF-8"))
                        .unwrap(),
                    secret: Vec::new(),
                };
            }
        };
        let secret = if secret_length == 0 {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(secret, secret_length) }.to_vec()
        };
        crate::api::run_bootstrap(request.to_owned(), secret)
    }))
    .unwrap_or_else(|_| crate::api::BootstrapReply {
        status_json: String::from_utf8(error("internal_error", "核心调用异常")).unwrap(),
        secret: Vec::new(),
    });
    let secret = zeroize::Zeroizing::new(response.secret);
    let Ok(mut buffers) = BUFFERS.lock() else {
        return 0;
    };
    let Some(slot) = buffers.values.get_mut(&handle) else {
        return 0;
    };
    if secret.len() > 128 || response.status_json.len() > MAX_OUTPUT {
        *slot = error("invalid_output", "核心结果长度不符");
        return handle;
    }
    *slot = response.status_json.into_bytes();
    // SAFETY: 容量与来源独立已在入口校验，调用方承担有效指针契约。
    unsafe {
        std::ptr::write_bytes(output, 0, 128);
        std::ptr::copy_nonoverlapping(secret.as_ptr(), output, secret.len());
        *written = secret.len();
    }
    handle
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bootstrap_secret_is_separate_and_invalid_capacity_has_no_output() {
        let metadata = br#"{"abi":1,"directory":"/synthetic","run":"00000000000000000000000000000000","action":{"kind":"newRecovery"}}"#;
        let mut output = zeroize::Zeroizing::new([0_u8; 128]);
        let mut written = 99;
        // SAFETY: all pointers refer to distinct live buffers with the supplied capacities.
        let invalid = unsafe {
            radishledger_bootstrap_execute(
                metadata.as_ptr(),
                metadata.len(),
                std::ptr::null(),
                0,
                output.as_mut_ptr(),
                127,
                &mut written,
            )
        };
        assert_eq!(written, 0);
        assert_eq!(radishledger_probe_buffer_free(invalid), 1);
        // SAFETY: same valid allocations; full output capacity is supplied.
        let handle = unsafe {
            radishledger_bootstrap_execute(
                metadata.as_ptr(),
                metadata.len(),
                std::ptr::null(),
                0,
                output.as_mut_ptr(),
                128,
                &mut written,
            )
        };
        assert_eq!(written, 80);
        let length = radishledger_probe_buffer_len(handle);
        let mut status = vec![0; length];
        // SAFETY: status has the queried capacity and lives through the copy.
        assert_eq!(
            unsafe { radishledger_probe_buffer_copy(handle, status.as_mut_ptr(), length) },
            length
        );
        let value: serde_json::Value = serde_json::from_slice(&status).unwrap();
        assert_eq!(value["ok"], true);
        assert!(
            !status
                .windows(written)
                .any(|part| part == &output[..written])
        );
        assert_eq!(radishledger_probe_buffer_free(handle), 1);
    }

    #[test]
    fn invalid_handles_and_double_free_are_bounded() {
        let handle = insert(error("synthetic", "合成错误"));
        assert_ne!(handle, 0);
        let length = radishledger_probe_buffer_len(handle);
        assert!(length > 0);
        let mut out = vec![0_u8; length];
        // SAFETY: out 是长度足够、存活的可写 buffer。
        assert_eq!(
            unsafe { radishledger_probe_buffer_copy(handle, out.as_mut_ptr(), length - 1) },
            0
        );
        assert!(out.iter().all(|b| *b == 0));
        assert_eq!(
            unsafe { radishledger_probe_buffer_copy(handle, out.as_mut_ptr(), length) },
            length
        );
        assert_eq!(radishledger_probe_buffer_free(handle), 1);
        assert_eq!(radishledger_probe_buffer_free(handle), 0);
        assert_eq!(radishledger_probe_buffer_len(handle), 0);
    }

    #[test]
    fn invalid_utf8_has_a_structured_error() {
        let invalid = [0xff_u8];
        // SAFETY: 单字节数组在调用期间可读且存活。
        let handle = unsafe { radishledger_probe_run(invalid.as_ptr(), invalid.len()) };
        let mut out = vec![0; radishledger_probe_buffer_len(handle)];
        assert_eq!(
            unsafe { radishledger_probe_buffer_copy(handle, out.as_mut_ptr(), out.len()) },
            out.len()
        );
        let response: serde_json::Value = serde_json::from_slice(&out).expect("structured error");
        assert_eq!(response["error"]["code"], "invalid_utf8");
        assert_eq!(radishledger_probe_buffer_free(handle), 1);
    }
}
