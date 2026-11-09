//! 透传原 VFS，观察真实 SQLCipher IO。宿主子进程可设为默认，App 仅用命名注册。
//! 契约来源：https://www.sqlite.org/c3ref/vfs.html 与 c3ref/io_methods.html。
use rusqlite::ffi;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::os::unix::fs::MetadataExt;
use std::ptr;
use std::sync::Mutex;

pub const MARKER: &[u8] = b"RadishLedger synthetic temporary boundary 2026-10-19";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    Database,
    Wal,
    Journal,
    TempDatabase,
    TempJournal,
    SubJournal,
    Transient,
    SuperJournal,
    Other,
}

impl Kind {
    fn from_flags(flags: c_int) -> Self {
        for (flag, kind) in [
            (ffi::SQLITE_OPEN_MAIN_DB, Self::Database),
            (ffi::SQLITE_OPEN_WAL, Self::Wal),
            (ffi::SQLITE_OPEN_MAIN_JOURNAL, Self::Journal),
            (ffi::SQLITE_OPEN_TEMP_DB, Self::TempDatabase),
            (ffi::SQLITE_OPEN_TEMP_JOURNAL, Self::TempJournal),
            (ffi::SQLITE_OPEN_SUBJOURNAL, Self::SubJournal),
            (ffi::SQLITE_OPEN_TRANSIENT_DB, Self::Transient),
            (ffi::SQLITE_OPEN_SUPER_JOURNAL, Self::SuperJournal),
        ] {
            if flags & flag != 0 {
                return kind;
            }
        }
        Self::Other
    }

