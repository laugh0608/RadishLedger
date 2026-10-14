import 'dart:convert';
import 'dart:ffi';
import 'package:ffi/ffi.dart';

@Native<Uint32 Function()>(symbol: 'radishledger_probe_abi')
external int _abi();
@Native<Uint64 Function(Pointer<Uint8>, UintPtr)>(
  symbol: 'radishledger_probe_run',
)
external int _run(Pointer<Uint8> input, int length);
@Native<UintPtr Function(Uint64)>(symbol: 'radishledger_probe_buffer_len')
external int _length(int handle);
@Native<UintPtr Function(Uint64, Pointer<Uint8>, UintPtr)>(
  symbol: 'radishledger_probe_buffer_copy',
)
external int _copy(int handle, Pointer<Uint8> destination, int capacity);
@Native<Int32 Function(Uint64)>(symbol: 'radishledger_probe_buffer_free')
external int _free(int handle);
@Native<Uint64 Function()>(symbol: 'radishledger_probe_library_path')
external int _libraryPath();

String _consume(int handle) {
  if (handle == 0) throw StateError('核心未分配有效结果句柄');
  Pointer<Uint8>? destination;
  try {
    final length = _length(handle);
    if (length <= 0 || length > 1024 * 1024) throw StateError('核心结果长度无效');
    destination = calloc<Uint8>(length);
    if (_copy(handle, destination, length) != length) {
      throw StateError('核心结果复制失败');
    }
    return utf8.decode(destination.asTypedList(length));
  } finally {
    if (destination != null) calloc.free(destination);
    if (_free(handle) != 1) throw StateError('核心结果释放失败');
  }
}

void _checkAbi() {
  if (_abi() != 1) throw StateError('验证应用与核心 ABI 不兼容');
}

String runThin(String directory) {
  _checkAbi();
  final bytes = utf8.encode(directory);
  if (bytes.isEmpty || bytes.length > 4096) {
    throw const FormatException('实验目录长度无效');
  }
  final input = calloc<Uint8>(bytes.length);
  try {
    input.asTypedList(bytes.length).setAll(0, bytes);
    return _consume(_run(input, bytes.length));
  } finally {
    calloc.free(input);
  }
}

String loadedLibraryPath() {
  _checkAbi();
  return _consume(_libraryPath());
}

@Native<Uint64 Function(Pointer<Uint8>, UintPtr)>(
  symbol: 'radishledger_probe_validate',
)
external int _validate(Pointer<Uint8> input, int length);
@Native<Int32 Function()>(symbol: 'radishledger_probe_buffer_count')
external int nativeBufferCount();

String runThinValidation(String request) {
  _checkAbi();
  final bytes = utf8.encode(request);
  if (bytes.isEmpty || bytes.length > 4096) {
    throw const FormatException('实验请求长度无效');
  }
  final input = calloc<Uint8>(bytes.length);
  try {
    input.asTypedList(bytes.length).setAll(0, bytes);
    return _consume(_validate(input, bytes.length));
  } finally {
    calloc.free(input);
  }
}

/// 实际 ABI 边界检查；只使用有效内存，不能用悬空指针测试来触发 UB。
void verifyNativeBufferBounds() {
  if (nativeBufferCount() != 0) throw StateError('开始前存在未释放句柄');
  final handles = <int>[];
  try {
    for (var i = 0; i < 16; i++) {
      final handle = _libraryPath();
      if (handle == 0) throw StateError('句柄上限以内分配失败');
      handles.add(handle);
    }
    if (_libraryPath() != 0 || nativeBufferCount() != 16) {
      throw StateError('句柄上限未生效');
    }
    final length = _length(handles.first);
    final output = calloc<Uint8>(length);
    try {
      output.asTypedList(length).fillRange(0, length, 0x5a);
      if (_copy(handles.first, output, length - 1) != 0 ||
          output.asTypedList(length).any((b) => b != 0x5a)) {
        throw StateError('短输出缓冲区被写入');
      }
    } finally {
      calloc.free(output);
    }
  } finally {
    for (final handle in handles) {
      if (_free(handle) != 1 || _free(handle) != 0 || _length(handle) != 0) {
        throw StateError('释放 / 重复释放契约不符');
      }
    }
  }
  final input = calloc<Uint8>(1)..value = 0xff;
  try {
    for (final pair in [
      (_validate(input, 1), 'invalid_utf8'),
      (_validate(input, 4097), 'invalid_argument'),
      (_validate(nullptr, 0), 'invalid_argument'),
    ]) {
      final result = jsonDecode(_consume(pair.$1)) as Map<String, dynamic>;
      if (result['ok'] != false || result['error']['code'] != pair.$2) {
        throw StateError('输入边界没有明确拒绝');
      }
    }
  } finally {
    calloc.free(input);
  }
  if (nativeBufferCount() != 0) throw StateError('边界检查残留句柄');
}
