import 'dart:io';
import 'package:flutter/material.dart';
import 'internal_ledger_app.dart';
import 'probe_app.dart';

void main() => runApp(
  Platform.isAndroid && const bool.fromEnvironment('RADISH_INTERNAL_LEDGER')
      ? const InternalLedgerApp()
      : const ProbeApp(),
);