    pub fn temporary(self) -> bool {
        matches!(
            self,
            Self::TempDatabase | Self::TempJournal | Self::SubJournal | Self::Transient
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operation {
    Write,
    Sync,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Phase {
    #[default]
    Setup,
    Commit,
    Checkpoint,
    TempTable,
    OrderBy,
    AutoIndex,
    Materialization,
    StatementRollback,
    TempIndex,
    Vacuum,
    MultiDatabaseCommit,
    Close,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum Failure {
    PartialWrite,
    SyncBefore,
    SyncAfter,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Fault {
    pub event: usize,
    pub failure: Failure,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    pub kind: Kind,
    pub operation: Operation,
    pub amount: i32,
    pub offset: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileObservation {
    pub kind: Kind,
    pub flags: i32,
    pub anonymous: bool,
    pub path: Option<String>,
    pub mode: Option<u32>,
    pub owner_uid: Option<u32>,
    pub opened_during: Phase,
    pub written_during: BTreeSet<Phase>,
    pub shm_maps: usize,
    pub writes: usize,
    pub bytes_written: u64,
    pub syncs: usize,
    pub write_contains_marker: bool,
    pub scanned_bytes: u64,
    pub disk_contains_marker: bool,
    pub scan_code: i32,
    pub closed: bool,
    pub close_code: Option<i32>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Observation {
    pub phase: Phase,
    pub files: Vec<FileObservation>,
    pub events: Vec<Event>,
    pub injected: usize,
    pub partial_bytes: i32,
    pub delegated_syncs_before_error: usize,
    pub fault: Option<Fault>,
}

struct Context {
    parent: *mut ffi::sqlite3_vfs,
    registered: *mut ffi::sqlite3_vfs,
    observation: Mutex<Observation>,
}

// SQLite 分配首字段；real 由 sqlite3_malloc64 分配并保留原 pMethods / 对齐。
#[repr(C)]
struct File {
    base: ffi::sqlite3_file,
    real: *mut ffi::sqlite3_file,
    context: *const Context,
    index: usize,
}

pub struct Registration {
    vfs: Box<ffi::sqlite3_vfs>,
    context: Box<Context>,
    name: CString,
    make_default: bool,
}

impl Registration {
    #[cfg(test)]
    pub fn new() -> Self {
        Self::register("radishledger-test-observer", true)
    }

    pub fn named(name: &str) -> Self {
        Self::register(name, false)
    }

    pub fn name(&self) -> &str {
        self.name.to_str().unwrap()
    }

    fn register(name: &str, make_default: bool) -> Self {
        // SAFETY: App 采用唯一名称且不设为默认；对象保持稳定，所有连接先关闭再注销。
        unsafe {
            assert_eq!(ffi::sqlite3_initialize(), ffi::SQLITE_OK);
            let parent = ffi::sqlite3_vfs_find(ptr::null());
            assert!(!parent.is_null());
            let mut context = Box::new(Context {
                parent,
                registered: ptr::null_mut(),
                observation: Mutex::new(Observation::default()),
            });
            let name = CString::new(name).unwrap();
            let mut vfs = Box::new(ffi::sqlite3_vfs {
                iVersion: 1,
                szOsFile: size_of::<File>() as c_int,
                mxPathname: (*parent).mxPathname,
                pNext: ptr::null_mut(),
                zName: name.as_ptr(),
                pAppData: (&mut *context as *mut Context).cast(),
                xOpen: Some(open),
                xDelete: Some(delete),
                xAccess: Some(access),
                xFullPathname: Some(full_path),
                xDlOpen: Some(dl_open),
                xDlError: Some(dl_error),
                xDlSym: Some(dl_sym),
                xDlClose: Some(dl_close),
                xRandomness: Some(randomness),
                xSleep: Some(sleep),
                xCurrentTime: Some(current_time),
                xGetLastError: Some(last_error),
                xCurrentTimeInt64: None,
                xSetSystemCall: None,
                xGetSystemCall: None,
                xNextSystemCall: None,
            });
            context.registered = &mut *vfs;
            assert_eq!(
                ffi::sqlite3_vfs_register(&mut *vfs, i32::from(make_default)),
                ffi::SQLITE_OK
            );
            Self {
                vfs,
                context,
                name,
                make_default,
            }
        }
    }

    pub fn arm(&self, fault: Option<Fault>) {
        let mut state = self.context.observation.lock().unwrap();
        state.events.clear();
        state.fault = fault;
        state.injected = 0;
    }

    pub fn phase(&self, phase: Phase) {
        self.context.observation.lock().unwrap().phase = phase;
    }

    pub fn report(&self) -> Observation {
        self.context.observation.lock().unwrap().clone()
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        // xClose 即使报错也已释放 shim 的文件内存，不能再按“仍存活”触发二次 panic。
        // closed 保留成功判据；close_code 单独证明回调已结束，失败仍会阻断验证。
        assert!(self.report().files.iter().all(|f| f.close_code.is_some()));
        // SAFETY: 本实例保持 Box 地址稳定，全部文件已关闭，回收前恢复原默认 VFS。
        unsafe {
            assert_eq!(ffi::sqlite3_vfs_unregister(&mut *self.vfs), ffi::SQLITE_OK);
            if self.make_default {
                assert_eq!(
                    ffi::sqlite3_vfs_register(self.context.parent, 1),
                    ffi::SQLITE_OK
                );
            }
        }
    }
}

// C 回调内部遇到不可能的契约 / mutex 损坏时终止测试子进程，绝不 unwind 跨 FFI。
fn required<T>(value: Option<T>) -> T {
    value.unwrap_or_else(|| std::process::abort())
}

fn record(file: &File, operation: Operation, amount: i32, offset: i64) -> Option<Failure> {
    // SAFETY: context 的 Box 比全部打开文件活得久，index 在 xOpen 时登记。
    let context = unsafe { &*file.context };
    let mut state = context
        .observation
        .lock()
        .unwrap_or_else(|_| std::process::abort());
    let kind = required(state.files.get(file.index)).kind;
    // 工作负载有界；不静默截断证据。
    if state.events.len() >= 100_000 {
        std::process::abort();
    }
    state.events.push(Event {
        kind,
        operation,
        amount,
        offset,
    });
    let ordinal = state.events.len();
    if let Some(fault) = state.fault
        && state.injected == 0
        && fault.event == ordinal
    {
        state.injected += 1;
        Some(fault.failure)
    } else {
        None
    }
}

unsafe extern "C" fn open(
    vfs: *mut ffi::sqlite3_vfs,
    name: *const c_char,
    file: *mut ffi::sqlite3_file,
    flags: c_int,
    output: *mut c_int,
) -> c_int {
    // SAFETY: SQLite 按 szOsFile 分配 file；真实文件单独分配并传给父 VFS。
    unsafe {
        (*file).pMethods = ptr::null();
        let context = &*((*vfs).pAppData.cast::<Context>());
        let parent = context.parent;
        let real = ffi::sqlite3_malloc64((*parent).szOsFile as u64).cast::<ffi::sqlite3_file>();
        if real.is_null() {
            return ffi::SQLITE_NOMEM;
        }
        ptr::write_bytes(real.cast::<u8>(), 0, (*parent).szOsFile as usize);
        let code = required((*parent).xOpen)(parent, name, real, flags, output);
        if code != ffi::SQLITE_OK {
            if !(*real).pMethods.is_null() {
                required((*(*real).pMethods).xClose)(real);
            }
            ffi::sqlite3_free(real.cast());
            return code;
        }
        if (*real).pMethods.is_null() || (*(*real).pMethods).iVersion < 2 {
            std::process::abort();
        }
        let mut state = context
            .observation
            .lock()
            .unwrap_or_else(|_| std::process::abort());
        let index = state.files.len();
        let phase = state.phase;
        let path = if name.is_null() {
            None
        } else {
            Some(CStr::from_ptr(name).to_string_lossy().into_owned())
        };
        let metadata = path
            .as_ref()
            .and_then(|path| std::fs::symlink_metadata(path).ok());
        state.files.push(FileObservation {
            kind: Kind::from_flags(flags),
            flags,
            anonymous: name.is_null(),
            path,
            mode: metadata.as_ref().map(|info| info.mode() & 0o777),
            owner_uid: metadata.as_ref().map(|info| info.uid()),
            opened_during: phase,
            written_during: BTreeSet::new(),
            shm_maps: 0,
            writes: 0,
            bytes_written: 0,
            syncs: 0,
            write_contains_marker: false,
            scanned_bytes: 0,
            disk_contains_marker: false,
            scan_code: ffi::SQLITE_OK,
            closed: false,
            close_code: None,
        });
        file.cast::<File>().write(File {
            base: ffi::sqlite3_file { pMethods: &METHODS },
            real,
            context,
            index,
        });
        code
    }
}

unsafe extern "C" fn write(
    file: *mut ffi::sqlite3_file,
    buffer: *const c_void,
    amount: c_int,
    offset: i64,
) -> c_int {
    // SAFETY: SQLite 保证 buffer 在本次调用中含 amount 字节；不保留载荷。
    unsafe {
        let f = &*file.cast::<File>();
        let failure = record(f, Operation::Write, amount, offset);
        let bytes = std::slice::from_raw_parts(buffer.cast::<u8>(), amount as usize);
        let partial = matches!(failure, Some(Failure::PartialWrite));
        if failure.is_some() && !partial {
            std::process::abort();
        }
        if partial && amount < 2 {
            std::process::abort();
        }
        let written = if partial {
            (amount / 2).clamp(1, 7)
        } else {
            amount
        };
        let code = required((*(*f.real).pMethods).xWrite)(f.real, buffer, written, offset);
        let mut state = (*f.context)
            .observation
            .lock()
            .unwrap_or_else(|_| std::process::abort());
        let phase = state.phase;
        let observed = required(state.files.get_mut(f.index));
        observed.written_during.insert(phase);
        observed.writes += 1;
        observed.bytes_written += if code == ffi::SQLITE_OK {
            written as u64
        } else {
            0
        };
        observed.write_contains_marker |= bytes.windows(MARKER.len()).any(|w| w == MARKER);
        if partial && code == ffi::SQLITE_OK {
            state.partial_bytes = written;
            ffi::SQLITE_IOERR_WRITE
        } else {
            code
        }
    }
}

unsafe extern "C" fn sync(file: *mut ffi::sqlite3_file, flags: c_int) -> c_int {
    // SAFETY: 原始 xSync 收到原始文件与原 flags；只在指定序号注入一次。
    unsafe {
        let f = &*file.cast::<File>();
        let failure = record(f, Operation::Sync, flags, 0);
        let mut state = (*f.context)
            .observation
            .lock()
            .unwrap_or_else(|_| std::process::abort());
        required(state.files.get_mut(f.index)).syncs += 1;
        drop(state);
        if matches!(failure, Some(Failure::SyncBefore)) {
            return ffi::SQLITE_IOERR_FSYNC;
        }
        if matches!(failure, Some(Failure::PartialWrite)) {
            std::process::abort();
        }
        let code = required((*(*f.real).pMethods).xSync)(f.real, flags);
        if matches!(failure, Some(Failure::SyncAfter)) && code == ffi::SQLITE_OK {
            (*f.context)
                .observation
                .lock()
                .unwrap_or_else(|_| std::process::abort())
                .delegated_syncs_before_error += 1;
            ffi::SQLITE_IOERR_FSYNC
        } else {
            code
        }
    }
}

unsafe extern "C" fn close(file: *mut ffi::sqlite3_file) -> c_int {
    // SAFETY: 在父 xClose 之前通过其 xRead 扫描，包含已 unlink 的匿名文件。
    // 最多 64 MiB，逐块带重叠检查；不导出任何载荷。超界 / 读取失败明确记录。
    unsafe {
        let f = &mut *file.cast::<File>();
        let methods = &*(*f.real).pMethods;
        let mut size = 0;
        let mut scan_code = required(methods.xFileSize)(f.real, &mut size);
        let mut scanned = 0_u64;
        let mut found = false;
        if !(0..=64 * 1024 * 1024).contains(&size) {
            scan_code = ffi::SQLITE_TOOBIG;
        }
        if scan_code == ffi::SQLITE_OK {
            let mut buffer = vec![0_u8; 65536 + MARKER.len() - 1];
            let mut overlap = 0;
            while (scanned as i64) < size {
                let count = (size - scanned as i64).min(65536) as usize;
                scan_code = required(methods.xRead)(
                    f.real,
                    buffer.as_mut_ptr().add(overlap).cast(),
                    count as i32,
                    scanned as i64,
                );
                if scan_code != ffi::SQLITE_OK {
                    break;
                }
                let end = overlap + count;
                found |= buffer[..end].windows(MARKER.len()).any(|w| w == MARKER);
                overlap = end.min(MARKER.len() - 1);
                buffer.copy_within(end - overlap..end, 0);
                scanned += count as u64;
            }
        }
        let code = required(methods.xClose)(f.real);
        ffi::sqlite3_free(f.real.cast());
        f.base.pMethods = ptr::null();
        let mut state = (*f.context)
            .observation
            .lock()
            .unwrap_or_else(|_| std::process::abort());
        let observed = required(state.files.get_mut(f.index));
        observed.scanned_bytes = scanned;
        observed.disk_contains_marker = found;
        observed.scan_code = scan_code;
        observed.closed = code == ffi::SQLITE_OK;
        observed.close_code = Some(code);
        code
    }
}

// 其余 IO 完整透传真实文件，保留原锁 / SHM / mmap 语义。
macro_rules! io_forward {
    ($name:ident, $field:ident, ($($arg:ident: $ty:ty),*) -> $result:ty) => {
        unsafe extern "C" fn $name(file: *mut ffi::sqlite3_file, $($arg: $ty),*) -> $result {
            // SAFETY: wrapper 与 real 一一对应；签名来自固定 SQLite 绑定。
            unsafe {
                let f = &*file.cast::<File>();
                required((*(*f.real).pMethods).$field)(f.real, $($arg),*)
            }
        }
    };
}
io_forward!(read, xRead, (buffer: *mut c_void, amount: c_int, offset: i64) -> c_int);
io_forward!(truncate, xTruncate, (size: i64) -> c_int);
io_forward!(file_size, xFileSize, (size: *mut i64) -> c_int);
io_forward!(lock, xLock, (level: c_int) -> c_int);
io_forward!(unlock, xUnlock, (level: c_int) -> c_int);
io_forward!(reserved, xCheckReservedLock, (output: *mut c_int) -> c_int);
io_forward!(sector_size, xSectorSize, () -> c_int);
io_forward!(characteristics, xDeviceCharacteristics, () -> c_int);
unsafe extern "C" fn shm_map(
    file: *mut ffi::sqlite3_file,
    page: c_int,
    size: c_int,
    extend: c_int,
    output: *mut *mut c_void,
) -> c_int {
    // SAFETY: SHM 仍由父 VFS 管理；只计实际返回的映射，不复制映射内容。
    unsafe {
        let f = &*file.cast::<File>();
        let code = required((*(*f.real).pMethods).xShmMap)(f.real, page, size, extend, output);
        if code == ffi::SQLITE_OK && !(*output).is_null() {
            let mut state = (*f.context)
                .observation
                .lock()
                .unwrap_or_else(|_| std::process::abort());
            required(state.files.get_mut(f.index)).shm_maps += 1;
        }
        code
    }
}
io_forward!(shm_lock, xShmLock, (offset: c_int, count: c_int, flags: c_int) -> c_int);
io_forward!(shm_barrier, xShmBarrier, () -> ());
io_forward!(shm_unmap, xShmUnmap, (delete: c_int) -> c_int);

unsafe extern "C" fn control(
    file: *mut ffi::sqlite3_file,
    operation: c_int,
    arg: *mut c_void,
) -> c_int {
    // SAFETY: 对两个指针查询保留顶层 shim；其他 opcode 交给原文件。
    unsafe {
        let f = &*file.cast::<File>();
        if operation == ffi::SQLITE_FCNTL_VFS_POINTER {
            arg.cast::<*mut ffi::sqlite3_vfs>()
                .write((*f.context).registered);
            ffi::SQLITE_OK
        } else if operation == ffi::SQLITE_FCNTL_FILE_POINTER {
            arg.cast::<*mut ffi::sqlite3_file>().write(file);
            ffi::SQLITE_OK
        } else {
            required((*(*f.real).pMethods).xFileControl)(f.real, operation, arg)
        }
    }
}

unsafe extern "C" fn fetch(
    file: *mut ffi::sqlite3_file,
    offset: i64,
    amount: c_int,
    output: *mut *mut c_void,
) -> c_int {
    // SAFETY: v3 透传 mmap；v2 父 VFS 明确表示没有映射，由 SQLite 使用 xRead。
    unsafe {
        let f = &*file.cast::<File>();
        let methods = &*(*f.real).pMethods;
        if methods.iVersion >= 3
            && let Some(callback) = methods.xFetch
        {
            callback(f.real, offset, amount, output)
        } else {
            output.write(ptr::null_mut());
            ffi::SQLITE_OK
        }
    }
}

unsafe extern "C" fn unfetch(
    file: *mut ffi::sqlite3_file,
    offset: i64,
    mapping: *mut c_void,
) -> c_int {
    // SAFETY: 映射只能来自同一个父 VFS 的 xFetch。
    unsafe {
        let f = &*file.cast::<File>();
        let methods = &*(*f.real).pMethods;
        if methods.iVersion >= 3
            && let Some(callback) = methods.xUnfetch
        {
            callback(f.real, offset, mapping)
        } else {
            ffi::SQLITE_OK
        }
    }
}

static METHODS: ffi::sqlite3_io_methods = ffi::sqlite3_io_methods {
    iVersion: 3,
    xClose: Some(close),
    xRead: Some(read),
    xWrite: Some(write),
    xTruncate: Some(truncate),
    xSync: Some(sync),
    xFileSize: Some(file_size),
    xLock: Some(lock),
    xUnlock: Some(unlock),
    xCheckReservedLock: Some(reserved),
    xFileControl: Some(control),
    xSectorSize: Some(sector_size),
    xDeviceCharacteristics: Some(characteristics),
    xShmMap: Some(shm_map),
    xShmLock: Some(shm_lock),
    xShmBarrier: Some(shm_barrier),
    xShmUnmap: Some(shm_unmap),
    xFetch: Some(fetch),
    xUnfetch: Some(unfetch),
};

macro_rules! vfs_forward {
    ($name:ident, $field:ident, ($($arg:ident: $ty:ty),*) -> $result:ty) => {
        unsafe extern "C" fn $name(vfs: *mut ffi::sqlite3_vfs, $($arg: $ty),*) -> $result {
            // SAFETY: pAppData 仅存本次 Context，父回调收到其自身 VFS 指针。
            unsafe {
                let parent = (*((*vfs).pAppData.cast::<Context>())).parent;
                required((*parent).$field)(parent, $($arg),*)
            }
        }
    };
}
vfs_forward!(delete, xDelete, (name: *const c_char, sync_dir: c_int) -> c_int);
vfs_forward!(access, xAccess, (name: *const c_char, flags: c_int, result: *mut c_int) -> c_int);
vfs_forward!(full_path, xFullPathname, (name: *const c_char, count: c_int, output: *mut c_char) -> c_int);
vfs_forward!(dl_open, xDlOpen, (name: *const c_char) -> *mut c_void);
vfs_forward!(dl_error, xDlError, (count: c_int, output: *mut c_char) -> ());
type Symbol = Option<unsafe extern "C" fn(*mut ffi::sqlite3_vfs, *mut c_void, *const c_char)>;
vfs_forward!(dl_sym, xDlSym, (handle: *mut c_void, name: *const c_char) -> Symbol);
vfs_forward!(dl_close, xDlClose, (handle: *mut c_void) -> ());
vfs_forward!(randomness, xRandomness, (count: c_int, output: *mut c_char) -> c_int);
vfs_forward!(sleep, xSleep, (microseconds: c_int) -> c_int);
vfs_forward!(current_time, xCurrentTime, (output: *mut f64) -> c_int);
vfs_forward!(last_error, xGetLastError, (count: c_int, output: *mut c_char) -> c_int);

pub fn sqlite_version() -> String {
    // SAFETY: SQLite 版本返回静态 NUL 结尾文本。
    unsafe {
        CStr::from_ptr(ffi::sqlite3_libversion())
            .to_string_lossy()
            .into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 包住真实空文件，只将关闭返回值改为 IOERR_CLOSE；资源仍由原 xClose 释放。
    #[repr(C)]
    struct FailedClose {
        base: ffi::sqlite3_file,
        real: *mut ffi::sqlite3_file,
    }

    unsafe extern "C" fn empty_size(_: *mut ffi::sqlite3_file, size: *mut i64) -> c_int {
        // SAFETY: 测试只包装刚打开的空文件；SQLite 提供有效输出指针。
        unsafe { size.write(0) };
        ffi::SQLITE_OK
    }

    unsafe extern "C" fn failed_close(file: *mut ffi::sqlite3_file) -> c_int {
        // SAFETY: 真实文件由测试转移给本回调，原 VFS 关闭后释放一次。
        unsafe {
            let real = (*file.cast::<FailedClose>()).real;
            let code = required((*(*real).pMethods).xClose)(real);
            ffi::sqlite3_free(real.cast());
            if code == ffi::SQLITE_OK {
                ffi::SQLITE_IOERR_CLOSE
            } else {
                code
            }
        }
    }

    #[test]
    fn close_error_remains_failed_but_allows_unwind_and_unregister() {
        let root = tempfile::tempdir().unwrap();
        let path = CString::new(root.path().join("empty.db").to_str().unwrap()).unwrap();
        let name = format!("radishledger-close-error-{}", crate::random_id().unwrap());
        let lookup = CString::new(name.clone()).unwrap();
        let failed = std::panic::catch_unwind(|| {
            let mut observer = Registration::named(&name);
            let mut methods = METHODS;
            methods.xClose = Some(failed_close);
            methods.xFileSize = Some(empty_size);
            // SAFETY: 按两个结构的实际大小分配，方法表存活至同步 close 返回。
            unsafe {
                let file = ffi::sqlite3_malloc64(size_of::<File>() as u64).cast::<File>();
                assert!(!file.is_null());
                let mut flags = 0;
                assert_eq!(
                    open(
                        &mut *observer.vfs,
                        path.as_ptr(),
                        file.cast(),
                        ffi::SQLITE_OPEN_MAIN_DB
                            | ffi::SQLITE_OPEN_READWRITE
                            | ffi::SQLITE_OPEN_CREATE,
                        &mut flags
                    ),
                    ffi::SQLITE_OK
                );
                let proxy =
                    ffi::sqlite3_malloc64(size_of::<FailedClose>() as u64).cast::<FailedClose>();
                assert!(!proxy.is_null());
                proxy.write(FailedClose {
                    base: ffi::sqlite3_file { pMethods: &methods },
                    real: (*file).real,
                });
                (*file).real = proxy.cast();
                let code = close(file.cast());
                ffi::sqlite3_free(file.cast());
                assert_eq!(code, ffi::SQLITE_IOERR_CLOSE);
            }
            let report = observer.report();
            assert_eq!(report.files.len(), 1);
            assert_eq!(report.files[0].close_code, Some(ffi::SQLITE_IOERR_CLOSE));
            // 模拟验收失败后 unwind；旧 Drop 在这里再次 panic 并终止进程。
            assert!(report.files[0].closed, "expected close failure");
        });
        let panic = failed.expect_err("关闭错误必须阻断验收");
        let message = panic
            .downcast_ref::<&str>()
            .copied()
            .or_else(|| panic.downcast_ref::<String>().map(String::as_str));
        assert_eq!(message, Some("expected close failure"));
        // SAFETY: 查询已注销的名称，不解引用返回指针。
        assert!(unsafe { ffi::sqlite3_vfs_find(lookup.as_ptr()) }.is_null());
        let connection = rusqlite::Connection::open(root.path().join("next.db")).unwrap();
        assert_eq!(
            connection
                .query_row("SELECT 1", [], |r| r.get::<_, i32>(0))
                .unwrap(),
            1
        );
    }
}
