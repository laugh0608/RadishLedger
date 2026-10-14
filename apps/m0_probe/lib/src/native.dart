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
