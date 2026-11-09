// 两种专用合成入口共用传输边界；调用者负责固定字段投影，不能传秘密。
import 'dart:convert';
import 'dart:math';

const probeReportMaxBytes = 96 * 1024;
const probeReportChunkSize = 700;

List<String> probeReportChunks(List<int> bytes) {
  if (bytes.isEmpty || bytes.length > probeReportMaxBytes) {
    throw StateError('合成报告超出有界传输上限');
  }
  final encoded = base64Encode(bytes);
  return [
    for (
      var offset = 0;
      offset < encoded.length;
      offset += probeReportChunkSize
    )
      encoded.substring(
        offset,
        min(offset + probeReportChunkSize, encoded.length),
      ),
  ];
}

Future<void> emitProbeReport(
  String marker,
  List<String> chunks,
  void Function(String) output,
) async {
  if (!RegExp(
        r'^RLM0_(APP_BOUNDARY:[0-9a-f]{32}|BOOTSTRAP:[0-9a-f]{32}:[0-9a-f]{32})$',
      ).hasMatch(marker) ||
      chunks.isEmpty ||
      chunks.length > 188) {
    throw StateError('合成报告标记或分片数量无效');
  }
  for (var i = 0; i < chunks.length; i++) {
    if (!RegExp(r'^[A-Za-z0-9+/=]+$').hasMatch(chunks[i]) ||
        chunks[i].length > probeReportChunkSize ||
        (i != chunks.length - 1 && chunks[i].length != probeReportChunkSize)) {
      throw StateError('合成报告分片格式无效');
    }
  }
  if (base64Decode(chunks.join()).length > probeReportMaxBytes) {
    throw StateError('合成报告超出有界传输上限');
  }
  for (var i = 0; i < chunks.length; i++) {
    output('$marker:$i:${chunks.length}:${chunks[i]}');
    await Future<void>.delayed(const Duration(milliseconds: 3));
  }
}
